//! The local API: HTTP and JSON over a Unix socket, for the `cntrl` CLI. It also
//! answers `curl --unix-socket /run/cntrl-agent/agent.sock http://cntrl/v1/status`.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

use super::health::Health;
use super::ipc::{self, Call};
use super::policy::PolicyState;

/// What the local API reads from the running agent.
pub struct AgentState {
    config_path: PathBuf,
    privd_socket: PathBuf,
    health: Arc<Health>,
}

impl AgentState {
    pub fn new(config_path: PathBuf, privd_socket: PathBuf, health: Arc<Health>) -> Self {
        Self {
            config_path,
            privd_socket,
            health,
        }
    }

    /// The agent's status. Asking privd for the policy also starts privd if its
    /// socket is installed.
    async fn status(&self) -> Status {
        let privd = match ipc::call_once(&self.privd_socket, Call::PolicyShow).await {
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
        Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            uptime_s: self.health.uptime().as_secs(),
            uplink: Uplink::NotEnrolled,
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
    pub config: String,
    pub privd: PrivdStatus,
}

/// The connection to Console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Uplink {
    NotEnrolled,
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
        .with_state(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { token.cancelled().await })
        .await
        .map_err(|e| e.to_string())
}

async fn status(State(state): State<Arc<AgentState>>) -> Json<Status> {
    Json(state.status().await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_is_served_over_the_socket() {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("agent.sock");
        let listener = bind(&socket).expect("bind");
        let mode = fs::metadata(&socket)
            .expect("socket metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o660);

        let state = Arc::new(AgentState::new(
            "/etc/cntrl/agent.toml".into(),
            dir.path().join("no-privd.sock"),
            Arc::new(Health::new()),
        ));
        let token = CancellationToken::new();
        let server = tokio::spawn(serve(listener, state, token.clone()));

        let status = super::super::client::get_status(&socket)
            .await
            .expect("status");
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(status.uplink, Uplink::NotEnrolled);
        assert!(!status.privd.reachable);

        token.cancel();
        server
            .await
            .expect("server task")
            .expect("server stopped cleanly");
    }
}
