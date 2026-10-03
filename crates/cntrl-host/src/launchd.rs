//! launchd jobs, behind the `service.*` operations on macOS: the system domain,
//! where LaunchDaemons live, and each logged-in user's GUI domain, with their
//! LaunchAgents and open apps. privd runs as root; run as another user, as in
//! tests, it acts on that user's GUI domain in place of the system's.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use cntrl_protocol::service::{JobResult, ServiceKind, ServiceScope, ServiceState, ServiceStatus};

use crate::HostError;

/// How long a restarted job gets to show it's running. launchd waits up to
/// 10 s (its default ThrottleInterval) to respawn a job that only just started.
const SETTLE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);
/// launchctl's exit status for a job that isn't loaded in the domain.
const NO_SUCH_JOB: i32 = 113;

/// Restarts `label` in `domain` with `launchctl kickstart -k`, which stops a
/// running instance first, then waits until launchd shows the job running, or
/// finished for a job that runs and exits. It blocks, so call it on a
/// blocking thread.
pub fn restart(domain: &str, label: &str) -> Result<JobResult, HostError> {
    let target = format!("{domain}/{label}");
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

/// The jobs in the system domain, where LaunchDaemons live, by label, each with
/// `protected: false` for the caller to fill in. Listing needs no root. It
/// blocks, so call it on a blocking thread.
pub fn list() -> Result<Vec<ServiceStatus>, HostError> {
    let output = launchctl(&["print", "system"])?;
    if !output.status.success() {
        return Err(HostError::Failed(message(&output)));
    }
    let mut services = services(&String::from_utf8_lossy(&output.stdout));
    services.sort_by(|a, b| a.unit.cmp(&b.unit));
    Ok(services)
}

/// Reads the `services = { … }` block of `launchctl print`: a line per job with
/// its PID (0 when it isn't running), its last exit status (`-` before it has
/// exited; `(pe)` and other markers are undocumented) and its label.
fn services(printed: &str) -> Vec<ServiceStatus> {
    printed
        .lines()
        .skip_while(|line| line.trim() != "services = {")
        .skip(1)
        .take_while(|line| line.trim() != "}")
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid: u32 = fields.next()?.parse().ok()?;
            let status = fields.next()?;
            let label = fields.next()?;
            if fields.next().is_some() {
                return None;
            }
            let (state, detail) = match status.parse::<i64>() {
                _ if pid > 0 => (ServiceState::Running, None),
                Ok(0) => (ServiceState::Exited, Some("exited 0".to_owned())),
                Ok(code) if code < 0 => (
                    ServiceState::Failed,
                    Some(format!("killed by signal {}", -code)),
                ),
                Ok(code) => (ServiceState::Failed, Some(format!("exited {code}"))),
                Err(_) => (ServiceState::Stopped, None),
            };
            Some(ServiceStatus {
                unit: label.to_owned(),
                description: None,
                state,
                detail,
                pid: (pid > 0).then_some(pid),
                protected: false,
                scope: ServiceScope::System,
                user: None,
                kind: ServiceKind::Service,
            })
        })
        .collect()
}

/// The users with a desktop session, by name and ID, from the system
/// configuration store's console user record. Empty when nobody is logged in.
pub fn sessions() -> Vec<(String, u32)> {
    let child = Command::new("/usr/sbin/scutil")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return Vec::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"show State:/Users/ConsoleUser\n");
    }
    child
        .wait_with_output()
        .map(|output| parse_sessions(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

/// Pairs each session's `kCGSSessionUserNameKey` with its
/// `kCGSSessionUserIDKey`, leaving out root and service accounts.
fn parse_sessions(text: &str) -> Vec<(String, u32)> {
    let mut sessions = Vec::new();
    let (mut name, mut uid) = (None::<String>, None::<u32>);
    for line in text.lines() {
        let Some((key, value)) = line.split_once(" : ") else {
            continue;
        };
        match key.trim() {
            "kCGSSessionUserNameKey" => name = Some(value.trim().to_owned()),
            "kCGSSessionUserIDKey" => uid = value.trim().parse().ok(),
            _ => continue,
        }
        if let (Some(user), Some(id)) = (&name, uid) {
            if id != 0 && !user.starts_with('_') {
                sessions.push((user.clone(), id));
            }
            (name, uid) = (None, None);
        }
    }
    sessions.sort();
    sessions.dedup();
    sessions
}

/// What runs in a user's desktop session: their LaunchAgents and the apps they
/// have open, each app named by its bundle ID and described by its name.
/// Reading another user's domain needs root. It blocks.
pub fn list_session(user: &str, uid: u32) -> Result<Vec<ServiceStatus>, HostError> {
    let domain = user_domain(uid);
    let output = launchctl(&["print", &domain])?;
    if !output.status.success() {
        return Err(HostError::Failed(message(&output)));
    }
    let mut services: Vec<ServiceStatus> = services(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .map(|mut service| {
            service.scope = ServiceScope::User;
            service.user = Some(user.to_owned());
            if service.unit.starts_with("application.") {
                describe_app(&domain, &mut service);
            }
            service
        })
        .collect();
    services.sort_by(|a, b| a.unit.cmp(&b.unit));
    Ok(services)
}

/// An open app's job is `application.<bundle ID>.<n>.<n>`, which changes with
/// every launch. launchd knows the bundle ID and the program, whose `.app`
/// folder gives the app's name.
fn describe_app(domain: &str, service: &mut ServiceStatus) {
    service.kind = ServiceKind::App;
    let Ok(output) = launchctl(&["print", &format!("{domain}/{}", service.unit)]) else {
        return;
    };
    let printed = String::from_utf8_lossy(&output.stdout);
    let field = |key: &str| {
        printed.lines().find_map(|line| {
            let (name, value) = line.split_once(" = ")?;
            (name.trim() == key).then(|| value.trim().to_owned())
        })
    };
    if let Some(bundle) = field("bundle id") {
        service.unit = bundle;
    }
    service.description = field("program").as_deref().and_then(app_name);
}

/// `HEAD` from `/Applications/HEAD.app/Contents/MacOS/head`.
fn app_name(program: &str) -> Option<String> {
    program
        .split('/')
        .find_map(|part| part.strip_suffix(".app"))
        .map(str::to_owned)
}

/// A user's GUI domain, where their LaunchAgents and apps live.
pub fn user_domain(uid: u32) -> String {
    format!("gui/{uid}")
}

/// Where privd acts at system scope: the system domain when it's root, else
/// (in tests) its own user's GUI domain.
pub fn system_domain() -> String {
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

    #[test]
    fn reads_the_services_block() {
        let printed = "system = {\n\ttype = system\n\tservices = {\n\t\t     214      - \tcom.apple.runningboardd\n\t\t       0      0 \tpw.cntrl.privd\n\t\t       0      1 \tcom.example.broken\n\t\t       0     -9 \tcom.example.killed\n\t\t       0   (pe) \tcom.apple.lskdd\n\t}\n\tdisabled services = {\n\t\t\"com.openssh.sshd\" => enabled\n\t}\n}";
        let services = services(printed);
        let states: Vec<_> = services
            .iter()
            .map(|s| (s.unit.as_str(), s.state, s.pid, s.detail.as_deref()))
            .collect();
        assert_eq!(
            states,
            [
                (
                    "com.apple.runningboardd",
                    ServiceState::Running,
                    Some(214),
                    None
                ),
                (
                    "pw.cntrl.privd",
                    ServiceState::Exited,
                    None,
                    Some("exited 0")
                ),
                (
                    "com.example.broken",
                    ServiceState::Failed,
                    None,
                    Some("exited 1")
                ),
                (
                    "com.example.killed",
                    ServiceState::Failed,
                    None,
                    Some("killed by signal 9")
                ),
                ("com.apple.lskdd", ServiceState::Stopped, None, None),
            ]
        );
    }

    #[test]
    fn pairs_session_names_and_ids() {
        let text = "<dictionary> {\n  Name : azaek\n  SessionInfo : <array> {\n    0 : <dictionary> {\n      kCGSSessionOnConsoleKey : TRUE\n      kCGSSessionUserIDKey : 501\n      kCGSSessionUserNameKey : azaek\n    }\n    1 : <dictionary> {\n      kCGSSessionUserIDKey : 0\n      kCGSSessionUserNameKey : root\n    }\n  }\n  UID : 501\n}";
        assert_eq!(parse_sessions(text), [("azaek".to_owned(), 501)]);
        assert_eq!(parse_sessions(""), []);
    }

    #[test]
    fn names_apps_by_their_bundle_folder() {
        assert_eq!(
            app_name("/Applications/HEAD.app/Contents/MacOS/head").as_deref(),
            Some("HEAD")
        );
        assert_eq!(app_name("/usr/libexec/thing"), None);
    }

    #[test]
    fn lists_this_macs_system_domain() {
        let services = list().expect("launchctl print system");
        assert!(services.iter().any(|s| s.unit.starts_with("com.apple.")));
    }

    /// Lists this user's own session, which needs no root:
    /// `cargo test -p cntrl-host -- --ignored --nocapture` with a desktop session.
    #[test]
    #[ignore = "reads the user's GUI domain"]
    fn lists_my_session() {
        let uid = rustix::process::getuid().as_raw();
        let services = list_session("me", uid).expect("launchctl print gui");
        for app in services
            .iter()
            .filter(|s| s.kind == ServiceKind::App && !s.unit.starts_with("com.apple."))
        {
            println!("app {} {:?} {:?}", app.unit, app.description, app.pid);
        }
        assert!(services.iter().all(|s| s.scope == ServiceScope::User));
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
        let domain = system_domain();
        let plist_path = plist.to_string_lossy().into_owned();
        launchctl(&["bootstrap", &domain, &plist_path]).expect("bootstrap");
        let result = restart(&domain, label);
        let _ = launchctl(&["bootout", &format!("{domain}/{label}")]);
        assert_eq!(result, Ok(JobResult::Done));
        assert!(matches!(
            restart(&domain, "pw.cntrl.test.missing"),
            Err(HostError::NotFound(_))
        ));
    }
}
