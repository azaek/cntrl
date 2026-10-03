//! privd, the privileged half of the agent. It runs as root with no network,
//! started on demand by its systemd socket, and is the only part that reads the
//! policy, writes the audit log and acts on the machine. It accepts connections
//! only from root, the `cntrl` user and its own user, and exits after a minute
//! without one.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

use super::audit::AuditLog;
use super::config::Config;
use super::ipc::{self, Call, Request, Response};
use super::{local_api, logging, policy};

const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const IDLE_CHECK: Duration = Duration::from_secs(5);

struct State {
    audit: Mutex<AuditLog>,
    policy_path: PathBuf,
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
    let listener = listener(&config.paths.privd_socket)?;
    let audit = AuditLog::open(&config.paths.audit_dir)?;
    let owner = own_uid();
    let mut allowed = vec![0, owner];
    allowed.extend(uid_of("cntrl"));
    let state = Arc::new(State {
        audit: Mutex::new(audit),
        policy_path: config.paths.policy.clone(),
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

/// Takes the socket systemd passed in, or binds one when started by hand.
fn listener(path: &Path) -> Result<UnixListener, String> {
    let mut fds = listenfd::ListenFd::from_env();
    match fds.take_unix_listener(0).map_err(|e| e.to_string())? {
        Some(listener) => {
            listener.set_nonblocking(true).map_err(|e| e.to_string())?;
            UnixListener::from_std(listener).map_err(|e| e.to_string())
        }
        None => local_api::bind(path),
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

async fn respond(state: &Arc<State>, call: Call) -> Result<Value, String> {
    match call {
        Call::Ping => Ok(json!({ "version": env!("CARGO_PKG_VERSION") })),
        Call::PolicyShow => serde_json::to_value(policy::load(&state.policy_path, state.owner))
            .map_err(|e| e.to_string()),
        Call::AuditAppend { kind, data } => {
            let state = Arc::clone(state);
            tokio::task::spawn_blocking(move || {
                let mut log = state.audit.lock().unwrap_or_else(PoisonError::into_inner);
                log.append("agent", &kind, data)
                    .map(|(seq, hash)| json!({ "seq": seq, "hash": hash }))
            })
            .await
            .map_err(|e| e.to_string())?
        }
    }
}

/// The user this process runs as; `/proc/self` belongs to it.
fn own_uid() -> u32 {
    fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .unwrap_or(0)
}

/// Looks `user` up in `/etc/passwd`, where the installer creates it.
fn uid_of(user: &str) -> Option<u32> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == user).then_some(())?;
        fields.nth(1)?.parse().ok()
    })
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
    async fn answers_ping_policy_and_audit_calls() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = Config {
            paths: Paths {
                privd_socket: dir.path().join("privd.sock"),
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

        server.abort();
    }

    #[test]
    fn finds_users_in_passwd_lines() {
        assert_eq!(uid_of("root"), Some(0));
        assert_eq!(uid_of("no-such-user-for-cntrl-tests"), None);
    }
}
