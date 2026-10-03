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
use cntrl_host::services::service_unit;
use cntrl_protocol::codes::ErrorCode;
use cntrl_protocol::frame::Actor;
use cntrl_protocol::records::{AuditCheckpoint, checkpoint_signing_string};
use cntrl_protocol::service::{JobResult, ServiceJob};
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
const AGENT_USER: &str = "_cntrl";
#[cfg(not(target_os = "macos"))]
const AGENT_USER: &str = "cntrl";
/// How long privd waits for systemd's verdict on a job.
const JOB_LIMIT: Duration = Duration::from_secs(300);

struct State {
    audit: Mutex<AuditLog>,
    policy_path: PathBuf,
    audit_key_path: PathBuf,
    /// The user privd runs as; the policy file must belong to it.
    owner: u32,
    allowed: Vec<u32>,
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
    allowed.extend(uid_of(AGENT_USER));
    let state = Arc::new(State {
        audit: Mutex::new(audit),
        policy_path: config.paths.policy.clone(),
        audit_key_path: state_dir.join(AUDIT_KEY_FILE),
        owner,
        allowed,
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
        Call::AuditCheckpoint {
            device_id,
            key_id,
            after,
        } => blocking(move || {
            let (next_seq, head) = state
                .audit
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .head();
            // An empty log, or nothing new since the last checkpoint.
            let Some(seq) = next_seq.checked_sub(1).filter(|seq| after != Some(*seq)) else {
                return Ok(Value::Null);
            };
            let ts = super::uplink::now_ms();
            let signed = checkpoint_signing_string(&device_id, seq, &head, ts, &key_id);
            let sig =
                SigningKey::load_or_generate(&state.audit_key_path)?.sign(signed.as_bytes())?;
            let checkpoint = AuditCheckpoint {
                device_id,
                seq,
                head,
                ts,
                key_id,
                sig,
            };
            serde_json::to_value(checkpoint).map_err(|e| e.to_string())
        })
        .await
        .map_err(CallError::internal),
        Call::AuditKey => blocking(move || {
            SigningKey::load_or_generate(&state.audit_key_path)
                .map(|key| json!({ "key": key.public_key(), "fingerprint": key.fingerprint() }))
        })
        .await
        .map_err(CallError::internal),
        Call::ServiceRestart {
            request_id,
            unit,
            actor,
        } => restart_service(state, request_id, unit, actor).await,
    }
}

/// Restarts a unit for Console. privd checks the policy itself, whatever the
/// agent decided, and audits its decision, synced to disk, before acting.
async fn restart_service(
    state: Arc<State>,
    id: String,
    unit: String,
    actor: Option<Actor>,
) -> Result<Value, CallError> {
    let unit = service_unit(&unit)?;
    let policy = policy::load(&state.policy_path, state.owner);
    let refusal = if !policy.allows("services.manage") {
        Some("the device policy doesn't allow services.manage".to_owned())
    } else if policy.protects(&unit) {
        Some(format!("the device policy protects {unit}"))
    } else {
        None
    };
    let request = json!({ "id": id, "op": "service.restart", "unit": unit, "actor": actor });
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
    let outcome = tokio::time::timeout(JOB_LIMIT, restart(&unit))
        .await
        .unwrap_or_else(|_| {
            Err(HostError::Failed(format!(
                "systemd gave no result within {JOB_LIMIT:?}"
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
async fn restart(unit: &str) -> Result<JobResult, HostError> {
    cntrl_host::systemd::Systemd::connect()
        .await?
        .restart(unit)
        .await
}

#[cfg(not(target_os = "linux"))]
async fn restart(_unit: &str) -> Result<JobResult, HostError> {
    Err(HostError::Unsupported)
}

/// Appends one of privd's own records to the audit log, synced to disk.
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
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

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

        let checkpoint = |after| Call::AuditCheckpoint {
            device_id: "dev_1".to_owned(),
            key_id: "key_1".to_owned(),
            after,
        };
        let signed = ipc::call_once(&socket, checkpoint(None))
            .await
            .expect("checkpoint");
        let signed: AuditCheckpoint = serde_json::from_value(signed).expect("a checkpoint");
        assert_eq!((signed.seq, signed.device_id.as_str()), (0, "dev_1"));
        let message = checkpoint_signing_string(
            &signed.device_id,
            signed.seq,
            &signed.head,
            signed.ts,
            &signed.key_id,
        );
        let public = URL_SAFE_NO_PAD
            .decode(first["key"].as_str().expect("a key"))
            .expect("base64");
        let sig = URL_SAFE_NO_PAD.decode(&signed.sig).expect("base64");
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
            .verify(message.as_bytes(), &sig)
            .expect("the audit key signed the checkpoint");
        let unchanged = ipc::call_once(&socket, checkpoint(Some(signed.seq)))
            .await
            .expect("checkpoint");
        assert_eq!(unchanged, Value::Null, "nothing new, no checkpoint");

        server.abort();
    }

    #[test]
    fn finds_users_in_passwd_lines() {
        assert_eq!(uid_of("root"), Some(0));
        assert_eq!(uid_of("no-such-user-for-cntrl-tests"), None);
    }
}
