//! privd, the privileged half of the agent. It runs as root with no network,
//! started on demand by its systemd or launchd socket, and is the only part that
//! reads the policy, writes the audit log, holds the audit key and acts on the
//! machine. It accepts connections only from root, the agent's user (`cntrl`,
//! or `_cntrl` on macOS) and its own user, and exits after a minute without one.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cntrl_host::HostError;
use cntrl_host::services::service_name;
use cntrl_protocol::app::{AppQuitResult, QuitResult};
use cntrl_protocol::codes::ErrorCode;
use cntrl_protocol::frame::Actor;
use cntrl_protocol::power::{PowerAction, PowerStarted};
use cntrl_protocol::process::ProcessSignalResult;
use cntrl_protocol::service::{JobResult, ServiceAction, ServiceJob, ServiceScope, ServiceStatus};
use serde_json::{Value, json};
use tokio::net::UnixStream;
use tracing::{error, info, warn};

use super::audit::AuditLog;
use super::config::Config;
use super::ipc::{self, Call, CallError, Request, Response};
use super::keys::SigningKey;
use super::{local_api, logging, policy};

const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const IDLE_CHECK: Duration = Duration::from_secs(5);
const AUDIT_KEY_FILE: &str = "audit.key";
/// The account the agent runs as, which the installer creates.
#[cfg(target_os = "macos")]
pub(super) const AGENT_USER: &str = "_cntrl";
#[cfg(not(target_os = "macos"))]
pub(super) const AGENT_USER: &str = "cntrl";
/// How long privd waits for systemd's verdict on a job.
const JOB_LIMIT: Duration = Duration::from_secs(300);

struct State {
    audit: Mutex<AuditLog>,
    policy_path: PathBuf,
    audit_key_path: PathBuf,
    /// The user privd runs as; the policy file must belong to it.
    owner: u32,
    allowed: Vec<u32>,
    /// The agent's user, whose processes privd won't stop.
    agent: Option<u32>,
    /// The process table's reader, made on first use; on macOS only privd
    /// sees every process (D24).
    processes: Mutex<Option<cntrl_host::processes::Sampler>>,
}

pub fn main(config: &Config) -> ExitCode {
    logging::init(&config.log.level);
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
    match runtime.block_on(serve(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(config: &Config) -> Result<(), String> {
    let listener = local_api::listen(&config.paths.privd_socket)?;
    let audit = AuditLog::open(&config.paths.audit_dir)?;
    let state_dir = &config.paths.privd_state_dir;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(state_dir)
        .map_err(|e| format!("can't create {}: {e}", state_dir.display()))?;
    let owner = own_uid();
    let mut allowed = vec![0, owner];
    let agent = uid_of(AGENT_USER);
    allowed.extend(agent);
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
                let (stream, _) = accepted.map_err(|e| format!("accept failed: {e}"))?;
                if !peer_allowed(&stream, &state.allowed) {
                    continue;
                }
                let (state, activity) = (Arc::clone(&state), Arc::clone(&activity));
                activity.start();
                tokio::spawn(async move {
                    handle(stream, &state).await;
                    activity.finish();
                });
            }
            () = tokio::time::sleep(IDLE_CHECK) => {
                if activity.idle_for(IDLE_TIMEOUT) {
                    info!("idle for {IDLE_TIMEOUT:?}, exiting");
                    return Ok(());
                }
            }
        }
    }
}

fn peer_allowed(stream: &UnixStream, allowed: &[u32]) -> bool {
    match stream.peer_cred() {
        Ok(cred) if allowed.contains(&cred.uid()) => true,
        Ok(cred) => {
            warn!(
                uid = cred.uid(),
                "refused a connection from a user that isn't allowed"
            );
            false
        }
        Err(e) => {
            warn!("refused a connection without peer credentials: {e}");
            false
        }
    }
}

async fn handle(stream: UnixStream, state: &Arc<State>) {
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
    }
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
                || target.uid.is_some_and(|uid| state.agent == Some(uid))
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

/// The user this process runs as.
fn own_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// Looks `user` up in `/etc/passwd`, where the installer creates it.
#[cfg(not(target_os = "macos"))]
fn uid_of(user: &str) -> Option<u32> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == user).then_some(())?;
        fields.nth(1)?.parse().ok()
    })
}

/// Looks `user` up through `id`, which asks Directory Services: macOS keeps the
/// accounts the installer creates out of `/etc/passwd`.
#[cfg(target_os = "macos")]
fn uid_of(user: &str) -> Option<u32> {
    let output = std::process::Command::new("/usr/bin/id")
        .args(["-u", user])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
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
                privd_socket: dir.path().join("privd.sock"),
                privd_state_dir: dir.path().join("privd"),
                policy: dir.path().join("policy.toml"),
                audit_dir: dir.path().join("audit"),
                ..Paths::default()
            },
            ..Config::default()
        };
        let socket = config.paths.privd_socket.clone();
        let server = tokio::spawn(async move { serve(&config).await });
        for _ in 0..50 {
            if socket.exists() {
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

    #[test]
    fn finds_users_in_passwd_lines() {
        assert_eq!(uid_of("root"), Some(0));
        assert_eq!(uid_of("no-such-user-for-cntrl-tests"), None);
    }
}
