//! The local API: HTTP and JSON over a Unix socket, for the `cntrl` CLI. Reads are
//! open to the socket's group; anything that changes who controls the machine
//! needs a root peer. It also answers
//! `curl --unix-socket /run/cntrl-agent/agent.sock http://cntrl/v1/status`.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::extract::connect_info::{self, ConnectInfo};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::serve::IncomingStream;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::config::Config;
use super::enroll::{self, EnrollCommand, EnrollOutcome};
use super::health::Health;
use super::identity;
use super::ipc::{self, Call};
use super::policy::PolicyState;

/// What the local API reads from the running agent.
pub struct AgentState {
    config: Config,
    config_path: PathBuf,
    health: Arc<Health>,
    /// One enrollment at a time.
    enrolling: Mutex<()>,
}

impl AgentState {
    pub fn new(config: Config, config_path: PathBuf, health: Arc<Health>) -> Self {
        Self {
            config,
            config_path,
            health,
            enrolling: Mutex::new(()),
        }
    }

    /// The agent's status. Asking privd for the policy also starts privd if its
    /// socket is installed.
    async fn status(&self) -> Status {
        let privd = match ipc::call_once(&self.config.paths.privd_socket, Call::PolicyShow).await {
            Ok(policy) => PrivdStatus {
                reachable: true,
                error: None,
                policy: serde_json::from_value(policy).ok(),
            },
            Err(e) => PrivdStatus {
                reachable: false,
                error: Some(e),
                policy: None,
            },
        };
        let device_id = identity::load(&self.config.paths.state_dir)
            .ok()
            .flatten()
            .map(|identity| identity.device_id);
        Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            uptime_s: self.health.uptime().as_secs(),
            uplink: if device_id.is_some() {
                Uplink::Enrolled
            } else {
                Uplink::NotEnrolled
            },
            device_id,
            config: self.config_path.display().to_string(),
            privd,
        }
    }
}

/// `GET /v1/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub pid: u32,
    pub uptime_s: u64,
    pub uplink: Uplink,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub config: String,
    pub privd: PrivdStatus,
}

/// The connection to Console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Uplink {
    NotEnrolled,
    Enrolled,
}

/// Whether privd answers, and the policy it reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivdStatus {
    pub reachable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicyState>,
}

/// The user on the other end of a connection, from the socket's credentials.
#[derive(Debug, Clone, Copy)]
struct Peer {
    uid: Option<u32>,
}

impl connect_info::Connected<IncomingStream<'_, UnixListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, UnixListener>) -> Self {
        Self {
            uid: stream.io().peer_cred().ok().map(|cred| cred.uid()),
        }
    }
}

/// Binds the socket, replacing a stale one from an earlier run. Mode 0660: the
/// owner and its group, and root.
pub fn bind(path: &Path) -> Result<UnixListener, String> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "can't remove the stale socket {}: {e}",
                path.display()
            ));
        }
    }
    let listener =
        UnixListener::bind(path).map_err(|e| format!("can't bind {}: {e}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o660))
        .map_err(|e| format!("can't set permissions on {}: {e}", path.display()))?;
    Ok(listener)
}

pub async fn serve(
    listener: UnixListener,
    state: Arc<AgentState>,
    token: CancellationToken,
) -> Result<(), String> {
    let app = Router::new()
        .route("/v1/status", get(status))
        .route("/v1/enroll", post(enroll_device))
        .with_state(state);
    axum::serve(listener, app.into_make_service_with_connect_info::<Peer>())
        .with_graceful_shutdown(async move { token.cancelled().await })
        .await
        .map_err(|e| e.to_string())
}

async fn status(State(state): State<Arc<AgentState>>) -> Json<Status> {
    Json(state.status().await)
}

async fn enroll_device(
    ConnectInfo(peer): ConnectInfo<Peer>,
    State(state): State<Arc<AgentState>>,
    Json(command): Json<EnrollCommand>,
) -> Result<Json<EnrollOutcome>, (StatusCode, String)> {
    if peer.uid != Some(0) {
        return Err((
            StatusCode::FORBIDDEN,
            "enrolling changes who controls this machine; run `sudo cntrl enroll`".to_owned(),
        ));
    }
    let _one_at_a_time = state.enrolling.lock().await;
    enroll::enroll(&state.config, command)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use hyper::Method;

    use super::super::client;
    use super::super::config::Paths;
    use super::*;

    fn start(
        dir: &Path,
    ) -> (
        PathBuf,
        CancellationToken,
        tokio::task::JoinHandle<Result<(), String>>,
    ) {
        let socket = dir.join("agent.sock");
        let listener = bind(&socket).expect("bind");
        let config = Config {
            paths: Paths {
                privd_socket: dir.join("no-privd.sock"),
                state_dir: dir.to_owned(),
                ..Paths::default()
            },
            ..Config::default()
        };
        let state = Arc::new(AgentState::new(
            config,
            "/etc/cntrl/agent.toml".into(),
            Arc::new(Health::new()),
        ));
        let token = CancellationToken::new();
        let server = tokio::spawn(serve(listener, state, token.clone()));
        (socket, token, server)
    }

    #[tokio::test]
    async fn status_is_served_over_the_socket() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (socket, token, server) = start(dir.path());
        let mode = fs::metadata(&socket)
            .expect("socket metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o660);

        let status = client::get_status(&socket).await.expect("status");
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(status.uplink, Uplink::NotEnrolled);
        assert!(!status.privd.reachable);

        token.cancel();
        server
            .await
            .expect("server task")
            .expect("server stopped cleanly");
    }

    #[tokio::test]
    async fn enrolling_needs_a_root_peer() {
        if fs::metadata("/proc/self")
            .map(|metadata| metadata.uid())
            .unwrap_or(0)
            == 0
        {
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let (socket, token, server) = start(dir.path());
        let body = br#"{"token":"cntrl_et_x"}"#.to_vec();
        let (status, _) = client::request(&socket, Method::POST, "/v1/enroll", body)
            .await
            .expect("request");
        assert_eq!(status, StatusCode::FORBIDDEN);

        token.cancel();
        server
            .await
            .expect("server task")
            .expect("server stopped cleanly");
    }
}
