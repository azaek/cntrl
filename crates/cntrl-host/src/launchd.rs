//! launchd jobs, behind the `service.*` operations on macOS. privd runs as
//! root and acts on the system domain, where LaunchDaemons live; run as
//! another user, as in tests, it acts on that user's GUI domain instead.

use std::process::{Command, Output};
use std::thread::sleep;
use std::time::{Duration, Instant};

use cntrl_protocol::service::JobResult;

use crate::HostError;

/// How long a restarted job gets to show it's running. launchd waits up to
/// 10 s (its default ThrottleInterval) to respawn a job that only just started.
const SETTLE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);
/// launchctl's exit status for a job that isn't loaded in the domain.
const NO_SUCH_JOB: i32 = 113;

/// Restarts `label` with `launchctl kickstart -k`, which stops a running
/// instance first, then waits until launchd shows the job running, or
/// finished for a job that runs and exits. It blocks, so call it on a
/// blocking thread.
pub fn restart(label: &str) -> Result<JobResult, HostError> {
    let target = format!("{}/{label}", domain());
    let output = launchctl(&["kickstart", "-k", &target])?;
    if !output.status.success() {
        return Err(match output.status.code() {
            Some(NO_SUCH_JOB) => HostError::NotFound(format!("{label} isn't a launchd job here")),
            _ => HostError::Failed(message(&output)),
        });
    }
    let deadline = Instant::now() + SETTLE;
    loop {
        let printed = launchctl(&["print", &target])?;
        match state(&String::from_utf8_lossy(&printed.stdout)) {
            JobState::Running | JobState::Exited(0) => return Ok(JobResult::Done),
            JobState::Exited(_) => return Ok(JobResult::Failed),
            JobState::Starting if Instant::now() < deadline => sleep(POLL),
            JobState::Starting => return Ok(JobResult::Timeout),
        }
    }
}

/// Where jobs live for this process: the system domain for root, else the
/// user's GUI domain.
fn domain() -> String {
    let uid = rustix::process::getuid();
    if uid.is_root() {
        "system".to_owned()
    } else {
        format!("gui/{}", uid.as_raw())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum JobState {
    Running,
    /// Not running, and its last run ended with this status.
    Exited(i64),
    /// Not running yet, or launchd hasn't said.
    Starting,
}

/// Reads the `state` and `last exit code` lines of `launchctl print`. Apple
/// calls its format unstable, so anything unexpected reads as still starting
/// and ends in a timeout rather than a wrong answer.
fn state(printed: &str) -> JobState {
    let value = |key: &str| {
        printed.lines().find_map(|line| {
            let (name, value) = line.split_once(" = ")?;
            (name.trim() == key).then(|| value.trim().to_owned())
        })
    };
    if value("state").as_deref() == Some("running") {
        return JobState::Running;
    }
    // Such as `0`, or `78: EX_CONFIG` with the status's name.
    let code = value("last exit code").and_then(|code| code.split(':').next()?.trim().parse().ok());
    match code {
        Some(code) if value("state").as_deref() == Some("not running") => JobState::Exited(code),
        _ => JobState::Starting,
    }
}

fn launchctl(args: &[&str]) -> Result<Output, HostError> {
    Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|e| HostError::Failed(format!("can't run launchctl: {e}")))
}

fn message(output: &Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr);
    let text = text.trim();
    if text.is_empty() {
        format!("launchctl exited with {}", output.status)
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_launchctl_print() {
        let running = "gui/501/x = {\n\tactive count = 1\n\tstate = running\n\tpid = 42\n}";
        assert_eq!(state(running), JobState::Running);
        let done = "\tstate = not running\n\tlast exit code = 0\n";
        assert_eq!(state(done), JobState::Exited(0));
        let failed = "\tstate = not running\n\tlast exit code = 78: EX_CONFIG\n";
        assert_eq!(state(failed), JobState::Exited(78));
        let crashed = "\tstate = not running\n\tlast exit code = 1\n";
        assert_eq!(state(crashed), JobState::Exited(1));
        let fresh = "\tstate = not running\n\tlast exit code = (never exited)\n";
        assert_eq!(state(fresh), JobState::Starting);
        assert_eq!(state("\tstate = spawn scheduled\n"), JobState::Starting);
    }

    /// Restarts a throwaway job in this user's GUI domain:
    /// `cargo test -p cntrl-host -- --ignored` on a Mac with a desktop session.
    #[test]
    #[ignore = "loads a launchd job in the user's GUI domain"]
    fn restarts_a_job() {
        let label = "pw.cntrl.test.sleeper";
        let dir = std::env::temp_dir().join(label);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let plist = dir.join(format!("{label}.plist"));
        std::fs::write(
            &plist,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                 <key>Label</key><string>{label}</string>\
                 <key>ProgramArguments</key><array><string>/bin/sleep</string><string>600</string></array>\
                 <key>KeepAlive</key><true/></dict></plist>"
            ),
        )
        .expect("plist");
        let domain = domain();
        let plist_path = plist.to_string_lossy().into_owned();
        launchctl(&["bootstrap", &domain, &plist_path]).expect("bootstrap");
        let result = restart(label);
        let _ = launchctl(&["bootout", &format!("{domain}/{label}")]);
        assert_eq!(result, Ok(JobResult::Done));
        assert!(matches!(
            restart("pw.cntrl.test.missing"),
            Err(HostError::NotFound(_))
        ));
    }
}
