//! Updating the agent from Console (D145). `agent.update` names a release;
//! the agent fetches it and checks it as `cntrl update` does, then asks
//! privd, which checks its own copy again, sets aside the agent installed
//! now, and starts the update as a job of its own: the installer stops both
//! services, so the update can't run inside either. The job installs the
//! release, waits for the new agent to come back connected, and when it
//! doesn't within two minutes, puts the old one back with its own installer.
//! Console can only pick among genuine releases newer than the agent's own.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use super::client::{self, block_on};
use super::config::Config;
use super::ipc::{self, Call};
use super::update::{self, ARCHIVE, Artifact, Manifest};
use super::uplink::UplinkStatus;

/// The release's signed manifest and its signature, as saved beside its archive.
const MANIFEST: &str = "manifest.json";
const SIGNATURE: &str = "manifest.json.sig";
/// Where privd keeps the agent installed now, to put it back.
const PREVIOUS: &str = "previous";
/// Written when an update starts, so a second waits for the first.
const STARTED: &str = "started";
/// How long an update counts as running; past it, a crashed one doesn't hold the next back.
const RUNNING_FOR: Duration = Duration::from_secs(20 * 60);
/// How long the new agent has to come back connected before the old one is put back.
const BACK_WITHIN: Duration = Duration::from_secs(120);
/// The job's file in the log directory.
const LOG: &str = "update.log";
/// Where the job writes its lines, once it knows.
static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();

#[cfg(unix)]
const AGENT: &str = "cntrl-agent";
#[cfg(windows)]
const AGENT: &str = "cntrl-agent.exe";

/// A release's version as Console may name it: letters, digits, dots,
/// dashes and pluses, short, since it goes into a URL and a directory.
pub fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 40
        && version.starts_with(|c: char| c.is_ascii_digit())
        && !version.contains("..")
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
}

/// The agent's half: downloads release `version` into `dir` and checks its
/// signature and its archive, which privd checks again from its own copy.
pub async fn download(version: &str, dir: &Path) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("cntrl-agent/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(120))
        .connect_timeout(update::CONNECT_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let base = format!("{}/agent-v{version}", update::RELEASES);
    let manifest = update::fetch(
        &client,
        &format!("{base}/{MANIFEST}"),
        update::MAX_ARCHIVE,
        None,
    )
    .await
    .map_err(|e| format!("can't fetch release {version}: {e}"))?;
    let signature = update::fetch(&client, &format!("{base}/{SIGNATURE}"), 1024, None).await?;
    let artifact = checked(&manifest, &signature, version)?;
    let archive = update::fetch(
        &client,
        &artifact.url,
        update::MAX_ARCHIVE.min(artifact.size),
        None,
    )
    .await?;
    update::matches(&archive, &artifact)?;
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| format!("can't clear {}: {e}", dir.display()))?;
    }
    fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    for (name, bytes) in [
        (MANIFEST, &manifest),
        (SIGNATURE, &signature),
        (ARCHIVE, &archive),
    ] {
        fs::write(dir.join(name), bytes).map_err(|e| format!("can't save the download: {e}"))?;
    }
    Ok(())
}

/// Checks a manifest's signature and version, and finds this machine's build in it.
fn checked(manifest: &[u8], signature: &[u8], version: &str) -> Result<Artifact, String> {
    update::verify(manifest, signature)?;
    let parsed: Manifest = serde_json::from_slice(manifest)
        .map_err(|e| format!("the release's manifest doesn't read: {e}"))?;
    if parsed.version != version {
        return Err(format!(
            "the manifest for {version} says it's {}",
            parsed.version
        ));
    }
    let target = update::release_target()?;
    parsed
        .artifacts
        .get(&target)
        .cloned()
        .ok_or_else(|| format!("release {version} has no build for {target}"))
}

/// Where privd works on an update: in its own directory, which only root reaches.
pub fn work_dir(privd_state: &Path) -> PathBuf {
    privd_state.join("update")
}

/// privd's half: copies the release the agent fetched into `work`, checks
/// it again there, sets aside the agent installed now, and starts the job.
pub fn start(work: &Path, fetched: &Path, from: &str, to: &str, logs: &Path) -> Result<(), String> {
    if running(work) {
        return Err("an update is already running".to_owned());
    }
    if work.exists() {
        fs::remove_dir_all(work).map_err(|e| format!("can't clear {}: {e}", work.display()))?;
    }
    fs::create_dir_all(work).map_err(|e| format!("can't make {}: {e}", work.display()))?;
    for name in [MANIFEST, SIGNATURE, ARCHIVE] {
        let source = fetched.join(name);
        // Only the files the agent saved: never a link to something else of root's.
        let meta = fs::symlink_metadata(&source).map_err(|e| format!("{name}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("{name} isn't a file"));
        }
        fs::copy(&source, work.join(name)).map_err(|e| format!("can't copy {name}: {e}"))?;
    }
    let read = |name: &str| fs::read(work.join(name)).map_err(|e| format!("{name}: {e}"));
    let artifact = checked(&read(MANIFEST)?, &read(SIGNATURE)?, to)?;
    update::matches(&read(ARCHIVE)?, &artifact)?;
    set_aside(work)?;
    fs::write(work.join(STARTED), from).map_err(|e| e.to_string())?;
    launch(work, from, to, logs)
}

/// Whether an update started here lately is still going.
fn running(work: &Path) -> bool {
    fs::metadata(work.join(STARTED))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| SystemTime::now().duration_since(at).ok())
        .is_some_and(|age| age < RUNNING_FOR)
}

/// Keeps what it takes to put this agent back: its binary, which also runs
/// the job, and on Linux and macOS its installer and the service files
/// installed now, laid out as the installer's `--binary` expects.
fn set_aside(work: &Path) -> Result<(), String> {
    let previous = work.join(PREVIOUS);
    let packaging = previous.join("packaging");
    fs::create_dir_all(&packaging).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| format!("can't find this agent: {e}"))?;
    fs::copy(&exe, previous.join(AGENT)).map_err(|e| format!("can't keep this agent: {e}"))?;
    #[cfg(unix)]
    fs::write(packaging.join("install.sh"), update::INSTALLER).map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    keep(
        Path::new("/etc/systemd/system"),
        &packaging.join("systemd"),
        &[
            "cntrl-agent.service",
            "cntrl-privd.service",
            "cntrl-privd.socket",
        ],
    )?;
    #[cfg(target_os = "macos")]
    keep(
        Path::new("/Library/LaunchDaemons"),
        &packaging.join("macos"),
        &["pw.cntrl.agent.plist", "pw.cntrl.privd.plist"],
    )?;
    Ok(())
}

/// Copies the service files installed now.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn keep(from: &Path, to: &Path, names: &[&str]) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for name in names {
        fs::copy(from.join(name), to.join(name)).map_err(|e| format!("can't keep {name}: {e}"))?;
    }
    Ok(())
}

/// The job's command line: the agent set aside runs it, so going back
/// doesn't depend on the release being installed.
fn job(work: &Path, from: &str, to: &str) -> (PathBuf, Vec<String>) {
    let exe = work.join(PREVIOUS).join(AGENT);
    let args = vec![
        "update-apply".to_owned(),
        "--work".to_owned(),
        work.display().to_string(),
        "--from".to_owned(),
        from.to_owned(),
        "--to".to_owned(),
        to.to_owned(),
    ];
    (exe, args)
}

/// On Linux, a transient unit: outside both services' units, and outside
/// privd's sandbox, which can't write where the agent is installed. The job
/// writes its own lines to `update.log`; the installers' go to the journal
/// (`journalctl -u cntrl-update`), since appending a unit's output to a file
/// needs systemd 240.
#[cfg(target_os = "linux")]
fn launch(work: &Path, from: &str, to: &str, _logs: &Path) -> Result<(), String> {
    let (exe, args) = job(work, from, to);
    let status = Command::new("systemd-run")
        .args([
            "--unit",
            "cntrl-update",
            "--collect",
            "--no-block",
            "--quiet",
        ])
        .arg("--description=cntrl agent update")
        .arg(&exe)
        .args(&args)
        .status()
        .map_err(|e| format!("can't run systemd-run: {e}"))?;
    if !status.success() {
        return Err(format!("systemd-run couldn't start the update ({status})"));
    }
    Ok(())
}

/// On macOS, a launchd job of its own, loaded from a plist in privd's
/// directory, so it isn't there again after a restart.
#[cfg(target_os = "macos")]
fn launch(work: &Path, from: &str, to: &str, logs: &Path) -> Result<(), String> {
    const LABEL: &str = "pw.cntrl.update";
    let (exe, args) = job(work, from, to);
    let log = logs.join(LOG);
    let strings: String = std::iter::once(exe.display().to_string())
        .chain(args)
        .map(|arg| format!("        <string>{}</string>\n", xml(&arg)))
        .collect();
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20   <key>Label</key>\n    <string>{LABEL}</string>\n\
         \x20   <key>ProgramArguments</key>\n    <array>\n{strings}    </array>\n\
         \x20   <key>RunAtLoad</key>\n    <true/>\n\
         \x20   <key>AbandonProcessGroup</key>\n    <true/>\n\
         \x20   <key>StandardOutPath</key>\n    <string>{log}</string>\n\
         \x20   <key>StandardErrorPath</key>\n    <string>{log}</string>\n\
         </dict>\n</plist>\n",
        log = xml(&log.display().to_string()),
    );
    let path = work.join(format!("{LABEL}.plist"));
    fs::write(&path, plist).map_err(|e| format!("can't write the update's job: {e}"))?;
    // One left from an update before is unloaded first; none is fine.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("system/{LABEL}")])
        .output();
    let status = Command::new("launchctl")
        .args(["bootstrap", "system"])
        .arg(&path)
        .status()
        .map_err(|e| format!("can't run launchctl: {e}"))?;
    if !status.success() {
        return Err(format!("launchctl couldn't start the update ({status})"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// On Windows, a process of its own: the service manager doesn't end a
/// service's children when it stops it.
#[cfg(windows)]
fn launch(work: &Path, from: &str, to: &str, logs: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let (exe, args) = job(work, from, to);
    let log = || {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(logs.join(LOG))
            .map_or_else(|_| std::process::Stdio::null(), std::process::Stdio::from)
    };
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
    let spawn = |flags: u32| {
        Command::new(&exe)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(log())
            .stderr(log())
            .creation_flags(flags)
            .spawn()
    };
    // Out of privd's job object where there is one; where it can't break away, as it is.
    spawn(flags | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| spawn(flags))
        .map(drop)
        .map_err(|e| format!("can't start the update: {e}"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn launch(_work: &Path, _from: &str, _to: &str, _logs: &Path) -> Result<(), String> {
    Err("updating from Console isn't supported here".to_owned())
}

/// The job (`cntrl-agent update-apply`): installs the release, waits for
/// the new agent to come back connected, and puts the old one back when it
/// doesn't. Its lines and the installers' go to `update.log`.
pub fn apply(config: &Config, work: &Path, from: &str, to: &str) -> ExitCode {
    let _ = LOG_FILE.set(config.paths.logs.join(LOG));
    line(&format!("updating the agent from {from} to {to}"));
    // privd's answer reaches Console before the installer stops the agent.
    std::thread::sleep(Duration::from_secs(2));
    let installed = match update::install(work, &work.join(ARCHIVE), false) {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("the installer for {to} failed ({status})")),
        Err(e) => Err(e),
    };
    let failure = match installed {
        Ok(()) if back_on(config, to) => {
            line(&format!("{to} is back connected"));
            record(config, "agent.updated", json!({ "from": from, "to": to }));
            finish(work);
            return ExitCode::SUCCESS;
        }
        Ok(()) => format!(
            "{to} didn't come back connected within {}s",
            BACK_WITHIN.as_secs()
        ),
        Err(e) => e,
    };
    line(&format!("{failure}; putting {from} back"));
    let restored = match restore(work) {
        Ok(()) => back_on(config, from),
        Err(e) => {
            line(&format!("couldn't put {from} back: {e}"));
            false
        }
    };
    line(if restored {
        "the agent is back on its old version"
    } else {
        "the old version didn't come back either; see this log"
    });
    record(
        config,
        "agent.update_failed",
        json!({ "from": from, "to": to, "reason": failure, "restored": restored }),
    );
    finish(work);
    ExitCode::FAILURE
}

/// Puts the agent set aside back, with its own installer.
#[cfg(unix)]
fn restore(work: &Path) -> Result<(), String> {
    let previous = work.join(PREVIOUS);
    let status = Command::new("/bin/sh")
        .arg(previous.join("packaging").join("install.sh"))
        .arg("--binary")
        .arg(previous.join(AGENT))
        .status()
        .map_err(|e| format!("can't run the installer: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("its installer failed ({status})"))
    }
}

/// Puts the agent set aside back: on Windows the agent installs itself.
#[cfg(windows)]
fn restore(work: &Path) -> Result<(), String> {
    let status = Command::new(work.join(PREVIOUS).join(AGENT))
        .arg("install")
        .status()
        .map_err(|e| format!("can't run the installer: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("its installer failed ({status})"))
    }
}

/// Whether the agent comes back on `version` and connected within BACK_WITHIN:
/// online, or on the quiet link of a machine its plan turned off.
fn back_on(config: &Config, version: &str) -> bool {
    let until = Instant::now() + BACK_WITHIN;
    while Instant::now() < until {
        if let Ok(status) = block_on(client::get_status(&config.paths.agent_socket))
            && status.version == version
            && matches!(
                status.uplink,
                UplinkStatus::Online { .. } | UplinkStatus::Disabled { .. }
            )
        {
            return true;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    false
}

/// Puts the outcome in the audit log, through privd.
fn record(config: &Config, kind: &str, data: Value) {
    let call = Call::AuditAppend {
        kind: kind.to_owned(),
        data,
    };
    if let Err(e) = block_on(ipc::call_once(&config.paths.privd_socket, call)) {
        line(&format!("couldn't audit the outcome: {e}"));
    }
}

/// Clears the work, the release and the agent set aside with it.
fn finish(work: &Path) {
    if let Err(e) = fs::remove_dir_all(work) {
        line(&format!("couldn't clear {}: {e}", work.display()));
    }
}

/// One line of the job's log, with when: in `update.log`, or where it can't
/// be opened, on standard output.
fn line(text: &str) {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let text = format!("{} {text}\n", utc(seconds));
    let file = LOG_FILE.get().and_then(|path| {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    match file {
        Some(mut file) => drop(file.write_all(text.as_bytes())),
        None => drop(std::io::stdout().write_all(text.as_bytes())),
    }
}

/// Seconds since 1970 as `2026-11-03T08:15:00Z`: Howard Hinnant's days-to-civil.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_a_version_and_nothing_else() {
        assert!(valid_version("0.1.22"));
        assert!(valid_version("0.2.0-beta.1"));
        assert!(!valid_version(""));
        assert!(!valid_version("../0.1.22"));
        assert!(!valid_version("0.1.22/../../etc"));
        assert!(!valid_version("v0.1.22"));
        assert!(!valid_version("0.1.22 && reboot"));
        assert!(!valid_version(&"1".repeat(41)));
    }

    #[test]
    fn a_running_update_holds_the_next_back() {
        let dir = tempfile::tempdir().expect("a directory");
        assert!(!running(dir.path()));
        fs::write(dir.path().join(STARTED), "0.1.21").expect("write");
        assert!(running(dir.path()));
    }

    #[test]
    fn the_log_says_when_in_utc() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_793_644_249), "2026-11-02T18:30:49Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn the_job_runs_the_agent_set_aside() {
        let (exe, args) = job(Path::new("/var/lib/cntrl-privd/update"), "0.1.21", "0.1.22");
        assert_eq!(
            exe,
            Path::new("/var/lib/cntrl-privd/update/previous").join(AGENT)
        );
        assert_eq!(args[0], "update-apply");
        assert_eq!(args[args.len() - 1], "0.1.22");
    }
}
