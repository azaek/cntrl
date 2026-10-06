//! The local API: HTTP and JSON over a Unix socket, or a named pipe on
//! Windows, for the `cntrl` CLI. Reads are open to whoever may connect;
//! anything that changes who controls the machine, or deletes what it
//! recorded, needs a root peer. It also answers
//! `curl --unix-socket /run/cntrl-agent/agent.sock http://cntrl/v1/status`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use cntrl_protocol::ErrorCode;
use cntrl_protocol::enroll::EnrollErrorCode;
use cntrl_protocol::history::HistoryStore;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::config::Config;
use super::enroll::{self, EnrollCommand, EnrollFailure, EnrollOutcome};
use super::health::Health;
use super::history::History;
use super::identity;
use super::ipc::{self, Call};
use super::os::{self, Peer};
use super::policy::PolicyState;
use super::uplink::{Uplink, UplinkStatus, now_ms};

/// What the local API reads from the running agent.
pub struct AgentState {
    config: Config,
    config_path: PathBuf,
    health: Arc<Health>,
    uplink: Arc<Uplink>,
    history: Arc<History>,
}

impl AgentState {
    pub fn new(
        config: Config,
        config_path: PathBuf,
        health: Arc<Health>,
        uplink: Arc<Uplink>,
        history: Arc<History>,
    ) -> Self {
        Self {
            config,
            config_path,
            health,
            uplink,
            history,
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
            uplink: self.uplink.status(),
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
    pub uplink: UplinkStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub config: String,
    pub privd: PrivdStatus,
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

pub async fn serve(
    listener: os::LocalListener,
    state: Arc<AgentState>,
    token: CancellationToken,
) -> Result<(), String> {
    let app = Router::new()
        .route("/v1/status", get(status))
        .route("/v1/enroll", post(enroll_device))
        .route("/v1/policy/reload", post(reload_policy))
        .route("/v1/pause", post(pause))
        .route("/v1/resume", post(resume))
        .route("/v1/history", get(history))
        .route("/v1/history/keep", post(keep_history))
        .route("/v1/history/clear", post(clear_history))
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
    if !peer.is_root() {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "enrolling changes who controls this machine; run {}",
                os::elevated("cntrl enroll")
            ),
        ));
    }
    // One enrollment at a time, and none while the uplink saves a credential.
    let _identity = state.uplink.lock_identity().await;
    let outcome =
        enroll::enroll(&state.config, command)
            .await
            .map_err(|failure| match failure {
                // The CLI asks the user to confirm the move, or says nothing changed.
                EnrollFailure::Refused(error)
                    if matches!(
                        error.code,
                        EnrollErrorCode::ConfirmMove | EnrollErrorCode::AlreadyEnrolled
                    ) =>
                {
                    let body = serde_json::to_string(&error).unwrap_or_else(|_| error.msg.clone());
                    (StatusCode::CONFLICT, body)
                }
                other => (StatusCode::BAD_REQUEST, other.to_string()),
            })?;
    state.uplink.enrolled();
    Ok(Json(outcome))
}

/// `cntrl policy allow` or `deny` changed the policy: reconnect, so the hello carries it.
async fn reload_policy(
    ConnectInfo(peer): ConnectInfo<Peer>,
    State(state): State<Arc<AgentState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !peer.is_root() {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "only {} changes the policy; run it {}",
                os::SUPERUSER,
                os::AS_ROOT
            ),
        ));
    }
    state.uplink.policy_changed();
    Ok(StatusCode::NO_CONTENT)
}

/// What `cntrl pause` sends: who ran it, and why.
#[derive(Debug, Serialize, Deserialize)]
pub struct PauseCommand {
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Whether the gateway recorded a pause; when the link was down, it couldn't.
#[derive(Debug, Serialize, Deserialize)]
pub struct PauseOutcome {
    pub told: bool,
}

/// Whether `cntrl resume` ended a pause.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResumeOutcome {
    pub was_paused: bool,
}

/// The longest name and reason a pause carries, as Console shows them.
const PAUSED_BY_MAX: usize = 64;
const PAUSE_REASON_MAX: usize = 200;

/// `cntrl pause` (D46): the uplink tells the gateway, hangs up, and stays away
/// until `cntrl resume`.
async fn pause(
    ConnectInfo(peer): ConnectInfo<Peer>,
    State(state): State<Arc<AgentState>>,
    Json(command): Json<PauseCommand>,
) -> Result<Json<PauseOutcome>, (StatusCode, String)> {
    if !peer.is_root() {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "pausing cuts Console off from this machine; run {}",
                os::elevated("cntrl pause")
            ),
        ));
    }
    let clip = |text: &str, max: usize| text.trim().chars().take(max).collect::<String>();
    let by = Some(clip(&command.by, PAUSED_BY_MAX))
        .filter(|by| !by.is_empty())
        .unwrap_or_else(|| os::SUPERUSER.to_owned());
    let reason = command
        .reason
        .map(|reason| clip(&reason, PAUSE_REASON_MAX))
        .filter(|reason| !reason.is_empty());
    let told = state
        .uplink
        .pause(&state.config.paths.state_dir, by, reason)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(PauseOutcome { told }))
}

/// `cntrl resume`: the uplink reconnects.
async fn resume(
    ConnectInfo(peer): ConnectInfo<Peer>,
    State(state): State<Arc<AgentState>>,
) -> Result<Json<ResumeOutcome>, (StatusCode, String)> {
    if !peer.is_root() {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "only {} resumes the agent; run {}",
                os::SUPERUSER,
                os::elevated("cntrl resume")
            ),
        ));
    }
    let was_paused = state
        .uplink
        .resume(&state.config.paths.state_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(ResumeOutcome { was_paused }))
}

/// What `cntrl history keep` sends: who ran it, and the days.
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryKeepCommand {
    pub by: String,
    pub days: u32,
}

/// What `cntrl history clear` sends: who ran it.
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryClearCommand {
    pub by: String,
}

/// `cntrl history show`: how many days are kept, since when, and the room
/// it takes (D52).
async fn history(
    State(state): State<Arc<AgentState>>,
) -> Result<Json<HistoryStore>, (StatusCode, String)> {
    let history = Arc::clone(&state.history);
    tokio::task::spawn_blocking(move || history.store())
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// `cntrl history keep <days>`: fewer days deletes the older ones at once.
async fn keep_history(
    ConnectInfo(peer): ConnectInfo<Peer>,
    State(state): State<Arc<AgentState>>,
    Json(command): Json<HistoryKeepCommand>,
) -> Result<Json<HistoryStore>, (StatusCode, String)> {
    if !peer.is_root() {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "only {} changes how much history is kept; run {}",
                os::SUPERUSER,
                os::elevated("cntrl history keep")
            ),
        ));
    }
    let history = Arc::clone(&state.history);
    let days = command.days;
    let store = tokio::task::spawn_blocking(move || history.keep(days, now_ms()))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(refusal)?;
    record(
        &state,
        "history.kept",
        serde_json::json!({ "by": command.by, "days": days }),
    )
    .await;
    Ok(Json(store))
}

/// `cntrl history clear`: deletes all of it.
async fn clear_history(
    ConnectInfo(peer): ConnectInfo<Peer>,
    State(state): State<Arc<AgentState>>,
    Json(command): Json<HistoryClearCommand>,
) -> Result<Json<HistoryStore>, (StatusCode, String)> {
    if !peer.is_root() {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "only {} clears the history; run {}",
                os::SUPERUSER,
                os::elevated("cntrl history clear")
            ),
        ));
    }
    let history = Arc::clone(&state.history);
    let store = tokio::task::spawn_blocking(move || history.clear())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(refusal)?;
    record(
        &state,
        "history.cleared",
        serde_json::json!({ "by": command.by }),
    )
    .await;
    Ok(Json(store))
}

fn refusal(error: ipc::CallError) -> (StatusCode, String) {
    let status = if error.code == ErrorCode::BadRequest {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, error.msg)
}

/// Records a change made on the machine in the audit log. privd may be
/// missing in a run by hand, so a failure here is only a warning.
async fn record(state: &AgentState, kind: &str, data: serde_json::Value) {
    let call = Call::AuditAppend {
        kind: kind.to_owned(),
        data,
    };
    if let Err(e) = ipc::call_once(&state.config.paths.privd_socket, call).await {
        tracing::warn!("couldn't record {kind} in the audit log: {e}");
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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
        let listener = os::LocalListener::listen(&socket, os::Endpoint::Agent).expect("listen");
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
            Arc::new(Uplink::new()),
            Arc::new(History::open(dir)),
        ));
        let token = CancellationToken::new();
        let server = tokio::spawn(serve(listener, state, token.clone()));
        (socket, token, server)
    }

    #[tokio::test]
    async fn status_is_served_over_the_socket() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (socket, token, server) = start(dir.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&socket)
                .expect("socket metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o660);
        }

        let status = client::get_status(&socket).await.expect("status");
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(status.uplink, UplinkStatus::NotEnrolled);
        assert!(!status.privd.reachable);

        token.cancel();
        server
            .await
            .expect("server task")
            .expect("server stopped cleanly");
    }

    #[tokio::test]
    async fn enrolling_needs_a_root_peer() {
        if os::is_root() {
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
