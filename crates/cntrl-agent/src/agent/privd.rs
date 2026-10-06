//! privd, the privileged half of the agent. It runs as root with no network,
//! started on demand by its systemd or launchd socket, or on Windows as a
//! LocalSystem service that keeps running (D58), and is the only part that
//! reads the policy, writes the audit log, holds the audit key and acts on the
//! machine. It accepts connections only from root, the agent's user (`cntrl`,
//! `_cntrl` on macOS, `NT SERVICE\cntrl-agent` on Windows) and its own user,
//! and exits after a minute without one.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cntrl_host::HostError;
use cntrl_host::services::service_name;
use cntrl_protocol::app::{AppQuitResult, QuitResult};
use cntrl_protocol::codes::ErrorCode;
use cntrl_protocol::containers::ContainerJob;
use cntrl_protocol::frame::Actor;
use cntrl_protocol::logs::LogsParams;
use cntrl_protocol::power::{PowerAction, PowerStarted};
use cntrl_protocol::process::ProcessSignalResult;
use cntrl_protocol::service::{JobResult, ServiceAction, ServiceJob, ServiceScope, ServiceStatus};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::audit::AuditLog;
use super::config::Config;
use super::docker;
use super::ipc::{self, Call, CallError, Request, Response};
use super::keys::SigningKey;
use super::os::{self, Private};
use super::{logging, policy};

const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Whether privd exits when idle: systemd and launchd start it again when the
/// agent next connects, and Windows' Service Control Manager doesn't.
const EXITS_WHEN_IDLE: bool = cfg!(unix);
const IDLE_CHECK: Duration = Duration::from_secs(5);
const AUDIT_KEY_FILE: &str = "audit.key";
/// How long privd waits for systemd's verdict on a job.
const JOB_LIMIT: Duration = Duration::from_secs(300);

struct State {
    audit: Mutex<AuditLog>,
    policy_path: PathBuf,
    audit_key_path: PathBuf,
    /// The user privd runs as; the policy file must belong to it.
    owner: os::Owner,
    /// Who may connect besides root: privd's own user and the agent's.
    allowed: Vec<os::Account>,
    /// The agent's user, whose processes privd won't stop.
    agent: Option<os::Account>,
    /// The process table's reader, made on first use; on macOS only privd
    /// sees every process (D24).
    processes: Mutex<Option<cntrl_host::processes::Sampler>>,
}

/// privd; `stop` is the service's token, which the service manager may
/// cancel.
pub fn main(config: &Config, stop: CancellationToken) -> ExitCode {
    logging::init(&config.log.level, &config.paths.logs, "privd");
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            error!("can't start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve(config, stop)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(config: &Config, stop: CancellationToken) -> Result<(), String> {
    let mut listener = os::LocalListener::listen(&config.paths.privd_socket, os::Endpoint::Privd)?;
    let audit = AuditLog::open(&config.paths.audit_dir)?;
    let state_dir = &config.paths.privd_state_dir;
    fs::DirBuilder::new()
        .recursive(true)
        .private()
        .create(state_dir)
        .map_err(|e| format!("can't create {}: {e}", state_dir.display()))?;
    let owner = os::own_owner();
    let agent = os::account(os::AGENT_ACCOUNT);
    let allowed = os::own_account()
        .into_iter()
        .chain(agent.iter().cloned())
        .collect();
    let state = Arc::new(State {
        audit: Mutex::new(audit),
        policy_path: config.paths.policy.clone(),
        audit_key_path: state_dir.join(AUDIT_KEY_FILE),
        owner,
        allowed,
        agent,
        processes: Mutex::new(None),
    });
    let activity = Arc::new(Activity::new());
    info!("privd started");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = accepted.map_err(|e| format!("accept failed: {e}"))?;
                if !peer_allowed(&peer, &state.allowed) {
                    continue;
                }
                let (state, activity) = (Arc::clone(&state), Arc::clone(&activity));
                activity.start();
                tokio::spawn(async move {
                    handle(stream, &state).await;
                    activity.finish();
                });
            }
            () = stop.cancelled() => {
                info!("asked to stop, exiting");
                return Ok(());
            }
            () = tokio::time::sleep(IDLE_CHECK) => {
                if EXITS_WHEN_IDLE && activity.idle_for(IDLE_TIMEOUT) {
                    info!("idle for {IDLE_TIMEOUT:?}, exiting");
                    return Ok(());
                }
            }
        }
    }
}

fn peer_allowed(peer: &os::Peer, allowed: &[os::Account]) -> bool {
    if peer.is_root() || allowed.iter().any(|account| peer.is(account)) {
        return true;
    }
    warn!(?peer, "refused a connection from a user that isn't allowed");
    false
}

async fn handle(stream: os::ServerStream, state: &Arc<State>) {
    let mut channel = ipc::channel(stream);
    loop {
        let request: Request = match ipc::receive(&mut channel).await {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(e) => {
                warn!("dropping a connection: {e}");
                return;
            }
        };
        // A log keeps the connection to itself until the agent hangs up.
        if let Call::LogStream { params } = request.call {
            return stream_logs(state, request.id, params, channel).await;
        }
        let response = Response::from_result(request.id, respond(state, request.call).await);
        if let Err(e) = ipc::send(&mut channel, &response).await {
            warn!("couldn't answer: {e}");
            return;
        }
    }
}

async fn respond(state: &Arc<State>, call: Call) -> Result<Value, CallError> {
    let state = Arc::clone(state);
    match call {
        Call::Ping => Ok(json!({ "version": env!("CARGO_PKG_VERSION") })),
        // `handle` gives a log its connection before it gets here.
        Call::LogStream { .. } => Err(CallError::new(
            ErrorCode::BadRequest,
            "a log stream takes its own connection",
        )),
        Call::PolicyShow => serde_json::to_value(policy::load(&state.policy_path, state.owner))
            .map_err(|e| CallError::internal(e.to_string())),
        Call::AuditAppend { kind, data } => blocking(move || {
            let mut log = state.audit.lock().unwrap_or_else(PoisonError::into_inner);
            log.append("agent", &kind, data)
                .map(|(seq, hash)| json!({ "seq": seq, "hash": hash }))
        })
        .await
        .map_err(CallError::internal),
        Call::AuditKey => blocking(move || {
            SigningKey::load_or_generate(&state.audit_key_path)
                .map(|key| json!({ "key": key.public_key(), "fingerprint": key.fingerprint() }))
        })
        .await
        .map_err(CallError::internal),
        Call::ServiceAct {
            request_id,
            unit,
            action,
            scope,
            user,
            actor,
        } => {
            let target = Target { unit, scope, user };
            act_on_service(state, request_id, target, action, actor).await
        }
        Call::AppQuit {
            request_id,
            app,
            user,
            force,
            actor,
        } => quit_app(state, request_id, app, user, force, actor).await,
        Call::Power {
            request_id,
            action,
            actor,
        } => power(state, request_id, action, actor).await,
        Call::ProcessList => {
            if !policy::load(&state.policy_path, state.owner).allows("processes.read") {
                let reason = "the device policy doesn't allow processes.read";
                return Err(CallError::new(ErrorCode::PolicyDenied, reason));
            }
            blocking(move || {
                let mut reader = state
                    .processes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let processes = reader
                    .get_or_insert_with(cntrl_host::processes::Sampler::new)
                    .read();
                Ok(json!({ "processes": processes }))
            })
            .await
            .map_err(CallError::internal)
        }
        Call::ProcessSignal {
            request_id,
            pid,
            started,
            force,
            actor,
        } => stop_process(state, request_id, pid, started, force, actor).await,
        Call::ServiceListSessions => {
            if !policy::load(&state.policy_path, state.owner).allows("services.read") {
                let reason = "the device policy doesn't allow services.read";
                return Err(CallError::new(ErrorCode::PolicyDenied, reason));
            }
            blocking(|| {
                let services = session_services()?;
                Ok(json!({ "services": services }))
            })
            .await
            .map_err(CallError::internal)
        }
        Call::SocketOwners { inodes } => {
            if !policy::load(&state.policy_path, state.owner).allows("network.read") {
                let reason = "the device policy doesn't allow network.read";
                return Err(CallError::new(ErrorCode::PolicyDenied, reason));
            }
            blocking(move || {
                let wanted: std::collections::HashSet<u64> = inodes.into_iter().collect();
                let owners = cntrl_host::network::socket_owners(std::path::Path::new("/"), &wanted);
                Ok(json!({ "owners": owners.into_iter().collect::<Vec<(u64, u32)>>() }))
            })
            .await
            .map_err(CallError::internal)
        }
        Call::Listeners => {
            if !policy::load(&state.policy_path, state.owner).allows("network.read") {
                let reason = "the device policy doesn't allow network.read";
                return Err(CallError::new(ErrorCode::PolicyDenied, reason));
            }
            listeners().await
        }
        Call::DiskHealth { disks } => {
            if !policy::load(&state.policy_path, state.owner).allows("system.read") {
                let reason = "the device policy doesn't allow system.read";
                return Err(CallError::new(ErrorCode::PolicyDenied, reason));
            }
            disk_health(disks).await
        }
        Call::Containers { counters } => {
            if !policy::load(&state.policy_path, state.owner).allows("containers.read") {
                let reason = "the device policy doesn't allow containers.read";
                return Err(CallError::new(ErrorCode::PolicyDenied, reason));
            }
            serde_json::to_value(docker::list(counters).await)
                .map_err(|e| CallError::internal(e.to_string()))
        }
        Call::ContainerAct {
            request_id,
            container,
            action,
            actor,
        } => act_on_container(state, request_id, container, action, actor).await,
    }
}

/// Starts, stops or restarts a container (D54): checks the policy, audits the
/// request, acts through the engine and audits how it went.
async fn act_on_container(
    state: Arc<State>,
    id: String,
    container: String,
    action: docker::Action,
    actor: Option<Actor>,
) -> Result<Value, CallError> {
    let container = docker::container_name(&container)
        .map_err(|e| CallError::new(ErrorCode::BadRequest, e))?
        .to_owned();
    let request = json!({
        "id": id,
        "op": action.op(),
        "container": container,
        "actor": actor,
    });
    if !policy::load(&state.policy_path, state.owner).allows("containers.manage") {
        let reason = "the device policy doesn't allow containers.manage";
        audit(
            &state,
            "request.denied",
            json!({ "request": request, "reason": reason }),
        )
        .await?;
        return Err(CallError::new(ErrorCode::PolicyDenied, reason));
    }
    audit(&state, "request.allowed", json!({ "request": request })).await?;
    let (record, answer) = match docker::act(&container, action).await {
        Ok(after) => (
            json!({ "id": id, "state": after }),
            serde_json::to_value(ContainerJob {
                id: container,
                state: after,
            })
            .map_err(|e| CallError::internal(e.to_string())),
        ),
        Err(e) => {
            let code = if e.starts_with("No such container") {
                ErrorCode::NotFound
            } else {
                ErrorCode::Internal
            };
            (
                json!({ "id": id, "error": e }),
                Err(CallError::new(code, e)),
            )
        }
    };
    audit(&state, "request.completed", record).await?;
    answer
}

/// Every port listened on, from lsof as root (angle 13).
#[cfg(target_os = "macos")]
async fn listeners() -> Result<Value, CallError> {
    blocking(|| {
        let found = cntrl_host::network::mac::listeners().map_err(|e| e.to_string())?;
        serde_json::to_value(found).map_err(|e| e.to_string())
    })
    .await
    .map_err(CallError::internal)
}

#[cfg(not(target_os = "macos"))]
async fn listeners() -> Result<Value, CallError> {
    let reason = "on this OS the agent reads the sockets and asks privd only who owns them";
    Err(CallError::new(ErrorCode::BadRequest, reason))
}

/// Each disk's SMART health, from smartctl, which reads the raw device.
#[cfg(target_os = "linux")]
async fn disk_health(disks: Vec<String>) -> Result<Value, CallError> {
    let health = cntrl_host::storage::smart_health(&disks).await;
    serde_json::to_value(health).map_err(|e| CallError::internal(e.to_string()))
}

#[cfg(not(target_os = "linux"))]
async fn disk_health(_disks: Vec<String>) -> Result<Value, CallError> {
    let reason = "on this OS the agent asks diskutil itself";
    Err(CallError::new(ErrorCode::BadRequest, reason))
}

/// Stops a process for Console, under `processes.signal` (D24). privd checks
/// the policy itself, refuses what has to keep running, and audits before
/// acting. A process that isn't running, or whose PID is another's now, is
/// left for `stop` to report.
async fn stop_process(
    state: Arc<State>,
    id: String,
    pid: u32,
    started: u64,
    force: bool,
    actor: Option<Actor>,
) -> Result<Value, CallError> {
    let target = tokio::task::spawn_blocking(move || cntrl_host::processes::target(pid))
        .await
        .map_err(|e| CallError::internal(e.to_string()))?;
    let policy = policy::load(&state.policy_path, state.owner);
    let refusal = if !policy.allows("processes.signal") {
        Some("the device policy doesn't allow processes.signal".to_owned())
    } else {
        target.as_ref().and_then(|target| {
            if pid <= 1 || target.kernel {
                Some(format!("process {pid} is part of the operating system"))
            } else if pid == std::process::id()
                || target
                    .owner
                    .as_ref()
                    .is_some_and(|owner| state.agent.as_ref() == Some(owner))
            {
                Some("that's the cntrl agent, which stops with its service".to_owned())
            } else {
                target
                    .unit
                    .as_ref()
                    .filter(|unit| policy.protects(unit))
                    .map(|unit| format!("it runs in {unit}, which the device policy protects"))
            }
        })
    };
    let request = json!({
        "id": id,
        "op": "process.signal",
        "pid": pid,
        "started": started,
        "force": force,
        "unit": target.as_ref().and_then(|target| target.unit.clone()),
        "actor": actor,
    });
    if let Some(reason) = refusal {
        audit(
            &state,
            "request.denied",
            json!({ "request": request, "reason": reason }),
        )
        .await?;
        return Err(CallError::new(ErrorCode::PolicyDenied, reason));
    }
    audit(&state, "request.allowed", json!({ "request": request })).await?;
    let outcome =
        tokio::task::spawn_blocking(move || cntrl_host::processes::stop(pid, started, force))
            .await
            .map_err(|e| CallError::internal(e.to_string()))?;
    let (record, answer) = match outcome {
        Ok(result) => (
            json!({ "id": id, "result": result }),
            serde_json::to_value(ProcessSignalResult { pid, result })
                .map_err(|e| CallError::internal(e.to_string())),
        ),
        Err(e) => (json!({ "id": id, "error": e.to_string() }), Err(e.into())),
    };
    audit(&state, "request.completed", record).await?;
    answer
}

/// Quits an app for Console, under `processes.signal`. As with restarts,
/// privd checks the policy itself and audits before acting.
async fn quit_app(
    state: Arc<State>,
    id: String,
    app: String,
    user: Option<String>,
    force: bool,
    actor: Option<Actor>,
) -> Result<Value, CallError> {
    let app = cntrl_host::services::launchd_label(&app)?;
    let policy = policy::load(&state.policy_path, state.owner);
    let refusal = if !policy.allows("processes.signal") {
        Some("the device policy doesn't allow processes.signal".to_owned())
    } else if policy.protects(&app) {
        Some(format!("the device policy protects {app}"))
    } else {
        None
    };
    let request = json!({
        "id": id,
        "op": "app.quit",
        "app": app,
        "user": user,
        "force": force,
        "actor": actor,
    });
    if let Some(reason) = refusal {
        audit(
            &state,
            "request.denied",
            json!({ "request": request, "reason": reason }),
        )
        .await?;
        return Err(CallError::new(ErrorCode::PolicyDenied, reason));
    }
    audit(&state, "request.allowed", json!({ "request": request })).await?;
    let outcome = quit(app.clone(), user, force).await;
    let (record, answer) = match outcome {
        Ok(result) => (
            json!({ "id": id, "result": result }),
            serde_json::to_value(AppQuitResult { app, result })
                .map_err(|e| CallError::internal(e.to_string())),
        ),
        Err(e) => (json!({ "id": id, "error": e.to_string() }), Err(e.into())),
    };
    audit(&state, "request.completed", record).await?;
    answer
}

#[cfg(target_os = "macos")]
async fn quit(app: String, user: Option<String>, force: bool) -> Result<QuitResult, HostError> {
    tokio::task::spawn_blocking(move || {
        let (name, uid) = session(user)?;
        cntrl_host::launchd::quit_app(&name, uid, &app, force)
    })
    .await
    .map_err(|e| HostError::Failed(e.to_string()))?
}

#[cfg(not(target_os = "macos"))]
async fn quit(_app: String, _user: Option<String>, _force: bool) -> Result<QuitResult, HostError> {
    Err(HostError::Unsupported)
}

/// The desktop session a request names, or with no user named, the only one.
#[cfg(target_os = "macos")]
fn session(user: Option<String>) -> Result<(String, u32), HostError> {
    let sessions = cntrl_host::launchd::sessions();
    let found = match &user {
        Some(name) => sessions.iter().find(|(session, _)| session == name),
        None if sessions.len() == 1 => sessions.first(),
        None => None,
    };
    found.cloned().ok_or_else(|| {
        HostError::NotFound(match (&user, sessions.len()) {
            (Some(name), _) => format!("{name} has no desktop session"),
            (None, 0) => "nobody is logged in".to_owned(),
            (None, _) => "several users are logged in; name one".to_owned(),
        })
    })
}

/// Which service a restart is for, and where it runs.
struct Target {
    unit: String,
    scope: ServiceScope,
    user: Option<String>,
}

/// What runs in each logged-in user's desktop session.
#[cfg(target_os = "macos")]
fn session_services() -> Result<Vec<ServiceStatus>, String> {
    let mut services = Vec::new();
    for (user, uid) in cntrl_host::launchd::sessions() {
        services.extend(cntrl_host::launchd::list_session(&user, uid).map_err(|e| e.to_string())?);
    }
    Ok(services)
}

#[cfg(not(target_os = "macos"))]
fn session_services() -> Result<Vec<ServiceStatus>, String> {
    Ok(Vec::new())
}

/// Restarts a service for Console. privd checks the policy itself, whatever
/// the agent decided, and audits its decision, synced to disk, before acting.
async fn act_on_service(
    state: Arc<State>,
    id: String,
    target: Target,
    action: ServiceAction,
    actor: Option<Actor>,
) -> Result<Value, CallError> {
    let unit = service_name(&target.unit)?;
    let policy = policy::load(&state.policy_path, state.owner);
    // A protected service can still be started or enabled (angle 11).
    let refusal = if !policy.allows("services.manage") {
        Some("the device policy doesn't allow services.manage".to_owned())
    } else if action.interrupts() && policy.protects(&unit) {
        Some(format!("the device policy protects {unit}"))
    } else {
        None
    };
    let request = json!({
        "id": id,
        "op": action.op(),
        "unit": unit,
        "scope": target.scope,
        "user": target.user,
        "actor": actor,
    });
    if let Some(reason) = refusal {
        audit(
            &state,
            "request.denied",
            json!({ "request": request, "reason": reason }),
        )
        .await?;
        return Err(CallError::new(ErrorCode::PolicyDenied, reason));
    }
    audit(&state, "request.allowed", json!({ "request": request })).await?;
    let outcome = tokio::time::timeout(JOB_LIMIT, act(&unit, target.scope, target.user, action))
        .await
        .unwrap_or_else(|_| {
            Err(HostError::Failed(format!(
                "the service manager gave no result within {JOB_LIMIT:?}"
            )))
        });
    let (record, answer) = match outcome {
        Ok(result) => (
            json!({ "id": id, "result": result }),
            serde_json::to_value(ServiceJob { unit, result })
                .map_err(|e| CallError::internal(e.to_string())),
        ),
        Err(e) => (json!({ "id": id, "error": e.to_string() }), Err(e.into())),
    };
    audit(&state, "request.completed", record).await?;
    answer
}

#[cfg(target_os = "linux")]
async fn act(
    unit: &str,
    scope: ServiceScope,
    _user: Option<String>,
    action: ServiceAction,
) -> Result<JobResult, HostError> {
    if !scope.is_system() {
        return Err(HostError::Invalid(
            "services in a user's session aren't supported on Linux yet".to_owned(),
        ));
    }
    cntrl_host::systemd::Systemd::connect()
        .await?
        .act(unit, action)
        .await
}

/// On macOS a service in a user's session restarts in that user's GUI domain;
/// with no user named, the one user logged in.
#[cfg(target_os = "macos")]
async fn act(
    label: &str,
    scope: ServiceScope,
    user: Option<String>,
    action: ServiceAction,
) -> Result<JobResult, HostError> {
    use cntrl_host::launchd;
    let label = label.to_owned();
    tokio::task::spawn_blocking(move || {
        let domain = match scope {
            ServiceScope::System => launchd::system_domain(),
            ServiceScope::User => launchd::user_domain(session(user)?.1),
        };
        launchd::act(&domain, &label, action)
    })
    .await
    .map_err(|e| HostError::Failed(e.to_string()))?
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn act(
    _unit: &str,
    _scope: ServiceScope,
    _user: Option<String>,
    _action: ServiceAction,
) -> Result<JobResult, HostError> {
    Err(HostError::Unsupported)
}

/// Appends one of privd's own records to the audit log, synced to disk.
/// How long after answering privd takes a power action: long enough for the
/// answer to reach Console before the network goes.
const POWER_DELAY: Duration = Duration::from_secs(2);

/// Restarts, shuts down, sleeps or hibernates the machine for a request from
/// Console (angle 10). privd checks the policy itself and that nothing holds
/// the action off, audits, answers, and acts a moment later, since once the
/// machine goes an answer can't.
async fn power(
    state: Arc<State>,
    id: String,
    action: PowerAction,
    actor: Option<Actor>,
) -> Result<Value, CallError> {
    let request = json!({ "id": id, "op": action.op(), "actor": actor });
    if !policy::load(&state.policy_path, state.owner).allows(action.op()) {
        let reason = format!("the device policy doesn't allow {}", action.op());
        audit(
            &state,
            "request.denied",
            json!({ "request": request, "reason": reason }),
        )
        .await?;
        return Err(CallError::new(ErrorCode::PolicyDenied, reason));
    }
    audit(&state, "request.allowed", json!({ "request": request })).await?;
    if let Err(e) = power_check(action).await {
        audit(
            &state,
            "request.completed",
            json!({ "id": id, "error": e.to_string() }),
        )
        .await?;
        return Err(e.into());
    }
    audit(
        &state,
        "request.completed",
        json!({ "id": id, "result": "started" }),
    )
    .await?;
    tokio::spawn(async move {
        tokio::time::sleep(POWER_DELAY).await;
        if let Err(e) = power_act(action).await {
            warn!("{} failed: {e}", action.op());
            let failure = json!({ "id": id, "error": e.to_string() });
            if let Err(e) = audit(&state, "request.failed", failure).await {
                warn!("can't audit the failure: {}", e.msg);
            }
        }
    });
    serde_json::to_value(PowerStarted { action }).map_err(|e| CallError::internal(e.to_string()))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn power_check(action: PowerAction) -> Result<(), HostError> {
    cntrl_host::power::check(action).await
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn power_act(action: PowerAction) -> Result<(), HostError> {
    cntrl_host::power::act(action).await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn power_check(_action: PowerAction) -> Result<(), HostError> {
    Err(HostError::Unsupported)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn power_act(_action: PowerAction) -> Result<(), HostError> {
    Err(HostError::Unsupported)
}

async fn audit(state: &Arc<State>, kind: &'static str, data: Value) -> Result<(), CallError> {
    let state = Arc::clone(state);
    blocking(move || {
        let mut log = state.audit.lock().unwrap_or_else(PoisonError::into_inner);
        log.append("privd", kind, data).map(|_| Value::Null)
    })
    .await
    .map(drop)
    .map_err(CallError::internal)
}

/// Runs file work on the blocking pool.
async fn blocking(
    work: impl FnOnce() -> Result<Value, String> + Send + 'static,
) -> Result<Value, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

/// Answers a `log_stream` call (angle 11 part 3, D54): checks the policy,
/// finds where the log comes from, then sends batches as answers to the one
/// request until the log stops, which the last says why, or the agent hangs
/// up, which ends what reads it.
async fn stream_logs(
    state: &Arc<State>,
    id: u64,
    params: LogsParams,
    channel: ipc::Channel<os::ServerStream>,
) {
    use cntrl_protocol::logs::LogsBatch;
    use futures_util::{SinkExt, StreamExt};

    let (mut sink, mut incoming) = channel.split();
    let mut answer = async |result: Result<Value, CallError>| {
        let frame = serde_json::to_vec(&Response::from_result(id, result)).unwrap_or_default();
        sink.send(bytes::Bytes::from(frame)).await.is_ok()
    };
    let policy = policy::load(&state.policy_path, state.owner);
    let needs: &[&str] = if params.container.is_some() {
        &["logs.read", "containers.read"]
    } else {
        &["logs.read"]
    };
    if let Some(missing) = needs.iter().find(|capability| !policy.allows(capability)) {
        let refused = CallError::new(
            ErrorCode::PolicyDenied,
            format!("the device policy doesn't allow {missing}"),
        );
        answer(Err(refused)).await;
        return;
    }
    let (out, mut batches) = tokio::sync::mpsc::channel(16);
    let mut reader = match log_reader(params, out).await {
        Ok(reader) => reader,
        Err(e) => {
            answer(Err(e)).await;
            return;
        }
    };
    let reason = loop {
        tokio::select! {
            batch = batches.recv() => match batch {
                Some(batch) => {
                    let value = serde_json::to_value(&batch).unwrap_or_default();
                    if !answer(Ok(value)).await {
                        reader.abort();
                        return;
                    }
                }
                // The reader has stopped, and drops its sender.
                None => break (&mut reader).await.unwrap_or_else(|e| e.to_string()),
            },
            // The agent hung up, or broke the one-request rule.
            _ = incoming.next() => {
                reader.abort();
                return;
            }
        }
    };
    let last = LogsBatch {
        ended: Some(reason),
        ..LogsBatch::default()
    };
    answer(Ok(serde_json::to_value(&last).unwrap_or_default())).await;
}

/// What reads a subscription's log: a container's through its engine, on any
/// OS (D54); on a Mac, the unified log or a launchd job's.
async fn log_reader(
    params: LogsParams,
    out: tokio::sync::mpsc::Sender<cntrl_protocol::logs::LogsBatch>,
) -> Result<tokio::task::JoinHandle<String>, CallError> {
    if params.container.is_some() {
        return Ok(tokio::spawn(async move {
            docker::read_logs(&params, &out).await
        }));
    }
    #[cfg(target_os = "macos")]
    {
        let source = log_source(&params).await?;
        Ok(tokio::spawn(async move {
            super::logs::mac::read(&params, &source, &out).await
        }))
    }
    #[cfg(not(target_os = "macos"))]
    {
        drop(out);
        Err(CallError::new(
            ErrorCode::BadRequest,
            "the agent reads the journal itself on this OS",
        ))
    }
}

/// Where a subscription's log comes from: the whole system's unified log, or
/// one launchd job's lines and output files.
#[cfg(target_os = "macos")]
async fn log_source(params: &LogsParams) -> Result<super::logs::mac::Source, CallError> {
    use super::logs::mac::Source;
    use cntrl_host::launchd;

    let Some(unit) = params.unit.clone() else {
        return Ok(Source::default());
    };
    let label = service_name(&unit)?;
    let user = params.user.clone();
    tokio::task::spawn_blocking(move || -> Result<Source, HostError> {
        let (domain, files_as) = match user {
            Some(user) => {
                let (name, uid) = session(Some(user))?;
                let gid = os::group_of(&name)
                    .ok_or_else(|| HostError::NotFound(format!("{name} has no group")))?;
                (launchd::user_domain(uid), Some((uid, gid)))
            }
            None => (launchd::system_domain(), None),
        };
        let log = launchd::job_log(&domain, &label)?;
        Ok(Source {
            job: Some(log.job),
            files: log.files,
            files_as,
        })
    })
    .await
    .map_err(|e| CallError::internal(e.to_string()))?
    .map_err(CallError::from)
}

/// Counts open connections and remembers when the last one ended.
struct Activity {
    origin: Instant,
    active: AtomicUsize,
    last_ms: AtomicU64,
}

impl Activity {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            active: AtomicUsize::new(0),
            last_ms: AtomicU64::new(0),
        }
    }

    fn start(&self) {
        self.active.fetch_add(1, Ordering::SeqCst);
        self.touch();
    }

    fn finish(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.touch();
    }

    fn idle_for(&self, timeout: Duration) -> bool {
        let idle_ms = self
            .now_ms()
            .saturating_sub(self.last_ms.load(Ordering::SeqCst));
        self.active.load(Ordering::SeqCst) == 0 && u128::from(idle_ms) >= timeout.as_millis()
    }

    fn touch(&self) {
        self.last_ms.store(self.now_ms(), Ordering::SeqCst);
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {

    use super::super::config::Paths;
    use super::super::policy::PolicyState;
    use super::*;

    #[tokio::test]
    async fn answers_ping_policy_audit_and_key_calls() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = Config {
            paths: Paths {
                privd_socket: os::test_endpoint(dir.path(), "privd"),
                privd_state_dir: dir.path().join("privd"),
                policy: dir.path().join("policy.toml"),
                audit_dir: dir.path().join("audit"),
                ..Paths::default()
            },
            ..Config::default()
        };
        let socket = config.paths.privd_socket.clone();
        let server = tokio::spawn(async move { serve(&config, CancellationToken::new()).await });
        for _ in 0..50 {
            if os::is_endpoint(&socket) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let pong = ipc::call_once(&socket, Call::Ping).await.expect("ping");
        assert_eq!(pong["version"], env!("CARGO_PKG_VERSION"));

        let policy = ipc::call_once(&socket, Call::PolicyShow)
            .await
            .expect("policy");
        let policy: PolicyState = serde_json::from_value(policy).expect("policy state");
        assert!(policy.allows("system.read"));

        let appended = ipc::call_once(
            &socket,
            Call::AuditAppend {
                kind: "agent.started".into(),
                data: json!({}),
            },
        )
        .await
        .expect("audit append");
        assert_eq!(appended["seq"], 0);

        let first = ipc::call_once(&socket, Call::AuditKey)
            .await
            .expect("audit key");
        let second = ipc::call_once(&socket, Call::AuditKey)
            .await
            .expect("audit key again");
        assert_eq!(first["key"], second["key"], "the audit key is created once");

        server.abort();
    }
}
