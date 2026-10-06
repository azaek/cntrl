//! Installing on Windows (D58). `cntrl-agent.exe install`, which install.ps1
//! runs from an elevated PowerShell and `cntrl update` runs from a release,
//! puts the agent under Program Files, gives `%ProgramData%\cntrl` access
//! lists that only its services and administrators get through, writes the
//! config when there's none, and registers and starts the two services.
//! `cntrl uninstall` takes them away, and with `--purge` the data too.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use windows_service::service::{
    Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceDependency,
    ServiceErrorControl, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo,
    ServiceSidType, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::config::{self, Config};
use super::os::{self, setup};
use super::say::{say, say_err};

const AGENT: &str = "cntrl-agent";
const PRIVD: &str = "cntrl-privd";
/// Tailscale's restart delays after a failure, in seconds: "(mostly)
/// squares", the last repeating (angle 14).
const RESTARTS: [u64; 9] = [1, 2, 4, 9, 16, 25, 36, 49, 64];
/// A minute without failing forgets the failures before.
const FAILURES_RESET: Duration = Duration::from_secs(60);
/// How long a service gets to start or stop.
const SERVICE_WAIT: Duration = Duration::from_secs(30);
/// SYSTEM and Administrators in full, in a list protected from its parent's,
/// handed down to what's inside.
const ADMINS: &str = "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
/// Reading and running, for the agent's account.
const READS: u32 = 0x0012_00a9;
/// Reading, writing and deleting, for the agent's account.
const MODIFIES: u32 = 0x0013_01bf;

/// `cntrl-agent install`: this binary as the installed agent, or the
/// installed one again.
pub fn install(config_path: &Path, console: Option<String>, gateway: Option<String>) -> ExitCode {
    match try_install(config_path, console, gateway) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            say_err!("cntrl-agent install: {e}");
            ExitCode::FAILURE
        }
    }
}

fn try_install(
    config_path: &Path,
    console: Option<String>,
    gateway: Option<String>,
) -> Result<(), String> {
    if !os::is_root() {
        return Err(format!(
            "installing needs {}; run it {}",
            os::SUPERUSER,
            os::AS_ROOT
        ));
    }
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| format!("can't reach the Service Control Manager: {}", why(&e)))?;

    // Both halves stop, so their binary can be replaced, with their restarts
    // held off meanwhile, so neither comes back before the new one is in
    // place; `configure` sets them again.
    for name in [AGENT, PRIVD] {
        hold_restarts(&manager, name);
    }
    stop(&manager, AGENT)?;
    stop(&manager, PRIVD)?;
    let programs = program_dir();
    std::fs::create_dir_all(&programs)
        .map_err(|e| format!("can't create {}: {e}", programs.display()))?;
    clear_aside(&programs);
    let bin = programs.join("cntrl-agent.exe");
    let this = std::env::current_exe().map_err(|e| format!("can't find this program: {e}"))?;
    if !same_file(&this, &bin) {
        put(&this, &bin)?;
    }
    put(&bin, &programs.join("cntrl.exe"))?;

    write_config(config_path, console, gateway)?;
    let config = Config::load(config_path).map_err(|e| e.to_string())?;

    register(&manager, &bin)?;
    let agent = os::account(os::AGENT_ACCOUNT)
        .ok_or_else(|| format!("Windows doesn't know {} yet", os::AGENT_ACCOUNT))?;
    let data = config_path
        .parent()
        .ok_or_else(|| format!("{} has no folder", config_path.display()))?;
    let reads = format!("{ADMINS}(A;OICI;{READS:#x};;;{agent})");
    let modifies = format!("{ADMINS}(A;OICI;{MODIFIES:#x};;;{agent})");
    let folders = [
        (data, reads.as_str()),
        (config.paths.state_dir.as_path(), modifies.as_str()),
        (config.paths.logs.as_path(), modifies.as_str()),
        (config.paths.privd_state_dir.as_path(), ADMINS),
        (config.paths.audit_dir.as_path(), ADMINS),
    ];
    for (folder, sddl) in folders {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("can't create {}: {e}", folder.display()))?;
        setup::set_access(folder, sddl)?;
    }
    setup::set_on_path(&programs, true)?;

    start(&manager, PRIVD)?;
    start(&manager, AGENT)?;
    say!(
        "Installed cntrl agent {} as {}, also called cntrl.",
        env!("CARGO_PKG_VERSION"),
        bin.display()
    );
    Ok(())
}

/// `cntrl uninstall`: the services and the program go; with `purge`, the
/// identity, config, audit log and logs too, and the device stays in Console
/// until someone removes it there.
pub fn uninstall(config: &Config, config_path: &Path, purge: bool) -> ExitCode {
    match try_uninstall(config, config_path, purge) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            say_err!("cntrl uninstall: {e}");
            ExitCode::FAILURE
        }
    }
}

fn try_uninstall(config: &Config, config_path: &Path, purge: bool) -> Result<(), String> {
    if !os::is_root() {
        return Err(format!(
            "uninstalling needs {}; run it {}",
            os::SUPERUSER,
            os::AS_ROOT
        ));
    }
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| format!("can't reach the Service Control Manager: {}", why(&e)))?;
    for name in [AGENT, PRIVD] {
        hold_restarts(&manager, name);
    }
    for name in [AGENT, PRIVD] {
        stop(&manager, name)?;
        if let Ok(service) = manager.open_service(name, ServiceAccess::DELETE) {
            service
                .delete()
                .map_err(|e| format!("can't remove the {name} service: {}", why(&e)))?;
        }
    }
    let programs = program_dir();
    setup::set_on_path(&programs, false)?;
    // A running program, as this one may be, goes when Windows restarts.
    let mut later = false;
    if let Ok(entries) = std::fs::read_dir(&programs) {
        for entry in entries.flatten() {
            let path = entry.path();
            if std::fs::remove_file(&path).is_err() {
                setup::delete_at_restart(&path)?;
                later = true;
            }
        }
    }
    if std::fs::remove_dir(&programs).is_err() && programs.exists() {
        setup::delete_at_restart(&programs)?;
        later = true;
    }
    if purge {
        let data = config_path.parent().unwrap_or(config_path);
        for path in [data, config.paths.logs.as_path()] {
            match std::fs::remove_dir_all(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("can't remove {}: {e}", path.display())),
            }
        }
        say!("Removed the agent, its identity, config, audit log and logs.");
    } else {
        say!(
            "Removed the agent. Its identity, config, audit log and logs stay in {}; --purge removes them too.",
            config_path.parent().unwrap_or(config_path).display()
        );
    }
    if later {
        say!("The last of its files go when Windows next restarts.");
    }
    Ok(())
}

/// `%ProgramFiles%\cntrl`, where only administrators write.
fn program_dir() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from)
        .join("cntrl")
}

/// Registers both services, or brings their settings up to date: privd as
/// LocalSystem, the agent as its virtual account, which needs privd running.
fn register(manager: &ServiceManager, bin: &Path) -> Result<(), String> {
    let service = |name: &str, display: &str, argument: &str| ServiceInfo {
        name: name.into(),
        display_name: display.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: bin.to_owned(),
        launch_arguments: vec![OsString::from(argument)],
        dependencies: Vec::new(),
        account_name: None,
        account_password: None,
    };
    let privd = service(PRIVD, "cntrl privileged helper", "privd");
    let privd = configure(manager, &privd)?;
    privd
        .set_description("The cntrl agent's half that acts on the machine and keeps its audit log.")
        .map_err(|e| why(&e))?;
    setup::require_privileges(&privd, &["SeShutdownPrivilege", "SeChangeNotifyPrivilege"])?;

    let mut agent = service(AGENT, "cntrl agent", "run");
    agent.account_name = Some(os::AGENT_ACCOUNT.into());
    agent.dependencies = vec![ServiceDependency::Service(PRIVD.into())];
    let agent = configure(manager, &agent)?;
    agent
        .set_description("Connects this machine to Console, within its device policy.")
        .map_err(|e| why(&e))?;
    agent
        .set_config_service_sid_info(ServiceSidType::Unrestricted)
        .map_err(|e| format!("can't set the agent's SID: {}", why(&e)))?;
    setup::require_privileges(&agent, &["SeChangeNotifyPrivilege"])?;
    Ok(())
}

/// Creates the service, or changes its settings to these, with restarts after
/// failures on Tailscale's backoff, nonzero exits counting.
fn configure(manager: &ServiceManager, info: &ServiceInfo) -> Result<Service, String> {
    let name = info.name.to_string_lossy().into_owned();
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::QUERY_CONFIG
        | ServiceAccess::CHANGE_CONFIG;
    let service = match manager.open_service(&info.name, access) {
        Ok(service) => {
            service
                .change_config(info)
                .map_err(|e| format!("can't update the {name} service: {}", why(&e)))?;
            service
        }
        Err(_) => manager
            .create_service(info, access)
            .map_err(|e| format!("can't create the {name} service: {}", why(&e)))?,
    };
    let actions = RESTARTS
        .iter()
        .map(|&seconds| ServiceAction {
            action_type: ServiceActionType::Restart,
            delay: Duration::from_secs(seconds),
        })
        .collect();
    service
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(FAILURES_RESET),
            reboot_msg: None,
            command: None,
            actions: Some(actions),
        })
        .map_err(|e| format!("can't set {name}'s restarts: {}", why(&e)))?;
    service
        .set_failure_actions_on_non_crash_failures(true)
        .map_err(|e| format!("can't set {name}'s restarts: {}", why(&e)))?;
    Ok(service)
}

/// Clears a service's restarts after failures, so ending it keeps it
/// stopped; one that isn't there has none.
fn hold_restarts(manager: &ServiceManager, name: &str) {
    if let Ok(service) = manager.open_service(name, ServiceAccess::CHANGE_CONFIG) {
        let _ = service.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(FAILURES_RESET),
            reboot_msg: None,
            command: None,
            actions: Some(Vec::new()),
        });
    }
}

/// Stops a service and waits for it; one that isn't there is stopped. One
/// that doesn't stop in time has its process ended.
fn stop(manager: &ServiceManager, name: &str) -> Result<(), String> {
    let Ok(service) = manager.open_service(name, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP)
    else {
        return Ok(());
    };
    let status = service
        .query_status()
        .map_err(|e| format!("can't ask about {name}: {}", why(&e)))?;
    if status.current_state == ServiceState::Stopped {
        return Ok(());
    }
    // It may be stopping already, which refuses another stop.
    let _ = service.stop();
    if wait_for(&service, name, ServiceState::Stopped).is_ok() {
        return Ok(());
    }
    let pid = setup::service_process(&service)
        .ok_or_else(|| format!("{name} didn't stop, and its process can't be found"))?;
    say!("{name} didn't stop within {SERVICE_WAIT:?}, so its process is ended.");
    setup::end_process(pid)?;
    wait_for(&service, name, ServiceState::Stopped)
}

fn start(manager: &ServiceManager, name: &str) -> Result<(), String> {
    let service = manager
        .open_service(name, ServiceAccess::QUERY_STATUS | ServiceAccess::START)
        .map_err(|e| format!("can't open the {name} service: {}", why(&e)))?;
    service
        .start::<&str>(&[])
        .map_err(|e| format!("{name} didn't start: {}", why(&e)))?;
    wait_for(&service, name, ServiceState::Running)
}

fn wait_for(service: &Service, name: &str, state: ServiceState) -> Result<(), String> {
    let deadline = Instant::now() + SERVICE_WAIT;
    loop {
        let now = service
            .query_status()
            .map_err(|e| format!("can't ask about {name}: {}", why(&e)))?
            .current_state;
        if now == state {
            return Ok(());
        }
        if state == ServiceState::Running && now == ServiceState::Stopped {
            return Err(format!(
                "{name} stopped as it started; its log is in {}",
                Config::default().paths.logs.display()
            ));
        }
        if Instant::now() > deadline {
            return Err(format!("{name} isn't {state:?} after {SERVICE_WAIT:?}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The config, when there's none yet: where enrollment goes, and a gateway in
/// place of the one enrollment returns.
fn write_config(
    path: &Path,
    console: Option<String>,
    gateway: Option<String>,
) -> Result<(), String> {
    if path.exists() {
        say!("Keeping {}", path.display());
        return Ok(());
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("can't create {}: {e}", folder.display()))?;
    }
    let console = console.unwrap_or_else(|| config::ConsoleConfig::default().url);
    let mut text = format!(
        "# The cntrl agent's config; `cntrl config check` validates it.\n[console]\nurl = {}\n",
        toml::Value::String(console)
    );
    if let Some(gateway) = gateway {
        text.push_str(&format!("gateway_url = {}\n", toml::Value::String(gateway)));
    }
    std::fs::write(path, text).map_err(|e| format!("can't write {}: {e}", path.display()))
}

/// Copies `from` over `to`, which may be running: Windows won't replace a
/// running program, but lets it be renamed, so it's moved aside for the next
/// install to delete.
fn put(from: &Path, to: &Path) -> Result<(), String> {
    let staged = to.with_extension("exe.new");
    std::fs::copy(from, &staged).map_err(|e| format!("can't copy to {}: {e}", to.display()))?;
    if std::fs::rename(&staged, to).is_ok() {
        return Ok(());
    }
    let aside = to.with_extension(format!("exe.old-{}", std::process::id()));
    std::fs::rename(to, &aside).map_err(|e| format!("can't replace {}: {e}", to.display()))?;
    std::fs::rename(&staged, to).map_err(|e| format!("can't replace {}: {e}", to.display()))
}

/// Deletes the programs earlier installs moved aside, where they've stopped.
fn clear_aside(programs: &Path) {
    let Ok(entries) = std::fs::read_dir(programs) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().contains(".exe.old-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// What a windows-service error says: its call's error, not "IO error in
/// winapi call".
fn why(e: &windows_service::Error) -> String {
    std::error::Error::source(e).map_or_else(|| e.to_string(), ToString::to_string)
}
