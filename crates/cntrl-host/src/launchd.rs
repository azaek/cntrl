//! launchd jobs, behind the `service.*` operations on macOS: the system domain,
//! where LaunchDaemons live, and each logged-in user's GUI domain, with their
//! LaunchAgents and open apps. privd runs as root; run as another user, as in
//! tests, it acts on that user's GUI domain in place of the system's.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use cntrl_protocol::app::QuitResult;
use cntrl_protocol::service::{
    JobResult, ServiceAction, ServiceKind, ServiceScope, ServiceState, ServiceStatus,
};

use crate::HostError;

/// How long a restarted job gets to show it's running. launchd waits up to
/// 10 s (its default ThrottleInterval) to respawn a job that only just started.
const SETTLE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);
/// launchctl's exit status for a job that isn't loaded in the domain.
const NO_SUCH_JOB: i32 = 113;
/// Third-party daemons' plists. A daemon that was stopped (booted out) is
/// loaded again from here; Apple's own live in /System and are left alone.
const DAEMONS: &str = "/Library/LaunchDaemons";

/// Restarts `label` in `domain` with `launchctl kickstart -k`, which stops a
/// running instance first, then waits until launchd shows the job running, or
/// finished for a job that runs and exits. It blocks, so call it on a
/// blocking thread.
pub fn restart(domain: &str, label: &str) -> Result<JobResult, HostError> {
    let target = format!("{domain}/{label}");
    succeeded(&launchctl(&["kickstart", "-k", &target])?, label)?;
    settle(&target)
}

/// Takes `action` on `label` in `domain` (angle 11). Stopping unloads the job,
/// since launchd starts a kept-alive job again after a signal; starting loads
/// a stopped daemon from its plist first; enabling and disabling set launchd's
/// lasting override, then load or unload the job to match. It blocks.
pub fn act(domain: &str, label: &str, action: ServiceAction) -> Result<JobResult, HostError> {
    let target = format!("{domain}/{label}");
    let loaded = || launchctl(&["print", &target]).is_ok_and(|o| o.status.success());
    match action {
        ServiceAction::Restart => restart(domain, label),
        ServiceAction::Start => {
            if !loaded() {
                bootstrap(domain, label)?;
            }
            succeeded(&launchctl(&["kickstart", &target])?, label)?;
            settle(&target)
        }
        ServiceAction::Stop => {
            let output = launchctl(&["bootout", &target])?;
            // A job that isn't loaded is stopped already.
            if output.status.code() != Some(NO_SUCH_JOB) {
                succeeded(&output, label)?;
            }
            Ok(JobResult::Done)
        }
        ServiceAction::Enable => {
            succeeded(&launchctl(&["enable", &target])?, label)?;
            if !loaded() {
                bootstrap(domain, label)?;
            }
            Ok(JobResult::Done)
        }
        ServiceAction::Disable => {
            succeeded(&launchctl(&["disable", &target])?, label)?;
            let output = launchctl(&["bootout", &target])?;
            if output.status.code() != Some(NO_SUCH_JOB) {
                succeeded(&output, label)?;
            }
            Ok(JobResult::Done)
        }
    }
}

/// Loads a third-party daemon from its plist in `/Library/LaunchDaemons`.
fn bootstrap(domain: &str, label: &str) -> Result<(), HostError> {
    let plist = Path::new(DAEMONS).join(format!("{label}.plist"));
    if domain != "system" || !plist.exists() {
        return Err(HostError::NotFound(format!(
            "{label} isn't loaded, and there's no {} to load it from",
            plist.display()
        )));
    }
    succeeded(
        &launchctl(&["bootstrap", domain, &plist.to_string_lossy()])?,
        label,
    )
}

fn succeeded(output: &Output, label: &str) -> Result<(), HostError> {
    if output.status.success() {
        return Ok(());
    }
    Err(match output.status.code() {
        Some(NO_SUCH_JOB) => HostError::NotFound(format!("{label} isn't a launchd job here")),
        _ => HostError::Failed(message(output)),
    })
}

/// Waits until launchd shows the job running, or finished for a job that runs
/// and exits.
fn settle(target: &str) -> Result<JobResult, HostError> {
    let deadline = Instant::now() + SETTLE;
    loop {
        let printed = launchctl(&["print", target])?;
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
    let disabled = launchctl(&["print-disabled", "system"])
        .map(|output| overrides(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    let installed = third_party(Path::new(DAEMONS));
    let loaded: HashSet<String> = services.iter().map(|s| s.unit.clone()).collect();
    for service in &mut services {
        if installed.contains(&service.unit) {
            service.enabled = Some(!disabled.get(&service.unit).copied().unwrap_or(false));
        }
    }
    // A daemon that was stopped isn't in the domain any more, but its plist is.
    services.extend(
        installed
            .into_iter()
            .filter(|label| !loaded.contains(label))
            .map(|label| ServiceStatus {
                enabled: Some(!disabled.get(&label).copied().unwrap_or(false)),
                unit: label,
                description: None,
                state: ServiceState::Stopped,
                detail: Some("not loaded".to_owned()),
                pid: None,
                protected: false,
                scope: ServiceScope::System,
                user: None,
                kind: ServiceKind::Service,
            }),
    );
    services.sort_by(|a, b| a.unit.cmp(&b.unit));
    Ok(services)
}

/// `launchctl print-disabled`'s overrides: `"label" => disabled` (or `=>
/// true` on older macOS) for each job with one.
fn overrides(printed: &str) -> HashMap<String, bool> {
    printed
        .lines()
        .filter_map(|line| {
            let (label, value) = line.split_once("=>")?;
            let label = label.trim().trim_matches('"');
            let disabled = match value.trim() {
                "disabled" | "true" => true,
                "enabled" | "false" => false,
                _ => return None,
            };
            Some((label.to_owned(), disabled))
        })
        .collect()
}

/// The labels of the daemons installed in a directory, by their plists'
/// names, which by convention are their labels.
fn third_party(dir: &Path) -> HashSet<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    name.strip_suffix(".plist").map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
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
                enabled: None,
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

/// How long an app asked to quit gets to close, and one force-quit.
const QUIT_WAIT: Duration = Duration::from_secs(15);
const FORCE_WAIT: Duration = Duration::from_secs(5);

/// Quits every running copy of an app through AppKit, the way the Dock does,
/// or force-quits it the way Force Quit does. It talks to the app through
/// LaunchServices rather than scripting it, so it needs no Automation
/// permission. Arguments: the bundle ID, then `quit` or `force`.
const QUIT_SCRIPT: &str = r#"function run(argv) {
    ObjC.import("AppKit");
    var apps = $.NSRunningApplication.runningApplicationsWithBundleIdentifier(argv[0]);
    for (var i = 0; i < apps.count; i++) {
        var app = apps.objectAtIndex(i);
        if (argv[1] === "force") { app.forceTerminate; } else { app.terminate; }
    }
    return apps.count;
}"#;

/// Asks an app open in a user's session to quit, or force-quits it, then waits
/// for it to close. The asking happens in the user's session as that user,
/// which privd as root reaches through `launchctl asuser`; run as the user
/// itself, as in tests, it asks directly. It blocks.
pub fn quit_app(user: &str, uid: u32, bundle: &str, force: bool) -> Result<QuitResult, HostError> {
    if !app_open(uid, bundle)? {
        return Ok(QuitResult::NotOpen);
    }
    let how = if force { "force" } else { "quit" };
    let script = [
        "/usr/bin/osascript",
        "-l",
        "JavaScript",
        "-e",
        QUIT_SCRIPT,
        bundle,
        how,
    ];
    let uid_text = uid.to_string();
    let output = if rustix::process::getuid().is_root() {
        let as_user = ["asuser", &uid_text, "/usr/bin/sudo", "-u", user, "--"];
        Command::new("/bin/launchctl")
            .args(as_user)
            .args(script)
            .output()
    } else {
        Command::new(script[0]).args(&script[1..]).output()
    }
    .map_err(|e| HostError::Failed(format!("can't run osascript: {e}")))?;
    if !output.status.success() {
        return Err(HostError::Failed(message(&output)));
    }
    let deadline = Instant::now() + if force { FORCE_WAIT } else { QUIT_WAIT };
    while app_open(uid, bundle)? {
        if Instant::now() >= deadline {
            return Ok(QuitResult::StillOpen);
        }
        sleep(POLL);
    }
    Ok(QuitResult::Quit)
}

/// Whether the user's domain has a job for the app: launchd names each open
/// app `application.<bundle ID>.<n>.<n>`.
fn app_open(uid: u32, bundle: &str) -> Result<bool, HostError> {
    let output = launchctl(&["print", &user_domain(uid)])?;
    if !output.status.success() {
        return Err(HostError::Failed(message(&output)));
    }
    let prefix = format!("application.{bundle}.");
    Ok(services(&String::from_utf8_lossy(&output.stdout))
        .iter()
        .any(|service| {
            service.unit.strip_prefix(&prefix).is_some_and(|rest| {
                !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
            })
        }))
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
    fn reads_launchds_overrides() {
        let printed = "\tdisabled services = {\n\t\t\"com.apple.ftpd\" => disabled\n\t\t\"com.example.web\" => enabled\n\t\t\"com.old.style\" => true\n\t}\n";
        let overrides = overrides(printed);
        assert_eq!(overrides.get("com.apple.ftpd"), Some(&true));
        assert_eq!(overrides.get("com.example.web"), Some(&false));
        assert_eq!(overrides.get("com.old.style"), Some(&true));
        assert_eq!(overrides.len(), 3);
    }

    #[test]
    fn lists_installed_daemons_by_their_plists() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("pw.cntrl.agent.plist"), "").expect("write");
        std::fs::write(dir.path().join("com.example.web.plist"), "").expect("write");
        std::fs::write(dir.path().join("README"), "").expect("write");
        let mut labels: Vec<String> = third_party(dir.path()).into_iter().collect();
        labels.sort();
        assert_eq!(labels, ["com.example.web", "pw.cntrl.agent"]);
        assert!(third_party(Path::new("/nonexistent")).is_empty());
    }

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

    /// Quits an app in this user's session: Calculator, opened hidden for it.
    /// `cargo test -p cntrl-host -- --ignored` with a desktop session.
    #[test]
    #[ignore = "opens and quits Calculator in the user's session"]
    fn quits_an_app() {
        let uid = rustix::process::getuid().as_raw();
        let bundle = "com.apple.calculator";
        let opened = Command::new("/usr/bin/open")
            .args(["-g", "-j", "-b", bundle])
            .status();
        assert!(opened.is_ok_and(|status| status.success()));
        sleep(Duration::from_secs(2));
        assert_eq!(quit_app("me", uid, bundle, false), Ok(QuitResult::Quit));
        assert_eq!(quit_app("me", uid, bundle, false), Ok(QuitResult::NotOpen));
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
