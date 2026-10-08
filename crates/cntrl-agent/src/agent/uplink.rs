//! The uplink: the agent's WebSocket to the gateway. It answers the gateway's
//! challenge with a hello signed by the device key, keeps the link alive with
//! heartbeats, and reconnects with full-jitter backoff. Close codes that mean
//! "stop" (revoked, locked, unsupported version) park it until the device is
//! enrolled again; it never ends the agent. `cntrl pause` parks it too, after
//! telling the gateway, until `cntrl resume` (D46). A device disabled in
//! Console keeps its link but goes quiet until the gateway brings it back, and
//! `cntrl uninstall` tells the gateway before the agent goes (D87).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cntrl_host::HostError;
use cntrl_protocol::auth::{gateway_host, hello_signing_string};
use cntrl_protocol::checks::{CheckResults, DeviceCheck};
use cntrl_protocol::codes::{ErrorCode, close};
use cntrl_protocol::containers::{ContainerRef, ContainersParams, ContainersSample};
use cntrl_protocol::frame::{
    AgentInfo, Disabled, Event, Frame, GoAway, HeartbeatConfig, Hello, HelloAuth, Pause, Records,
    Request, Response, SigAlg, Subscribe, Uninstall, Welcome,
};
use cntrl_protocol::logs::{LogsBatch, LogsParams};
use cntrl_protocol::network::{Listeners, NetworkParams, NetworkSample};
use cntrl_protocol::ops::{self, Topic};
use cntrl_protocol::power::{PowerAction, PowerInfo};
use cntrl_protocol::process::ProcessesParams;
use cntrl_protocol::service::{ServiceAction, ServiceList, ServiceRef, ServiceStatus};
use cntrl_protocol::stats::{StatsParams, StatsSample};
use cntrl_protocol::storage::{DisksHealth, StorageParams, StorageSample};
use cntrl_protocol::{MAX_FRAME_BYTES, PING, PONG, PROTOCOL_VERSION, SUBPROTOCOL};
use futures_util::{SinkExt, StreamExt};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, MutexGuard, Notify, mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use tokio::time::{MissedTickBehavior, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{self, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::alerts::Alerts;
use super::checks;
use super::containers;
use super::docker;
use super::history::History;
use super::host;
use super::identity::{self, DEVICE_KEY_FILE, Identity};
use super::ipc::{self, Call, CallError};
use super::keys::SigningKey;
use super::logs;
use super::network;
use super::outbox::Outbox;
use super::paused::{self, PausedState};
use super::policy::PolicyState;
use super::processes::{self, Table};
use super::stats::Latest;
use super::storage;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the gateway has to send its challenge, then its welcome.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(60);
/// The least wait after the gateway refuses this device's credentials.
const AUTH_RETRY: Duration = Duration::from_secs(300);
/// A session that lasted this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// How long a pause waits for the gateway to record it before hanging up anyway.
const PAUSE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounds on the heartbeat the gateway asks for.
const HEARTBEAT_MIN: Duration = Duration::from_secs(5);
const HEARTBEAT_MAX: Duration = Duration::from_secs(300);

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Where the uplink stands, for `cntrl status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UplinkStatus {
    NotEnrolled,
    Connecting {
        gateway: String,
        attempt: u32,
    },
    Online {
        gateway: String,
        session: String,
        since_ms: u64,
    },
    /// Waiting to reconnect.
    Retrying {
        reason: String,
        retry_at_ms: u64,
    },
    /// Parked until the device is enrolled again.
    Stopped {
        reason: String,
    },
    /// Paused on this machine (`cntrl pause`) until `cntrl resume`.
    Paused {
        by: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        since_ms: u64,
    },
    /// Disabled in Console (D87): the link stays up and quiet until the
    /// organization's plan covers this device again.
    Disabled {
        gateway: String,
        /// Console's words for why.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        since_ms: u64,
    },
}

/// A pause for the online session to tell the gateway about, and where to say
/// whether the gateway recorded it.
struct PauseRequest {
    notice: Pause,
    told: oneshot::Sender<bool>,
}

/// An uninstall for the session to tell the gateway about (D87), and where to
/// say whether the gateway recorded it.
struct UninstallRequest {
    notice: Uninstall,
    told: oneshot::Sender<bool>,
}

/// What the uplink shares with the local API.
pub struct Uplink {
    status: watch::Sender<UplinkStatus>,
    /// Signalled after an enrollment, so the uplink reconnects as the new identity.
    enrolled: Notify,
    /// Signalled after a policy change, so the uplink reconnects and Console
    /// sees the new policy in the hello.
    policy_changed: Notify,
    /// Held while the identity file is written.
    identity: Mutex<()>,
    /// A pause waiting for the online session, which `pausing` wakes.
    pause: std::sync::Mutex<Option<PauseRequest>>,
    pausing: Notify,
    /// Signalled by `cntrl resume`.
    resumed: Notify,
    /// An uninstall waiting for the session, which `uninstalling` wakes.
    uninstall: std::sync::Mutex<Option<UninstallRequest>>,
    uninstalling: Notify,
}

impl Uplink {
    pub fn new() -> Self {
        Self {
            status: watch::Sender::new(UplinkStatus::NotEnrolled),
            enrolled: Notify::new(),
            policy_changed: Notify::new(),
            identity: Mutex::new(()),
            pause: std::sync::Mutex::new(None),
            pausing: Notify::new(),
            resumed: Notify::new(),
            uninstall: std::sync::Mutex::new(None),
            uninstalling: Notify::new(),
        }
    }

    /// Pauses the agent (`cntrl pause`): saves the pause, so it holds across
    /// restarts, then has the online session tell the gateway and hang up. True
    /// when the gateway recorded it; false when the link was down, and Console
    /// sees the device go offline instead.
    pub async fn pause(
        &self,
        state_dir: &Path,
        by: String,
        reason: Option<String>,
    ) -> Result<bool, String> {
        let state = PausedState {
            by: by.clone(),
            reason: reason.clone(),
            since_ms: now_ms(),
        };
        paused::save(state_dir, &state).map_err(|e| format!("can't save the pause: {e}"))?;
        let (told, answer) = oneshot::channel();
        *self.pause.lock().unwrap_or_else(PoisonError::into_inner) = Some(PauseRequest {
            notice: Pause { by, reason },
            told,
        });
        self.pausing.notify_one();
        Ok(timeout(PAUSE_TIMEOUT.saturating_mul(2), answer)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false))
    }

    /// Ends a pause (`cntrl resume`), and the uplink reconnects. False when the
    /// agent wasn't paused.
    pub fn resume(&self, state_dir: &Path) -> Result<bool, String> {
        let was = paused::clear(state_dir).map_err(|e| format!("can't clear the pause: {e}"))?;
        self.resumed.notify_one();
        Ok(was)
    }

    /// The pause waiting for the session, if any.
    fn take_pause(&self) -> Option<PauseRequest> {
        self.pause
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Tells Console the agent is being uninstalled (`cntrl uninstall`, D87):
    /// the session sends it and hangs up, and the uplink stays down until the
    /// agent stops. True when the gateway recorded it; false when the link was
    /// down, or the gateway didn't answer, as one from before D87 doesn't.
    pub async fn uninstall(&self, by: String) -> bool {
        let (told, answer) = oneshot::channel();
        *self
            .uninstall
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(UninstallRequest {
            notice: Uninstall { by },
            told,
        });
        self.uninstalling.notify_one();
        timeout(PAUSE_TIMEOUT.saturating_mul(2), answer)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false)
    }

    /// The uninstall waiting for the session, if any.
    fn take_uninstall(&self) -> Option<UninstallRequest> {
        self.uninstall
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Serializes writes to the identity file: enrollments and credential renewals.
    pub async fn lock_identity(&self) -> MutexGuard<'_, ()> {
        self.identity.lock().await
    }

    pub fn status(&self) -> UplinkStatus {
        self.status.borrow().clone()
    }

    /// Tells the uplink the identity changed.
    pub fn enrolled(&self) {
        self.enrolled.notify_one();
    }

    /// Tells the uplink the policy changed.
    pub fn policy_changed(&self) {
        self.policy_changed.notify_one();
    }

    fn set(&self, status: UplinkStatus) {
        // `send` fails when nobody holds a receiver; the status must stick anyway.
        self.status.send_replace(status);
    }
}

/// What the uplink reads from the configuration, and the stats it serves.
pub struct UplinkConfig {
    pub state_dir: PathBuf,
    pub privd_socket: PathBuf,
    /// Replaces the gateway URL from enrollment.
    pub gateway_url: Option<String>,
    /// The sampler's latest sample, for the `stats` topic.
    pub stats: Arc<Latest>,
    /// The latest process table, for the `processes` topic.
    pub processes: Arc<processes::Latest>,
    /// The latest interfaces, for the `network` topic.
    pub network: Arc<network::Latest>,
    /// The latest disks and volumes, for the `storage` topic.
    pub storage: Arc<storage::Latest>,
    /// The latest containers, for the `containers` topic (D54).
    pub containers: Arc<containers::Latest>,
    /// Records for Console, sent and acknowledged over the link.
    pub outbox: Arc<Outbox>,
    /// The alert rules this device decides itself, which the hub sends (D43).
    pub alerts: Arc<Alerts>,
    /// The history this device keeps, for `history.*` (D52).
    pub history: Arc<History>,
}

/// How a session ended, and what to do next.
enum End {
    Shutdown,
    /// The identity or the policy changed: reconnect now.
    Reconnect,
    /// Paused on this machine: park until resumed.
    Paused,
    Retry {
        reason: String,
        at_least: Duration,
        /// Whether the session was up long enough to reset the backoff.
        stable: bool,
    },
    /// Don't reconnect until the device is enrolled again.
    Stop {
        reason: String,
    },
    /// The gateway rejected the connection's credentials (close 4001).
    Rejected {
        stable: bool,
    },
    /// The gateway disabled the device mid-session: the session goes quiet.
    Disabled(Box<Disabled>),
    /// Told the gateway the agent is being uninstalled: stay down.
    Uninstalled,
}

fn retry(reason: impl Into<String>) -> End {
    End::Retry {
        reason: reason.into(),
        at_least: Duration::ZERO,
        stable: false,
    }
}

/// Runs the uplink until shutdown.
pub async fn run(
    config: UplinkConfig,
    uplink: Arc<Uplink>,
    token: CancellationToken,
) -> Result<(), String> {
    let mut attempt: u32 = 0;
    loop {
        if let Some(state) = paused::load(&config.state_dir) {
            // Nobody is online to tell; a session that was told the gateway already.
            if let Some(request) = uplink.take_pause() {
                let _ = request.told.send(false);
            }
            info!(by = %state.by, "uplink paused; `cntrl resume` reconnects");
            uplink.set(UplinkStatus::Paused {
                by: state.by,
                reason: state.reason,
                since_ms: state.since_ms,
            });
            tokio::select! {
                () = uplink.resumed.notified() => {}
                // Nobody is online to tell.
                () = uplink.uninstalling.notified() => {
                    if let Some(request) = uplink.take_uninstall() {
                        let _ = request.told.send(false);
                        return gone(&uplink, &token).await;
                    }
                }
                () = token.cancelled() => return Ok(()),
            }
            attempt = 0;
            continue;
        }
        let identity = match identity::load(&config.state_dir) {
            Ok(Some(identity)) => identity,
            Ok(None) => {
                uplink.set(UplinkStatus::NotEnrolled);
                if !park(&uplink, &token).await {
                    return Ok(());
                }
                continue;
            }
            Err(e) => {
                warn!("uplink stopped: {e}");
                uplink.set(UplinkStatus::Stopped { reason: e });
                if !park(&uplink, &token).await {
                    return Ok(());
                }
                continue;
            }
        };
        let url = config
            .gateway_url
            .clone()
            .unwrap_or_else(|| identity.gateway_url.clone());
        uplink.set(UplinkStatus::Connecting {
            gateway: url.clone(),
            attempt,
        });

        let retry = match session(&config, &identity, &url, &uplink, &token).await {
            End::Shutdown => return Ok(()),
            // A session meets `Disabled` only to go quiet in place; it never ends one.
            End::Reconnect | End::Paused | End::Disabled(_) => {
                attempt = 0;
                continue;
            }
            End::Uninstalled => return gone(&uplink, &token).await,
            End::Stop { reason } => {
                warn!("uplink stopped: {reason}");
                uplink.set(UplinkStatus::Stopped { reason });
                if !park(&uplink, &token).await {
                    return Ok(());
                }
                attempt = 0;
                continue;
            }
            // A stale credential, such as one naming a workspace the device left,
            // is dropped, and the agent reconnects at once through the lookup.
            End::Rejected { .. } if identity.credential.is_some() => {
                info!("the gateway rejected the connection credential; reconnecting without it");
                save_credential(&config.state_dir, &uplink, &identity, None).await;
                continue;
            }
            End::Rejected { stable } => (
                "the gateway rejected this device's credentials".to_owned(),
                AUTH_RETRY,
                stable,
            ),
            End::Retry {
                reason,
                at_least,
                stable,
            } => (reason, at_least, stable),
        };

        let (reason, at_least, stable) = retry;
        if stable {
            attempt = 0;
        }
        let delay = backoff(attempt).max(at_least);
        attempt = attempt.saturating_add(1);
        info!(retry_in_s = delay.as_secs(), "uplink down: {reason}");
        uplink.set(UplinkStatus::Retrying {
            reason,
            retry_at_ms: now_ms().saturating_add(millis(delay)),
        });
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = uplink.enrolled.notified() => attempt = 0,
            // Paused while away: the loop parks without waiting out the backoff.
            () = uplink.pausing.notified() => {}
            // Uninstalled while away: nobody to tell, and nothing to come back to.
            () = uplink.uninstalling.notified() => {
                if let Some(request) = uplink.take_uninstall() {
                    let _ = request.told.send(false);
                    return gone(&uplink, &token).await;
                }
            }
            () = token.cancelled() => return Ok(()),
        }
    }
}

/// After an uninstall: the link stays down until the agent stops.
async fn gone(uplink: &Uplink, token: &CancellationToken) -> Result<(), String> {
    info!("uninstalling; the uplink stays down until the agent stops");
    uplink.set(UplinkStatus::Stopped {
        reason: "uninstalling".to_owned(),
    });
    token.cancelled().await;
    Ok(())
}

/// Waits for an enrollment; false on shutdown. An uninstall meanwhile has
/// nobody to tell.
async fn park(uplink: &Uplink, token: &CancellationToken) -> bool {
    loop {
        tokio::select! {
            () = uplink.enrolled.notified() => return true,
            () = uplink.uninstalling.notified() => {
                if let Some(request) = uplink.take_uninstall() {
                    let _ = request.told.send(false);
                }
            }
            () = token.cancelled() => return false,
        }
    }
}

async fn session(
    config: &UplinkConfig,
    identity: &Identity,
    url: &str,
    uplink: &Uplink,
    token: &CancellationToken,
) -> End {
    let key = match SigningKey::load(&config.state_dir.join(DEVICE_KEY_FILE)) {
        Ok(key) => key,
        Err(e) => {
            return End::Stop {
                reason: format!("can't load the device key: {e}"),
            };
        }
    };
    let Some(host) = gateway_host(url) else {
        return End::Stop {
            reason: format!("{url} isn't a gateway URL"),
        };
    };
    let request = match connect_request(url, &identity.device_id, identity.credential.as_deref()) {
        Ok(request) => request,
        Err(reason) => return End::Stop { reason },
    };

    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_BYTES as usize))
        .max_frame_size(Some(MAX_FRAME_BYTES as usize));
    let connect = tokio_tungstenite::connect_async_with_config(request, Some(ws_config), true);
    let mut ws = tokio::select! {
        connected = timeout(CONNECT_TIMEOUT, connect) => match connected {
            Err(_) => return retry("the gateway didn't answer in time"),
            Ok(Err(WsError::Http(response))) => return refused(&response),
            Ok(Err(e)) => return retry(format!("can't reach the gateway: {e}")),
            Ok(Ok((ws, _))) => ws,
        },
        () = token.cancelled() => return End::Shutdown,
    };

    let challenge = match timeout(HANDSHAKE_TIMEOUT, next_frame(&mut ws)).await {
        Ok(Ok(Frame::Challenge(challenge))) => challenge,
        Ok(Ok(_)) => return retry("the gateway sent something other than a challenge"),
        Ok(Err(end)) => return end,
        Err(_) => return retry("the gateway sent no challenge"),
    };
    let signed = hello_signing_string(
        &challenge.sid,
        &challenge.nonce,
        &host,
        &identity.device_id,
        &identity.key_id,
        identity.generation,
    );
    let sig = match key.sign(signed.as_bytes()) {
        Ok(sig) => sig,
        Err(e) => {
            return End::Stop {
                reason: format!("can't sign the hello: {e}"),
            };
        }
    };
    let policy = policy(&config.privd_socket).await;
    let hello = Hello {
        v: PROTOCOL_VERSION,
        agent: AgentInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            target: host::TARGET.to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            boot_id: host::boot_id(),
            machine_id_hash: host::machine_id_hash(),
        },
        auth: HelloAuth {
            device_id: identity.device_id.clone(),
            key_id: identity.key_id.clone(),
            alg: SigAlg::Es256,
            generation: identity.generation,
            sig,
        },
        caps: policy.caps(),
        policy: policy.summary(),
        outbox: config.outbox.state().await,
    };
    if let Err(end) = send(&mut ws, &Frame::Hello(Box::new(hello))).await {
        return end;
    }
    let welcome = match timeout(HANDSHAKE_TIMEOUT, next_frame(&mut ws)).await {
        Ok(Ok(Frame::Welcome(welcome))) => welcome,
        Ok(Ok(Frame::Disabled(disabled))) => {
            let beat = Beat::of(&disabled.hb);
            let started = Instant::now();
            return quiet(
                &mut ws,
                &disabled,
                beat,
                started,
                url,
                &config.alerts,
                uplink,
                token,
            )
            .await;
        }
        Ok(Ok(_)) => return retry("the gateway sent something other than a welcome"),
        Ok(Err(end)) => return end,
        Err(_) => return retry("the gateway sent no welcome"),
    };

    if let Some(credential) = &welcome.credential {
        save_credential(
            &config.state_dir,
            uplink,
            identity,
            Some(credential.clone()),
        )
        .await;
    }
    // On, as ever or once more: the alert rules judge again (D87).
    config.alerts.rest(false).await;
    info!(session = %welcome.session, gateway = url, "uplink online");
    uplink.set(UplinkStatus::Online {
        gateway: url.to_owned(),
        session: welcome.session.clone(),
        since_ms: now_ms(),
    });
    online(
        &mut ws,
        &welcome,
        Arc::new(policy),
        url,
        config,
        uplink,
        token,
    )
    .await
}

/// The connected session: heartbeats, and frames from Console.
async fn online(
    ws: &mut Ws,
    welcome: &Welcome,
    policy: Arc<PolicyState>,
    gateway: &str,
    config: &UplinkConfig,
    uplink: &Uplink,
    token: &CancellationToken,
) -> End {
    let Beat {
        interval,
        pong_timeout,
    } = Beat::of(&welcome.hb);
    let started = Instant::now();
    let stable = || started.elapsed() >= STABLE_AFTER;
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_pong = Instant::now();
    let (answers, mut answered) = mpsc::channel(ANSWER_QUEUE);
    let (check_results, mut checked) = mpsc::channel(CHECK_QUEUE);
    let outbox = Arc::clone(&config.outbox);
    let mut session = Session {
        requests: Requests::new(answers, welcome.limits.max_inflight as usize),
        subs: Subscriptions::new(config.privd_socket.clone()),
        checks: checks::Runner::new(check_results),
        policy,
        config,
        in_flight: None,
        batch: (welcome.limits.max_rec_batch as usize).max(1),
    };
    // Console already has these; the rest go out again.
    if let Some(upto) = welcome.acked_upto {
        outbox.ack(upto).await;
    }
    if let Err(end) = session.send_records(ws).await {
        return end;
    }
    // A reconnecting session gets its subscriptions back in the welcome.
    for subscribe in welcome.subs.iter().cloned() {
        let samplers = Samplers {
            stats: &session.config.stats,
            processes: &session.config.processes,
            network: &session.config.network,
            storage: &session.config.storage,
            containers: &session.config.containers,
        };
        if let Err(end) = session
            .subs
            .open(ws, subscribe, &session.policy, &samplers)
            .await
        {
            return end;
        }
    }

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if last_pong.elapsed() > pong_timeout {
                    close_link(ws, close::HEARTBEAT_TIMEOUT, "heartbeat timeout").await;
                    return End::Retry {
                        reason: "the gateway stopped answering heartbeats".to_owned(),
                        at_least: Duration::ZERO,
                        stable: stable(),
                    };
                }
                if let Err(e) = ws.send(Message::text(PING)).await {
                    return End::Retry {
                        reason: format!("link lost: {e}"),
                        at_least: Duration::ZERO,
                        stable: stable(),
                    };
                }
                // Sends a batch again if its ack is overdue.
                if let Err(end) = session.send_records(ws).await {
                    return end;
                }
            }
            message = ws.next() => match message {
                Some(Ok(Message::Text(text))) => match text.as_str() {
                    PONG => last_pong = Instant::now(),
                    PING => {
                        if let Err(e) = ws.send(Message::text(PONG)).await {
                            debug!("can't answer a ping: {e}");
                        }
                    }
                    json => {
                        if let Some(end) = handle(ws, json, stable(), &mut session).await {
                            let End::Disabled(disabled) = end else { return end };
                            // What the session served stops here: subscriptions,
                            // checks and requests.
                            drop(session);
                            let beat = Beat::of(&disabled.hb);
                            return quiet(ws, &disabled, beat, started, gateway, &config.alerts, uplink, token).await;
                        }
                    }
                },
                Some(Ok(Message::Close(frame))) => return after_close(frame, stable()),
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    return End::Retry {
                        reason: format!("link lost: {e}"),
                        at_least: Duration::ZERO,
                        stable: stable(),
                    };
                }
                None => {
                    return End::Retry {
                        reason: "the gateway closed the connection".to_owned(),
                        at_least: Duration::ZERO,
                        stable: stable(),
                    };
                }
            },
            Some(sample) = next_value(&mut session.subs.stats), if session.subs.stats.is_some() => {
                if let Err(end) = session.subs.publish(ws, &sample).await {
                    return end;
                }
            }
            Some(table) = next_value(&mut session.subs.processes), if session.subs.processes.is_some() => {
                if let Err(end) = session.subs.publish_table(ws, &table, &session.policy).await {
                    return end;
                }
            }
            Some(sample) = next_value(&mut session.subs.network), if session.subs.network.is_some() => {
                let network = |topic: &Watching| matches!(topic, Watching::Network);
                if let Err(end) = session.subs.publish_to(ws, network, sample.ts, &*sample).await {
                    return end;
                }
            }
            Some(sample) = next_value(&mut session.subs.storage), if session.subs.storage.is_some() => {
                let storage = |topic: &Watching| matches!(topic, Watching::Storage);
                if let Err(end) = session.subs.publish_to(ws, storage, sample.ts, &*sample).await {
                    return end;
                }
            }
            Some(sample) = next_value(&mut session.subs.containers), if session.subs.containers.is_some() => {
                let containers = |topic: &Watching| matches!(topic, Watching::Containers);
                if let Err(end) = session.subs.publish_to(ws, containers, sample.ts, &*sample).await {
                    return end;
                }
            }
            Some((id, batch)) = session.subs.log_batches.recv() => {
                if let Err(end) = session.subs.publish_logs(ws, &id, &batch).await {
                    return end;
                }
            }
            () = outbox.added.notified() => {
                if let Err(end) = session.send_records(ws).await {
                    return end;
                }
            }
            Some(answer) = answered.recv() => {
                if let Some(answer) = session.requests.finish(answer)
                    && let Err(end) = send(ws, &Frame::Res(answer)).await
                {
                    return end;
                }
            }
            Some(result) = checked.recv() => {
                // Results that came together go together.
                let mut results = vec![result];
                while results.len() < CHECK_QUEUE {
                    match checked.try_recv() {
                        Ok(more) => results.push(more),
                        Err(_) => break,
                    }
                }
                let frame = Frame::CheckResults(CheckResults { results, refused: None });
                if let Err(end) = send(ws, &frame).await {
                    return end;
                }
            }
            () = uplink.enrolled.notified() => {
                close_link(ws, close::DISCONNECTED_BY_DEVICE, "enrolled again").await;
                return End::Reconnect;
            }
            () = uplink.policy_changed.notified() => {
                close_link(ws, close::DISCONNECTED_BY_DEVICE, "policy changed").await;
                return End::Reconnect;
            }
            () = uplink.pausing.notified() => {
                if let Some(request) = uplink.take_pause() {
                    return pause(ws, request).await;
                }
            }
            () = uplink.uninstalling.notified() => {
                if let Some(request) = uplink.take_uninstall() {
                    return goodbye(ws, request).await;
                }
            }
            () = token.cancelled() => {
                close_link(ws, close::RESTARTING, "agent stopping").await;
                return End::Shutdown;
            }
        }
    }
}

/// The heartbeat the gateway asks for, within bounds.
#[derive(Debug, Clone, Copy)]
struct Beat {
    interval: Duration,
    pong_timeout: Duration,
}

impl Beat {
    fn of(hb: &HeartbeatConfig) -> Self {
        let interval =
            Duration::from_millis(u64::from(hb.interval_ms)).clamp(HEARTBEAT_MIN, HEARTBEAT_MAX);
        let pong_timeout = Duration::from_millis(u64::from(hb.timeout_ms))
            .clamp(interval, HEARTBEAT_MAX.saturating_mul(2));
        Self {
            interval,
            pong_timeout,
        }
    }
}

/// What the agent tells the gateway it understands, as it connects (D87).
const FEATURES: &str = "disabled";

/// The longest message from Console that `cntrl status` shows.
const DISABLED_MESSAGE_MAX: usize = 200;
/// What a request gets while the device is disabled.
const DISABLED_REFUSAL: &str = "this device is disabled in Console";

/// Disabled in Console (D87): the link stays up and quiet, heartbeats only,
/// until the gateway closes it to bring the device back. The alert rules rest
/// meanwhile; requests are refused; a pause or an uninstall still gets told.
#[allow(clippy::too_many_arguments)]
async fn quiet(
    ws: &mut Ws,
    disabled: &Disabled,
    beat: Beat,
    started: Instant,
    gateway: &str,
    alerts: &Alerts,
    uplink: &Uplink,
    token: &CancellationToken,
) -> End {
    info!("disabled in Console; the link stays quiet until the plan covers this device again");
    uplink.set(UplinkStatus::Disabled {
        gateway: gateway.to_owned(),
        message: shown(disabled.message.as_deref()),
        since_ms: now_ms(),
    });
    alerts.rest(true).await;
    let stable = || started.elapsed() >= STABLE_AFTER;
    let mut ticker =
        tokio::time::interval_at(tokio::time::Instant::now() + beat.interval, beat.interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_pong = Instant::now();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if last_pong.elapsed() > beat.pong_timeout {
                    close_link(ws, close::HEARTBEAT_TIMEOUT, "heartbeat timeout").await;
                    return End::Retry {
                        reason: "the gateway stopped answering heartbeats".to_owned(),
                        at_least: Duration::ZERO,
                        stable: stable(),
                    };
                }
                if let Err(e) = ws.send(Message::text(PING)).await {
                    return End::Retry {
                        reason: format!("link lost: {e}"),
                        at_least: Duration::ZERO,
                        stable: stable(),
                    };
                }
            }
            message = ws.next() => match message {
                Some(Ok(Message::Text(text))) => match text.as_str() {
                    PONG => last_pong = Instant::now(),
                    PING => {
                        if let Err(e) = ws.send(Message::text(PONG)).await {
                            debug!("can't answer a ping: {e}");
                        }
                    }
                    json => {
                        if let Some(end) = while_disabled(ws, json, stable(), uplink).await {
                            return end;
                        }
                    }
                },
                Some(Ok(Message::Close(frame))) => return after_close(frame, stable()),
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    return End::Retry {
                        reason: format!("link lost: {e}"),
                        at_least: Duration::ZERO,
                        stable: stable(),
                    };
                }
                None => {
                    return End::Retry {
                        reason: "the gateway closed the connection".to_owned(),
                        at_least: Duration::ZERO,
                        stable: stable(),
                    };
                }
            },
            () = uplink.enrolled.notified() => {
                close_link(ws, close::DISCONNECTED_BY_DEVICE, "enrolled again").await;
                return End::Reconnect;
            }
            () = uplink.policy_changed.notified() => {
                close_link(ws, close::DISCONNECTED_BY_DEVICE, "policy changed").await;
                return End::Reconnect;
            }
            () = uplink.pausing.notified() => {
                if let Some(request) = uplink.take_pause() {
                    return pause(ws, request).await;
                }
            }
            () = uplink.uninstalling.notified() => {
                if let Some(request) = uplink.take_uninstall() {
                    return goodbye(ws, request).await;
                }
            }
            () = token.cancelled() => {
                close_link(ws, close::RESTARTING, "agent stopping").await;
                return End::Shutdown;
            }
        }
    }
}

/// A frame from Console while disabled. Requests and subscriptions are
/// refused, so a hub that hasn't caught up doesn't wait on them; a goaway is
/// honoured; a new word on why is shown; anything else waits for the device to
/// be back, when the hub sends it again.
async fn while_disabled(ws: &mut Ws, json: &str, stable: bool, uplink: &Uplink) -> Option<End> {
    let frame = match serde_json::from_str::<Frame>(json) {
        Ok(frame) => frame,
        Err(e) => {
            debug!("ignoring an unreadable frame: {e}");
            return None;
        }
    };
    let refuse =
        |id: String| Frame::Res(Response::err(id, ErrorCode::PolicyDenied, DISABLED_REFUSAL));
    let reply = match frame {
        Frame::Req(request) => refuse(request.id),
        Frame::Sub(subscribe) => refuse(subscribe.id),
        Frame::Goaway(goaway) => return Some(go_away(ws, &goaway, stable).await),
        Frame::Disabled(disabled) => {
            if let UplinkStatus::Disabled {
                gateway, since_ms, ..
            } = uplink.status()
            {
                uplink.set(UplinkStatus::Disabled {
                    gateway,
                    message: shown(disabled.message.as_deref()),
                    since_ms,
                });
            }
            return None;
        }
        other => {
            debug!(?other, "disabled; ignoring a frame");
            return None;
        }
    };
    send(ws, &reply).await.err()
}

/// Console's words as `cntrl status` may print them to a terminal: without
/// control or direction-changing characters, and not too long.
fn shown(text: Option<&str>) -> Option<String> {
    let kept: String = text?
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(DISABLED_MESSAGE_MAX)
        .collect();
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.to_owned())
}

/// Tells the gateway who paused the agent and why, waits for it to record the
/// pause, and hangs up.
async fn pause(ws: &mut Ws, request: PauseRequest) -> End {
    let told = send(ws, &Frame::Pause(request.notice)).await.is_ok()
        && answered(ws, |frame| matches!(frame, Frame::Paused(_))).await;
    let _ = request.told.send(told);
    close_link(ws, close::DISCONNECTED_BY_DEVICE, "paused").await;
    End::Paused
}

/// Tells the gateway the agent is being uninstalled, waits for it to record
/// that, and hangs up (D87).
async fn goodbye(ws: &mut Ws, request: UninstallRequest) -> End {
    let told = send(ws, &Frame::Uninstall(request.notice)).await.is_ok()
        && answered(ws, |frame| matches!(frame, Frame::Uninstalled(_))).await;
    let _ = request.told.send(told);
    close_link(ws, close::DISCONNECTED_BY_DEVICE, "uninstalled").await;
    End::Uninstalled
}

/// Whether the gateway answers as `is_answer` expects before the pause times
/// out. The session is ending, so anything else it sends goes unanswered.
async fn answered(ws: &mut Ws, is_answer: impl Fn(&Frame) -> bool) -> bool {
    let wait = async {
        while let Some(message) = ws.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    if serde_json::from_str::<Frame>(&text).is_ok_and(|frame| is_answer(&frame)) {
                        return true;
                    }
                }
                Ok(Message::Close(_)) | Err(_) => return false,
                Ok(_) => {}
            }
        }
        false
    };
    timeout(PAUSE_TIMEOUT, wait).await.unwrap_or(false)
}

/// What an online session serves from and keeps track of.
struct Session<'a> {
    requests: Requests,
    subs: Subscriptions,
    /// The checks the hub handed this device, running while the link is up (D56).
    checks: checks::Runner,
    /// The policy the hello reported; a change reconnects.
    policy: Arc<PolicyState>,
    config: &'a UplinkConfig,
    /// The outbox batch awaiting Console's ack. One at a time, so an ack never
    /// covers a batch Console didn't store.
    in_flight: Option<InFlight>,
    /// Records per batch: the hub's `limits.max_rec_batch`.
    batch: usize,
}

/// A batch of outbox records sent and not yet acknowledged.
struct InFlight {
    upto: u64,
    sent: Instant,
}

/// How long a batch waits for its ack before it goes out again.
const ACK_TIMEOUT: Duration = Duration::from_secs(30);

impl Session<'_> {
    /// Sends the next batch of outbox records, unless one is still awaiting
    /// its ack.
    async fn send_records(&mut self, ws: &mut Ws) -> Result<(), End> {
        if self
            .in_flight
            .as_ref()
            .is_some_and(|batch| batch.sent.elapsed() < ACK_TIMEOUT)
        {
            return Ok(());
        }
        let recs = self.config.outbox.pending(self.batch).await;
        let Some(upto) = recs.last().map(|record| record.seq) else {
            self.in_flight = None;
            return Ok(());
        };
        send(ws, &Frame::Rec(Records { recs })).await?;
        self.in_flight = Some(InFlight {
            upto,
            sent: Instant::now(),
        });
        Ok(())
    }

    /// Runs the checks the hub sent, if the policy allows them, and audits a
    /// changed set once, with its targets: the runs themselves aren't
    /// requests, so they aren't audited. A refused set is answered with why.
    async fn set_checks(&mut self, ws: &mut Ws, checks: Vec<DeviceCheck>) -> Result<(), End> {
        let privd = &self.config.privd_socket;
        if !self.policy.allows(CHECKS_CAPABILITY) {
            let stopped = self.checks.set(Vec::new()).unwrap_or(false);
            if checks.is_empty() && !stopped {
                return Ok(());
            }
            let reason = format!("the device policy doesn't allow {CHECKS_CAPABILITY}");
            let record = serde_json::json!({ "checks": checks.len(), "reason": reason });
            audit(privd, "checks.denied", record).await;
            let frame = Frame::CheckResults(CheckResults {
                results: Vec::new(),
                refused: Some(reason),
            });
            return send(ws, &frame).await;
        }
        match self.checks.set(checks) {
            Ok(true) => {
                let record = serde_json::json!({ "checks": self.checks.summary() });
                audit(privd, "checks.set", record).await;
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(reason) => {
                warn!("can't run checks: {reason}");
                let frame = Frame::CheckResults(CheckResults {
                    results: Vec::new(),
                    refused: Some(reason),
                });
                send(ws, &frame).await
            }
        }
    }

    /// Console has every record up to `upto`: drop them, and send what's next.
    async fn acked(&mut self, ws: &mut Ws, upto: u64) -> Result<(), End> {
        self.config.outbox.ack(upto).await;
        if self
            .in_flight
            .as_ref()
            .is_some_and(|batch| batch.upto <= upto)
        {
            self.in_flight = None;
        }
        self.send_records(ws).await
    }
}

/// Handles a frame from Console; `Some` ends the session.
async fn handle(ws: &mut Ws, json: &str, stable: bool, session: &mut Session<'_>) -> Option<End> {
    let frame = match serde_json::from_str::<Frame>(json) {
        Ok(frame) => frame,
        Err(e) => {
            debug!("ignoring an unreadable frame: {e}");
            return None;
        }
    };
    let reply = match frame {
        Frame::Req(request) => {
            let privd = &session.config.privd_socket;
            let history = &session.config.history;
            session
                .requests
                .start(request, &session.policy, privd, history)?
        }
        Frame::Cancel(cancel) => session.requests.cancel(&cancel.id)?,
        Frame::Ack(ack) => return session.acked(ws, ack.upto).await.err(),
        Frame::Sub(subscribe) => {
            let policy = &session.policy;
            let samplers = Samplers {
                stats: &session.config.stats,
                processes: &session.config.processes,
                network: &session.config.network,
                storage: &session.config.storage,
                containers: &session.config.containers,
            };
            return session
                .subs
                .open(ws, subscribe, policy, &samplers)
                .await
                .err();
        }
        Frame::Unsub(unsubscribe) => {
            session.subs.close(&unsubscribe.id);
            return None;
        }
        Frame::Goaway(goaway) => return Some(go_away(ws, &goaway, stable).await),
        Frame::Alerts(rules) => {
            session.config.alerts.set(rules.rules).await;
            return None;
        }
        Frame::Checks(set) => return session.set_checks(ws, set.checks).await.err(),
        Frame::Disabled(disabled) => return Some(End::Disabled(Box::new(disabled))),
        other => {
            debug!(?other, "ignoring a frame");
            return None;
        }
    };
    send(ws, &Frame::Res(reply)).await.err()
}

/// How many finished requests may wait for the session to send their answers.
const ANSWER_QUEUE: usize = 64;
/// How many check results may wait to go, and the most in one frame.
const CHECK_QUEUE: usize = 64;
/// What a device's policy allows for it to run checks (D56).
const CHECKS_CAPABILITY: &str = "checks.run";
/// Bounds for a request's `deadline_ms`.
const DEADLINE_MIN: Duration = Duration::from_secs(1);
const DEADLINE_MAX: Duration = Duration::from_secs(600);

/// The session's running requests. Each runs in its own task and hands its
/// answer back through a channel, so a slow operation never holds up the link.
struct Requests {
    running: HashMap<String, AbortHandle>,
    answers: mpsc::Sender<Response>,
    /// The most that may run at once: the hub's `limits.max_inflight`.
    limit: usize,
}

impl Requests {
    fn new(answers: mpsc::Sender<Response>, limit: usize) -> Self {
        Self {
            running: HashMap::new(),
            answers,
            limit,
        }
    }

    /// Starts a request. Refusals that need no work come back at once.
    fn start(
        &mut self,
        request: Request,
        policy: &Arc<PolicyState>,
        privd: &Path,
        history: &Arc<History>,
    ) -> Option<Response> {
        let refuse = |code, msg: String| Some(Response::err(request.id.clone(), code, msg));
        if self.running.contains_key(&request.id) {
            return refuse(
                ErrorCode::BadRequest,
                format!("{} is already running", request.id),
            );
        }
        if self.running.len() >= self.limit {
            return refuse(
                ErrorCode::Busy,
                format!("{} requests are running", self.limit),
            );
        }
        if request.ver != 1 {
            let msg = format!("{} has no version {}", request.op, request.ver);
            return refuse(ErrorCode::UnsupportedVersion, msg);
        }
        let call = match ops::Call::decode(&request.op, request.data.clone()) {
            Ok(call) => call,
            Err(e) => return refuse(e.code(), e.to_string()),
        };
        let id = request.id.clone();
        let (answers, policy, privd) = (self.answers.clone(), Arc::clone(policy), privd.to_owned());
        let history = Arc::clone(history);
        let task = tokio::spawn(async move {
            let answer = answer(request, call, &policy, &privd, &history).await;
            // A closed channel means the session ended; nobody is waiting.
            let _ = answers.send(answer).await;
        });
        self.running.insert(id, task.abort_handle());
        None
    }

    /// Cancels a running request; a finished or unknown one is ignored.
    fn cancel(&mut self, id: &str) -> Option<Response> {
        let task = self.running.remove(id)?;
        task.abort();
        Some(Response::err(id, ErrorCode::Cancelled, "cancelled"))
    }

    /// A finished request's answer, unless it was cancelled meanwhile.
    fn finish(&mut self, answer: Response) -> Option<Response> {
        self.running.remove(&answer.id).map(|_| answer)
    }
}

impl Drop for Requests {
    fn drop(&mut self) {
        // The session is over, so the answers would have nowhere to go. privd
        // finishes and audits whatever it already started.
        for task in self.running.values() {
            task.abort();
        }
    }
}

/// Starts, stops or restarts a container through privd, which checks the
/// policy and audits it (D54).
async fn act_on_container(
    request: &Request,
    target: ContainerRef,
    action: docker::Action,
    privd: &Path,
    limit: Duration,
) -> Result<serde_json::Value, CallError> {
    let call = Call::ContainerAct {
        request_id: request.id.clone(),
        container: target.id,
        action,
        actor: request.actor.clone(),
    };
    ipc::call_within(privd, call, limit.max(docker::ACTION_LIMIT)).await
}

/// Runs one request within its deadline.
async fn answer(
    request: Request,
    call: ops::Call,
    policy: &PolicyState,
    privd: &Path,
    history: &Arc<History>,
) -> Response {
    let limit =
        Duration::from_millis(u64::from(request.deadline_ms)).clamp(DEADLINE_MIN, DEADLINE_MAX);
    match timeout(
        limit,
        execute(&request, call, policy, privd, history, limit),
    )
    .await
    {
        Ok(Ok(data)) => Response::ok(request.id, data),
        Ok(Err(e)) => Response::err(request.id, e.code, e.msg),
        Err(_) => Response::err(
            request.id,
            ErrorCode::Timeout,
            format!("no result within {limit:?}"),
        ),
    }
}

/// Checks the policy, then runs the operation. Every operation needs an arm
/// here, so a new one doesn't compile until it's handled.
async fn execute(
    request: &Request,
    call: ops::Call,
    policy: &PolicyState,
    privd: &Path,
    history: &Arc<History>,
    limit: Duration,
) -> Result<serde_json::Value, CallError> {
    let summary = serde_json::json!({ "id": request.id, "op": request.op, "actor": request.actor });
    let capability = call.capability();
    if !policy.allows(capability) {
        let reason = format!("the device policy doesn't allow {capability}");
        let record = serde_json::json!({ "request": summary, "reason": reason });
        audit(privd, "request.denied", record).await;
        return Err(CallError::new(ErrorCode::PolicyDenied, reason));
    }
    match call {
        ops::Call::SystemInfo(_) => {
            audit(
                privd,
                "request.allowed",
                serde_json::json!({ "request": summary }),
            )
            .await;
            let system = cntrl_host::system::backend();
            let info = tokio::task::spawn_blocking(move || system.info(env!("CARGO_PKG_VERSION")))
                .await
                .map_err(|e| CallError::internal(e.to_string()))??;
            serde_json::to_value(info).map_err(|e| CallError::internal(e.to_string()))
        }
        ops::Call::ServiceList(_) => {
            audit(
                privd,
                "request.allowed",
                serde_json::json!({ "request": summary }),
            )
            .await;
            let mut services = list_services().await?;
            services.extend(session_services(privd, limit).await);
            for service in &mut services {
                service.protected = policy.protects(&service.unit);
            }
            serde_json::to_value(ServiceList { services })
                .map_err(|e| CallError::internal(e.to_string()))
        }
        ops::Call::PowerInfo(_) => {
            audit(
                privd,
                "request.allowed",
                serde_json::json!({ "request": summary }),
            )
            .await;
            serde_json::to_value(power_info().await?)
                .map_err(|e| CallError::internal(e.to_string()))
        }
        ops::Call::NetworkListeners(_) => {
            audit(
                privd,
                "request.allowed",
                serde_json::json!({ "request": summary }),
            )
            .await;
            serde_json::to_value(listeners(privd, limit).await?)
                .map_err(|e| CallError::internal(e.to_string()))
        }
        ops::Call::StorageHealth(_) => {
            audit(
                privd,
                "request.allowed",
                serde_json::json!({ "request": summary }),
            )
            .await;
            serde_json::to_value(disk_health(privd, limit).await?)
                .map_err(|e| CallError::internal(e.to_string()))
        }
        ops::Call::HistoryRead(params) => {
            audit(
                privd,
                "request.allowed",
                serde_json::json!({ "request": summary }),
            )
            .await;
            let history = Arc::clone(history);
            let chart = tokio::task::spawn_blocking(move || history.read(&params, now_ms()))
                .await
                .map_err(|e| CallError::internal(e.to_string()))??;
            serde_json::to_value(chart).map_err(|e| CallError::internal(e.to_string()))
        }
        ops::Call::HistoryKeep(keep) => {
            audit(
                privd,
                "request.allowed",
                serde_json::json!({ "request": summary, "days": keep.days }),
            )
            .await;
            let history = Arc::clone(history);
            let store = tokio::task::spawn_blocking(move || history.keep(keep.days, now_ms()))
                .await
                .map_err(|e| CallError::internal(e.to_string()))??;
            serde_json::to_value(store).map_err(|e| CallError::internal(e.to_string()))
        }
        ops::Call::HistoryClear(_) => {
            audit(
                privd,
                "request.allowed",
                serde_json::json!({ "request": summary }),
            )
            .await;
            let history = Arc::clone(history);
            let store = tokio::task::spawn_blocking(move || history.clear())
                .await
                .map_err(|e| CallError::internal(e.to_string()))??;
            serde_json::to_value(store).map_err(|e| CallError::internal(e.to_string()))
        }
        // privd checks the policy again and audits its own decision.
        ops::Call::PowerReboot(_) => power(request, PowerAction::Reboot, privd, limit).await,
        ops::Call::PowerPoweroff(_) => power(request, PowerAction::Poweroff, privd, limit).await,
        ops::Call::PowerSuspend(_) => power(request, PowerAction::Suspend, privd, limit).await,
        ops::Call::PowerHibernate(_) => power(request, PowerAction::Hibernate, privd, limit).await,
        // privd checks the policy again and audits its own decision.
        ops::Call::AppQuit(quit) => {
            let call = Call::AppQuit {
                request_id: request.id.clone(),
                app: quit.app,
                user: quit.user,
                force: quit.force,
                actor: request.actor.clone(),
            };
            ipc::call_within(privd, call, limit).await
        }
        // privd checks the policy again and audits its own decision (D54).
        ops::Call::ContainerStart(target) => {
            act_on_container(request, target, docker::Action::Start, privd, limit).await
        }
        ops::Call::ContainerStop(target) => {
            act_on_container(request, target, docker::Action::Stop, privd, limit).await
        }
        ops::Call::ContainerRestart(target) => {
            act_on_container(request, target, docker::Action::Restart, privd, limit).await
        }
        ops::Call::ProcessSignal(signal) => {
            let call = Call::ProcessSignal {
                request_id: request.id.clone(),
                pid: signal.pid,
                started: signal.started,
                force: signal.force,
                actor: request.actor.clone(),
            };
            ipc::call_within(privd, call, limit).await
        }
        ops::Call::ServiceRestart(service) => {
            act_on_service(request, service, ServiceAction::Restart, privd, limit).await
        }
        ops::Call::ServiceStart(service) => {
            act_on_service(request, service, ServiceAction::Start, privd, limit).await
        }
        ops::Call::ServiceStop(service) => {
            act_on_service(request, service, ServiceAction::Stop, privd, limit).await
        }
        ops::Call::ServiceEnable(service) => {
            act_on_service(request, service, ServiceAction::Enable, privd, limit).await
        }
        ops::Call::ServiceDisable(service) => {
            act_on_service(request, service, ServiceAction::Disable, privd, limit).await
        }
    }
}

/// Asks privd to act on a service; it checks the policy again and audits.
async fn act_on_service(
    request: &Request,
    service: ServiceRef,
    action: ServiceAction,
    privd: &Path,
    limit: Duration,
) -> Result<serde_json::Value, CallError> {
    let call = Call::ServiceAct {
        request_id: request.id.clone(),
        unit: service.unit,
        action,
        scope: service.scope,
        user: service.user,
        actor: request.actor.clone(),
    };
    ipc::call_within(privd, call, limit).await
}

/// Asks privd to take a power action, which it answers before taking.
async fn power(
    request: &Request,
    action: PowerAction,
    privd: &Path,
    limit: Duration,
) -> Result<serde_json::Value, CallError> {
    let call = Call::Power {
        request_id: request.id.clone(),
        action,
        actor: request.actor.clone(),
    };
    ipc::call_within(privd, call, limit).await
}

/// How long privd may take to say who owns the sockets; past it, the ports
/// go without their processes.
#[cfg(target_os = "linux")]
const OWNERS_LIMIT: Duration = Duration::from_secs(10);

/// The ports the machine listens on (angle 13). On Linux the agent reads the
/// socket tables, which anyone can, and privd says which process holds each,
/// which takes root; without privd the ports come alone.
#[cfg(target_os = "linux")]
async fn listeners(privd: &Path, limit: Duration) -> Result<Listeners, CallError> {
    #[derive(Deserialize)]
    struct Owners {
        owners: Vec<(u64, u32)>,
    }
    let root = Path::new("/");
    let sockets =
        tokio::task::spawn_blocking(|| cntrl_host::network::linux::sockets(Path::new("/")))
            .await
            .map_err(|e| CallError::internal(e.to_string()))?;
    let inodes = sockets.iter().map(|socket| socket.inode).collect();
    let owners = match ipc::call_within(
        privd,
        Call::SocketOwners { inodes },
        limit.min(OWNERS_LIMIT),
    )
    .await
    {
        Ok(value) => serde_json::from_value::<Owners>(value)
            .ok()
            .map(|found| found.owners.into_iter().collect::<HashMap<u64, u32>>()),
        Err(e) => {
            warn!("privd didn't say who owns the sockets: {}", e.msg);
            None
        }
    };
    let users = cntrl_host::network::user_names(root);
    Ok(cntrl_host::network::listeners(
        root,
        &sockets,
        owners.as_ref(),
        &users,
    ))
}

/// On a Mac only root sees other users' sockets, so privd runs lsof; each
/// listener's service is the launchd job whose process it is.
#[cfg(target_os = "macos")]
async fn listeners(privd: &Path, limit: Duration) -> Result<Listeners, CallError> {
    let value = ipc::call_within(privd, Call::Listeners, limit).await?;
    let mut found: Listeners =
        serde_json::from_value(value).map_err(|e| CallError::internal(e.to_string()))?;
    if let Ok(services) = list_services().await {
        let jobs: HashMap<u32, String> = services
            .into_iter()
            .filter_map(|service| Some((service.pid?, service.unit)))
            .collect();
        for listener in &mut found.listeners {
            listener.service = listener.pid.and_then(|pid| jobs.get(&pid).cloned());
        }
    }
    Ok(found)
}

/// On Windows any account reads the socket tables with their owners, so the
/// agent does it all (D58).
#[cfg(windows)]
async fn listeners(_privd: &Path, _limit: Duration) -> Result<Listeners, CallError> {
    tokio::task::spawn_blocking(cntrl_host::windows::network::listeners)
        .await
        .map_err(|e| CallError::internal(e.to_string()))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
async fn listeners(_privd: &Path, _limit: Duration) -> Result<Listeners, CallError> {
    Err(HostError::Unsupported.into())
}

/// What each physical disk says of its health, leaving out a hypervisor's,
/// which have none of their own (angle 13). On Linux smartctl reads the raw
/// device, so privd runs it; on Windows privd reads an NVMe disk's health
/// log, which takes an administrator (D58).
#[cfg(any(target_os = "linux", windows))]
async fn disk_health(privd: &Path, limit: Duration) -> Result<DisksHealth, CallError> {
    let disks = physical_disks().await?;
    if disks.is_empty() {
        return Ok(only_virtual());
    }
    let value = ipc::call_within(privd, Call::DiskHealth { disks }, limit).await?;
    serde_json::from_value(value).map_err(|e| CallError::internal(e.to_string()))
}

/// On a Mac diskutil says, without root.
#[cfg(target_os = "macos")]
async fn disk_health(_privd: &Path, _limit: Duration) -> Result<DisksHealth, CallError> {
    let disks = physical_disks().await?;
    if disks.is_empty() {
        return Ok(only_virtual());
    }
    tokio::task::spawn_blocking(move || cntrl_host::storage::mac::health(&disks))
        .await
        .map_err(|e| CallError::internal(e.to_string()))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
async fn disk_health(_privd: &Path, _limit: Duration) -> Result<DisksHealth, CallError> {
    Err(HostError::Unsupported.into())
}

/// A machine whose disks a hypervisor provides can't see their health; the
/// host it runs on can.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn only_virtual() -> DisksHealth {
    DisksHealth {
        disks: Vec::new(),
        note: Some(
            "its disks are virtual, so their health is for the machine it runs on to tell"
                .to_owned(),
        ),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
async fn physical_disks() -> Result<Vec<String>, CallError> {
    tokio::task::spawn_blocking(|| {
        cntrl_host::storage::backend()
            .disks()
            .into_iter()
            .filter(|disk| disk.kind != cntrl_protocol::storage::DiskKind::Virtual)
            .map(|disk| disk.name)
            .collect()
    })
    .await
    .map_err(|e| CallError::internal(e.to_string()))
}

/// What the machine can do about power. Reading it needs no root (angle 10).
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
async fn power_info() -> Result<PowerInfo, HostError> {
    cntrl_host::power::info().await
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
async fn power_info() -> Result<PowerInfo, HostError> {
    Err(HostError::Unsupported)
}

/// The services at system scope. Listing needs no root, so the agent does it.
#[cfg(target_os = "linux")]
pub(super) async fn list_services() -> Result<Vec<ServiceStatus>, HostError> {
    cntrl_host::systemd::Systemd::connect().await?.list().await
}

#[cfg(target_os = "macos")]
pub(super) async fn list_services() -> Result<Vec<ServiceStatus>, HostError> {
    tokio::task::spawn_blocking(cntrl_host::launchd::list)
        .await
        .map_err(|e| HostError::Failed(e.to_string()))?
}

/// The Service Control Manager lists services to any account (D58).
#[cfg(windows)]
pub(super) async fn list_services() -> Result<Vec<ServiceStatus>, HostError> {
    tokio::task::spawn_blocking(cntrl_host::windows::services::list)
        .await
        .map_err(|e| HostError::Failed(e.to_string()))?
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) async fn list_services() -> Result<Vec<ServiceStatus>, HostError> {
    Err(HostError::Unsupported)
}

/// What runs in users' desktop sessions, which only privd can read. Without
/// it the list still has the system's services, so a failure is logged.
#[cfg(target_os = "macos")]
async fn session_services(privd: &Path, limit: Duration) -> Vec<ServiceStatus> {
    #[derive(Deserialize)]
    struct Listed {
        services: Vec<ServiceStatus>,
    }
    match ipc::call_within(privd, Call::ServiceListSessions, limit).await {
        Ok(value) => serde_json::from_value::<Listed>(value)
            .map(|listed| listed.services)
            .unwrap_or_else(|e| {
                warn!("privd's session services didn't parse: {e}");
                Vec::new()
            }),
        Err(e) => {
            warn!("can't list session services: {}", e.msg);
            Vec::new()
        }
    }
}

#[cfg(not(target_os = "macos"))]
async fn session_services(_privd: &Path, _limit: Duration) -> Vec<ServiceStatus> {
    Vec::new()
}

/// Records one of the agent's decisions in the audit log. A read goes ahead
/// even if the log can't be written; the failure is logged instead.
async fn audit(privd: &Path, kind: &str, data: serde_json::Value) {
    let call = Call::AuditAppend {
        kind: kind.to_owned(),
        data,
    };
    if let Err(e) = ipc::call_once(privd, call).await {
        warn!("can't audit a request: {e}");
    }
}

/// How much earlier than due a sample may be taken and still go out, in
/// milliseconds. Samples are about a second apart, so without slack a
/// subscription every second would skip every other one.
const DUE_SLACK_MS: u64 = 500;

/// A session's open subscriptions. Every `stats` subscription shares one
/// receiver of the sampler's latest sample, and every `processes` one a
/// receiver of the latest process table, each held only while such a
/// subscription is open, so the samplers work only while someone watches.
/// Each `logs` subscription has a reader of its own, which sends its batches
/// through `log_batches`.
struct Subscriptions {
    stats: Option<watch::Receiver<Option<Arc<StatsSample>>>>,
    processes: Option<watch::Receiver<Option<Arc<Table>>>>,
    network: Option<watch::Receiver<Option<Arc<NetworkSample>>>>,
    storage: Option<watch::Receiver<Option<Arc<StorageSample>>>>,
    containers: Option<watch::Receiver<Option<Arc<ContainersSample>>>>,
    open: HashMap<String, Subscription>,
    log_out: logs::Batches,
    log_batches: mpsc::Receiver<(String, LogsBatch)>,
    /// privd's socket, which a Mac's log readers go through.
    privd: PathBuf,
}

impl Subscriptions {
    fn new(privd: PathBuf) -> Self {
        let (log_out, log_batches) = mpsc::channel(64);
        Self {
            stats: None,
            processes: None,
            network: None,
            storage: None,
            containers: None,
            open: HashMap::new(),
            log_out,
            log_batches,
            privd,
        }
    }
}

impl Default for Subscriptions {
    fn default() -> Self {
        Self::new(PathBuf::new())
    }
}

/// A session that ends stops its log readers, which may be waiting on a quiet
/// log and wouldn't notice otherwise.
impl Drop for Subscriptions {
    fn drop(&mut self) {
        for sub in self.open.values() {
            if let Watching::Logs(reader) = &sub.topic {
                reader.stop();
            }
        }
    }
}

/// One open subscription. It's scheduled by the samples' own times, so events
/// stay evenly spaced whenever they're delivered.
struct Subscription {
    topic: Watching,
    every_ms: u64,
    /// The earliest sample time the next event may carry, in Unix milliseconds.
    next_ts: u64,
    seq: u64,
}

/// What a subscription watches. Each `processes` subscription gets its own
/// view of the table; each `logs` one, its own reader.
enum Watching {
    Stats,
    Processes(ProcessesParams),
    Logs(logs::Reader),
    Network,
    Storage,
    Containers,
}

/// The samplers a subscription reads from.
struct Samplers<'a> {
    stats: &'a Latest,
    processes: &'a processes::Latest,
    network: &'a network::Latest,
    storage: &'a storage::Latest,
    containers: &'a containers::Latest,
}

/// A process table this recent goes to a new subscription at once.
const FRESH_TABLE_MS: u64 = 3_000;

impl Subscriptions {
    /// Answers a `sub`: opens the subscription, or refuses it with the reason.
    async fn open(
        &mut self,
        ws: &mut Ws,
        subscribe: Subscribe,
        policy: &PolicyState,
        samplers: &Samplers<'_>,
    ) -> Result<(), End> {
        let refuse = |code, msg: String| Frame::Res(Response::err(subscribe.id.clone(), code, msg));
        if subscribe.ver != 1 {
            let msg = format!("{} has no version {}", subscribe.topic, subscribe.ver);
            return send(ws, &refuse(ErrorCode::UnsupportedVersion, msg)).await;
        }
        let topic = match Topic::decode(&subscribe.topic, subscribe.data) {
            Ok(topic) => topic,
            Err(e) => return send(ws, &refuse(e.code(), e.to_string())).await,
        };
        if !policy.allows(topic.capability()) {
            let msg = format!("the device policy doesn't allow {}", topic.capability());
            return send(ws, &refuse(ErrorCode::PolicyDenied, msg)).await;
        }
        match topic {
            Topic::Stats(params) => {
                let every_ms = params.interval_ms.unwrap_or(1_000).max(1_000);
                let receiver = self.stats.get_or_insert_with(|| samplers.stats.subscribe());
                let latest = receiver.borrow().clone();
                let accepted = encode(&StatsParams {
                    interval_ms: Some(every_ms),
                })?;
                send(
                    ws,
                    &Frame::Res(Response::ok(subscribe.id.clone(), accepted)),
                )
                .await?;
                let mut sub = Subscription {
                    topic: Watching::Stats,
                    every_ms: u64::from(every_ms),
                    next_ts: 0,
                    seq: 0,
                };
                // A new chart shouldn't wait for the next sample.
                if let Some(sample) = latest {
                    send_event(ws, &subscribe.id, &mut sub, sample.ts, &*sample).await?;
                }
                debug!(id = %subscribe.id, every_ms, "stats subscription opened");
                self.open.insert(subscribe.id, sub);
            }
            Topic::Processes(params) => {
                let limit = params
                    .limit
                    .unwrap_or(cntrl_host::processes::DEFAULT_LIMIT)
                    .clamp(1, cntrl_host::processes::MAX_LIMIT);
                let params = ProcessesParams {
                    limit: Some(limit),
                    ..params
                };
                let receiver = self
                    .processes
                    .get_or_insert_with(|| samplers.processes.subscribe());
                let latest = receiver.borrow().clone();
                let accepted = encode(&params)?;
                send(
                    ws,
                    &Frame::Res(Response::ok(subscribe.id.clone(), accepted)),
                )
                .await?;
                let mut sub = Subscription {
                    topic: Watching::Processes(params),
                    every_ms: millis(processes::INTERVAL),
                    next_ts: 0,
                    seq: 0,
                };
                if let Some(table) =
                    latest.filter(|table| now_ms().saturating_sub(table.ts) < FRESH_TABLE_MS)
                {
                    let view = sub.view(&table, policy);
                    if let Some(view) = view {
                        send_event(ws, &subscribe.id, &mut sub, table.ts, &view).await?;
                    }
                }
                debug!(id = %subscribe.id, "processes subscription opened");
                self.open.insert(subscribe.id, sub);
            }
            Topic::Network(params) => {
                let every_ms = params.interval_ms.unwrap_or(2_000).max(1_000);
                let receiver = self
                    .network
                    .get_or_insert_with(|| samplers.network.subscribe());
                let latest = receiver.borrow().clone();
                let accepted = encode(&NetworkParams {
                    interval_ms: Some(every_ms),
                })?;
                send(
                    ws,
                    &Frame::Res(Response::ok(subscribe.id.clone(), accepted)),
                )
                .await?;
                let mut sub = Subscription {
                    topic: Watching::Network,
                    every_ms: u64::from(every_ms),
                    next_ts: 0,
                    seq: 0,
                };
                if let Some(sample) =
                    latest.filter(|sample| now_ms().saturating_sub(sample.ts) < FRESH_TABLE_MS)
                {
                    send_event(ws, &subscribe.id, &mut sub, sample.ts, &*sample).await?;
                }
                debug!(id = %subscribe.id, every_ms, "network subscription opened");
                self.open.insert(subscribe.id, sub);
            }
            Topic::Storage(params) => {
                let every_ms = params.interval_ms.unwrap_or(2_000).max(1_000);
                let receiver = self
                    .storage
                    .get_or_insert_with(|| samplers.storage.subscribe());
                let latest = receiver.borrow().clone();
                let accepted = encode(&StorageParams {
                    interval_ms: Some(every_ms),
                })?;
                send(
                    ws,
                    &Frame::Res(Response::ok(subscribe.id.clone(), accepted)),
                )
                .await?;
                let mut sub = Subscription {
                    topic: Watching::Storage,
                    every_ms: u64::from(every_ms),
                    next_ts: 0,
                    seq: 0,
                };
                if let Some(sample) =
                    latest.filter(|sample| now_ms().saturating_sub(sample.ts) < FRESH_TABLE_MS)
                {
                    send_event(ws, &subscribe.id, &mut sub, sample.ts, &*sample).await?;
                }
                debug!(id = %subscribe.id, every_ms, "storage subscription opened");
                self.open.insert(subscribe.id, sub);
            }
            Topic::Containers(params) => {
                let every_ms = params.interval_ms.unwrap_or(3_000).max(2_000);
                let receiver = self
                    .containers
                    .get_or_insert_with(|| samplers.containers.subscribe());
                let latest = receiver.borrow().clone();
                let accepted = encode(&ContainersParams {
                    interval_ms: Some(every_ms),
                })?;
                send(
                    ws,
                    &Frame::Res(Response::ok(subscribe.id.clone(), accepted)),
                )
                .await?;
                let mut sub = Subscription {
                    topic: Watching::Containers,
                    every_ms: u64::from(every_ms),
                    next_ts: 0,
                    seq: 0,
                };
                if let Some(sample) =
                    latest.filter(|sample| now_ms().saturating_sub(sample.ts) < FRESH_TABLE_MS)
                {
                    send_event(ws, &subscribe.id, &mut sub, sample.ts, &*sample).await?;
                }
                debug!(id = %subscribe.id, every_ms, "containers subscription opened");
                self.open.insert(subscribe.id, sub);
            }
            Topic::Logs(params) => {
                if cfg!(not(any(target_os = "linux", target_os = "macos", windows))) {
                    let msg = "this agent can't read logs on this OS yet".to_owned();
                    return send(ws, &refuse(ErrorCode::BadRequest, msg)).await;
                }
                // A user's session runs a Mac's LaunchAgents; systemd's user
                // units come later.
                let user = match params.user.as_deref() {
                    None => None,
                    Some(_) if cfg!(not(target_os = "macos")) => {
                        let msg = "a user's services' logs aren't read on this OS yet".to_owned();
                        return send(ws, &refuse(ErrorCode::BadRequest, msg)).await;
                    }
                    Some(_) if params.unit.is_none() => {
                        let msg = "a user goes with a unit".to_owned();
                        return send(ws, &refuse(ErrorCode::BadRequest, msg)).await;
                    }
                    Some(user) if user_name(user) => Some(user.to_owned()),
                    Some(user) => {
                        let msg = format!("`{user}` isn't a user name");
                        return send(ws, &refuse(ErrorCode::BadRequest, msg)).await;
                    }
                };
                if let Some(oldest) = self.crowded_out() {
                    let last = LogsBatch {
                        ended: Some(format!(
                            "the device closed it for a newer one, since it keeps {} logs open at most",
                            logs::MAX_OPEN
                        )),
                        ..LogsBatch::default()
                    };
                    self.publish_logs(ws, &oldest, &last).await?;
                }
                let unit = match params
                    .unit
                    .as_deref()
                    .map(cntrl_host::services::service_name)
                {
                    Some(Err(e)) => {
                        return send(ws, &refuse(ErrorCode::BadRequest, e.to_string())).await;
                    }
                    Some(Ok(unit)) => Some(unit),
                    None => None,
                };
                if let Some(problem) = [&params.only, &params.hide]
                    .into_iter()
                    .find_map(|names| source_names(names))
                {
                    return send(ws, &refuse(ErrorCode::BadRequest, problem)).await;
                }
                let container = match params.container.as_deref().map(docker::container_name) {
                    Some(Err(e)) => return send(ws, &refuse(ErrorCode::BadRequest, e)).await,
                    Some(Ok(_)) if unit.is_some() => {
                        let msg = "a log is a service's or a container's, not both".to_owned();
                        return send(ws, &refuse(ErrorCode::BadRequest, msg)).await;
                    }
                    Some(Ok(name)) => Some(name.to_owned()),
                    None => None,
                };
                let params = LogsParams {
                    unit,
                    user,
                    container,
                    priority: params.priority.map(|p| p.min(7)),
                    lines: Some(
                        params
                            .lines
                            .unwrap_or(cntrl_host::journal::DEFAULT_LINES)
                            .min(cntrl_host::journal::MAX_LINES),
                    ),
                    grep: params.grep.filter(|g| !g.trim().is_empty()),
                    include_os: params.include_os,
                    only: params.only,
                    hide: params.hide,
                };
                let accepted = encode(&params)?;
                send(
                    ws,
                    &Frame::Res(Response::ok(subscribe.id.clone(), accepted)),
                )
                .await?;
                let reader = logs::start(
                    subscribe.id.clone(),
                    params,
                    self.privd.clone(),
                    self.log_out.clone(),
                );
                let sub = Subscription {
                    topic: Watching::Logs(reader),
                    every_ms: 0,
                    next_ts: 0,
                    seq: 0,
                };
                debug!(id = %subscribe.id, "logs subscription opened");
                self.open.insert(subscribe.id, sub);
            }
        }
        Ok(())
    }

    /// The `logs` subscription to close for a new one, when `MAX_OPEN` are
    /// open: the oldest.
    fn crowded_out(&self) -> Option<String> {
        let readers: Vec<_> = self
            .open
            .iter()
            .filter_map(|(id, sub)| match &sub.topic {
                Watching::Logs(reader) => Some((reader.started, id)),
                _ => None,
            })
            .collect();
        if readers.len() < logs::MAX_OPEN {
            return None;
        }
        readers
            .into_iter()
            .min_by_key(|(started, _)| *started)
            .map(|(_, id)| id.clone())
    }

    /// Sends a log reader's batch to its subscription; the last batch, which
    /// says why the reader stopped, closes it.
    async fn publish_logs(&mut self, ws: &mut Ws, id: &str, batch: &LogsBatch) -> Result<(), End> {
        let Some(sub) = self.open.get_mut(id) else {
            return Ok(());
        };
        send_event(ws, id, sub, now_ms(), batch).await?;
        if batch.ended.is_some() {
            self.close(id);
        }
        Ok(())
    }

    /// Ends a subscription; an unknown ID is ignored. A sampler's receiver goes
    /// with the last subscription that needed it.
    fn close(&mut self, id: &str) {
        if let Some(sub) = self.open.remove(id) {
            if let Watching::Logs(reader) = &sub.topic {
                reader.stop();
            }
            debug!(id, "subscription closed");
        }
        if !self
            .open
            .values()
            .any(|sub| matches!(sub.topic, Watching::Stats))
        {
            self.stats = None;
        }
        if !self
            .open
            .values()
            .any(|sub| matches!(sub.topic, Watching::Processes(_)))
        {
            self.processes = None;
        }
        if !self
            .open
            .values()
            .any(|sub| matches!(sub.topic, Watching::Network))
        {
            self.network = None;
        }
        if !self
            .open
            .values()
            .any(|sub| matches!(sub.topic, Watching::Storage))
        {
            self.storage = None;
        }
        if !self
            .open
            .values()
            .any(|sub| matches!(sub.topic, Watching::Containers))
        {
            self.containers = None;
        }
    }

    /// Sends a new sample to every `stats` subscription that's due.
    async fn publish(&mut self, ws: &mut Ws, sample: &StatsSample) -> Result<(), End> {
        for (id, sub) in &mut self.open {
            if matches!(sub.topic, Watching::Stats) && sample.ts >= sub.next_ts {
                send_event(ws, id, sub, sample.ts, sample).await?;
            }
        }
        Ok(())
    }

    /// Sends a new sample to every subscription of its kind that's due.
    async fn publish_to<T: Serialize>(
        &mut self,
        ws: &mut Ws,
        kind: impl Fn(&Watching) -> bool,
        ts: u64,
        sample: &T,
    ) -> Result<(), End> {
        for (id, sub) in &mut self.open {
            if kind(&sub.topic) && ts >= sub.next_ts {
                send_event(ws, id, sub, ts, sample).await?;
            }
        }
        Ok(())
    }

    /// Sends each `processes` subscription that's due its view of a new table.
    async fn publish_table(
        &mut self,
        ws: &mut Ws,
        table: &Table,
        policy: &PolicyState,
    ) -> Result<(), End> {
        for (id, sub) in &mut self.open {
            if table.ts < sub.next_ts {
                continue;
            }
            if let Some(view) = sub.view(table, policy) {
                send_event(ws, id, sub, table.ts, &view).await?;
            }
        }
        Ok(())
    }
}

impl Subscription {
    /// A `processes` subscription's view of a table, with what privd would
    /// refuse to stop marked, so Console doesn't offer it; `None` for other
    /// topics.
    fn view(
        &self,
        table: &Table,
        policy: &PolicyState,
    ) -> Option<cntrl_protocol::process::ProcessesSample> {
        let Watching::Processes(params) = &self.topic else {
            return None;
        };
        let mut view = cntrl_host::processes::view(&table.processes, params, table.ts);
        for process in &mut view.processes {
            process.protected = process.pid <= 1
                || process.kernel
                // The table names accounts without their domain.
                || process.user.as_deref() == super::os::AGENT_ACCOUNT.rsplit('\\').next()
                || process
                    .unit
                    .as_deref()
                    .is_some_and(|unit| policy.protects(unit));
        }
        Some(view)
    }
}

/// Sends one event and schedules the subscription's next.
async fn send_event<T: Serialize>(
    ws: &mut Ws,
    id: &str,
    sub: &mut Subscription,
    ts: u64,
    data: &T,
) -> Result<(), End> {
    let event = Event {
        sub: id.to_owned(),
        seq: sub.seq,
        data: encode(data)?,
    };
    send(ws, &Frame::Evt(event)).await?;
    sub.seq += 1;
    sub.next_ts = ts + sub.every_ms.saturating_sub(DUE_SLACK_MS);
    Ok(())
}

/// A sampler's next value; `None` if it stopped.
async fn next_value<T>(latest: &mut Option<watch::Receiver<Option<Arc<T>>>>) -> Option<Arc<T>> {
    let receiver = latest.as_mut()?;
    receiver.changed().await.ok()?;
    receiver.borrow_and_update().clone()
}

fn encode<T: Serialize>(value: &T) -> Result<serde_json::Value, End> {
    serde_json::to_value(value).map_err(|e| End::Stop {
        reason: format!("can't encode a frame: {e}"),
    })
}

/// The gateway is draining: leave, and come back inside its window.
async fn go_away(ws: &mut Ws, goaway: &GoAway, stable: bool) -> End {
    close_link(ws, close::DISCONNECTED_BY_DEVICE, "going away").await;
    let window = &goaway.reconnect_after;
    let span = u64::from(window.max_ms.saturating_sub(window.min_ms));
    let delay = u64::from(window.min_ms) + random_u64().checked_rem(span + 1).unwrap_or(0);
    End::Retry {
        reason: format!("the gateway asked agents to reconnect: {}", goaway.reason),
        at_least: Duration::from_millis(delay),
        stable,
    }
}

/// The policy as privd reads it. Anything short of a valid policy allows nothing.
async fn policy(privd_socket: &Path) -> PolicyState {
    match ipc::call_once(privd_socket, Call::PolicyShow).await {
        Ok(value) => serde_json::from_value(value).unwrap_or_else(|e| PolicyState::Invalid {
            reason: format!("privd's policy reply is unreadable: {e}"),
        }),
        Err(e) => PolicyState::Invalid {
            reason: format!("privd is unreachable: {e}"),
        },
    }
}

/// Keeps a renewed credential, or forgets a rejected one, unless the device was
/// enrolled again meanwhile.
async fn save_credential(
    state_dir: &Path,
    uplink: &Uplink,
    session: &Identity,
    credential: Option<String>,
) {
    let _identity = uplink.lock_identity().await;
    let current = match identity::load(state_dir) {
        Ok(Some(current)) => current,
        Ok(None) => return,
        Err(e) => {
            warn!("can't update the connection credential: {e}");
            return;
        }
    };
    if current.device_id != session.device_id || current.key_id != session.key_id {
        return;
    }
    let updated = Identity {
        credential,
        ..current
    };
    if let Err(e) = identity::save(state_dir, &updated) {
        warn!("can't save the connection credential: {e}");
    }
}

fn connect_request(
    url: &str,
    device_id: &str,
    credential: Option<&str>,
) -> Result<http::Request<()>, String> {
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("{url} isn't a gateway URL: {e}"))?;
    let device = HeaderValue::from_str(device_id)
        .map_err(|_| format!("the device ID {device_id:?} isn't a valid header value"))?;
    let headers = request.headers_mut();
    // Without a usable credential the gateway falls back to the device ID.
    if let Some(credential) = credential
        && let Ok(value) = HeaderValue::from_str(&format!("Bearer {credential}"))
    {
        headers.insert("authorization", value);
    }
    headers.insert(
        "sec-websocket-protocol",
        HeaderValue::from_static(SUBPROTOCOL),
    );
    headers.insert("cntrl-device", device);
    headers.insert(
        "user-agent",
        HeaderValue::from_static(concat!("cntrl-agent/", env!("CARGO_PKG_VERSION"))),
    );
    // What this agent understands that older ones don't: a gateway keeps a
    // disabled device's link quiet only for those that say `disabled` (D87).
    headers.insert("cntrl-features", HeaderValue::from_static(FEATURES));
    Ok(request)
}

/// The next protocol frame, skipping heartbeats; how the session ends if the
/// socket closes or fails first.
async fn next_frame(ws: &mut Ws) -> Result<Frame, End> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(text))) => {
                if text.as_str() == PING || text.as_str() == PONG {
                    continue;
                }
                return serde_json::from_str(text.as_str())
                    .map_err(|e| retry(format!("the gateway sent an unreadable frame: {e}")));
            }
            Some(Ok(Message::Close(frame))) => return Err(after_close(frame, false)),
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(retry(format!("link lost: {e}"))),
            None => return Err(retry("the gateway closed the connection")),
        }
    }
}

async fn send(ws: &mut Ws, frame: &Frame) -> Result<(), End> {
    let text = serde_json::to_string(frame).map_err(|e| End::Stop {
        reason: format!("can't encode a frame: {e}"),
    })?;
    ws.send(Message::text(text))
        .await
        .map_err(|e| retry(format!("link lost: {e}")))
}

/// Sends a close frame and waits briefly for the gateway's.
async fn close_link(ws: &mut Ws, code: u16, reason: &'static str) {
    let frame = CloseFrame {
        code: CloseCode::from(code),
        reason: reason.into(),
    };
    let drain = async {
        if ws.close(Some(frame)).await.is_ok() {
            while let Some(Ok(_)) = ws.next().await {}
        }
    };
    if timeout(Duration::from_secs(2), drain).await.is_err() {
        debug!("the gateway didn't finish closing");
    }
}

/// What a close from the gateway means for reconnecting.
fn after_close(frame: Option<CloseFrame>, stable: bool) -> End {
    let (code, said) = frame.map_or((1005, String::new()), |frame| {
        (u16::from(frame.code), frame.reason.as_str().to_owned())
    });
    let stop = |reason: &str| End::Stop {
        reason: reason.to_owned(),
    };
    let wait = |reason: String, at_least: Duration| End::Retry {
        reason,
        at_least,
        stable,
    };
    match code {
        close::REVOKED => stop(
            "this device was removed in Console; run Add device's command on it to add it again",
        ),
        close::GENERATION_MISMATCH => stop(
            "Console locked this device because its key generation doesn't match; enroll it again",
        ),
        close::UNSUPPORTED_VERSION => {
            stop("Console doesn't support this agent's protocol version; update the agent")
        }
        close::REPLACED => wait(
            "another connection with this device's identity replaced this one".to_owned(),
            AUTH_RETRY,
        ),
        close::CREDENTIAL_EXPIRED => End::Rejected { stable },
        close::TRY_LATER => wait("the gateway is busy".to_owned(), Duration::from_secs(30)),
        _ if said.is_empty() => wait(
            format!("the gateway closed the link ({code})"),
            Duration::ZERO,
        ),
        _ => wait(
            format!("the gateway closed the link ({code}: {said})"),
            Duration::ZERO,
        ),
    }
}

/// What an HTTP refusal of the upgrade means for reconnecting.
fn refused(response: &http::Response<Option<Vec<u8>>>) -> End {
    let status = response.status().as_u16();
    let msg = response
        .body()
        .as_deref()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
        .and_then(|body| body.get("msg")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| response.status().to_string());
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(3_600)));
    let at_least = match status {
        401 | 403 => AUTH_RETRY,
        429 | 503 => retry_after.unwrap_or(Duration::ZERO),
        _ => retry_after.unwrap_or(Duration::from_secs(30)),
    };
    End::Retry {
        reason: format!("the gateway refused the connection ({status}: {msg})"),
        at_least,
        stable: false,
    }
}

/// Full jitter: a random delay up to min(cap, base * 2^attempt).
fn backoff(attempt: u32) -> Duration {
    let ceiling = BACKOFF_BASE
        .saturating_mul(1 << attempt.min(6))
        .min(BACKOFF_CAP);
    let ceiling_ms = millis(ceiling);
    Duration::from_millis(
        random_u64()
            .checked_rem(ceiling_ms + 1)
            .unwrap_or(ceiling_ms),
    )
}

fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    match SystemRandom::new().fill(&mut bytes) {
        Ok(()) => u64::from_le_bytes(bytes),
        // No entropy: wait the longest, which is the cautious choice.
        Err(_) => u64::MAX,
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, millis)
}

/// Why a list of sources to show or hide can't be used, if it can't: at most
/// 20, each a name of 1 to 256 characters without control characters.
fn source_names(names: &[String]) -> Option<String> {
    if names.len() > 20 {
        return Some("at most 20 sources can be named".to_owned());
    }
    names
        .iter()
        .find(|name| name.is_empty() || name.len() > 256 || name.chars().any(char::is_control))
        .map(|name| format!("`{name}` isn't a source's name"))
}

/// A user name as macOS and Linux allow it: letters, digits and `._-`, not
/// starting with `-`.
fn user_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

#[cfg(test)]
mod tests {
    use cntrl_protocol::frame::DisabledReason;
    use tokio_tungstenite::WebSocketStream;

    use super::*;
    use crate::agent::paused::PausedState;

    /// A heartbeat fast enough for a test.
    const QUICK: Beat = Beat {
        interval: Duration::from_millis(40),
        pong_timeout: Duration::from_secs(5),
    };

    fn disabled(message: &str) -> Disabled {
        Disabled {
            reason: DisabledReason::Plan,
            message: Some(message.to_owned()),
            hb: HeartbeatConfig {
                interval_ms: 300_000,
                timeout_ms: 600_000,
            },
        }
    }

    /// A loopback link: the agent's end, and the gateway's.
    async fn link() -> (Ws, WebSocketStream<TcpStream>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let accepting = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            tokio_tungstenite::accept_async(stream)
                .await
                .expect("handshake")
        });
        let (agent, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
            .await
            .expect("connect");
        (agent, accepting.await.expect("joined"))
    }

    /// The next frame the agent sends the gateway, answering its heartbeats;
    /// none if the link ends first.
    async fn next_frame_from(gateway: &mut WebSocketStream<TcpStream>) -> Option<Frame> {
        loop {
            let message = timeout(Duration::from_secs(5), gateway.next())
                .await
                .ok()??
                .ok()?;
            match message {
                Message::Text(text) if text.as_str() == PING => {
                    gateway.send(Message::text(PONG)).await.ok()?;
                }
                Message::Text(text) => return serde_json::from_str(text.as_str()).ok(),
                _ => {}
            }
        }
    }

    /// The next frame, as `pick` takes it apart.
    async fn next<T>(
        gateway: &mut WebSocketStream<TcpStream>,
        pick: impl Fn(Frame) -> Option<T>,
    ) -> Option<T> {
        next_frame_from(gateway).await.and_then(pick)
    }

    /// Runs the gateway's end until the agent's close finishes.
    async fn drain(gateway: &mut WebSocketStream<TcpStream>) {
        while let Some(Ok(_)) = gateway.next().await {}
    }

    #[tokio::test]
    async fn a_disabled_link_sends_only_heartbeats_and_refuses_requests() {
        let dir = tempfile::tempdir().expect("temp dir");
        let alerts = Alerts::open(dir.path()).await;
        let uplink = Uplink::new();
        let token = CancellationToken::new();
        let (mut agent, mut gateway) = link().await;
        let notice =
            disabled("Free covers 3 devices\u{1b}[31m, and this isn't one of them.\u{202e}");
        let quieting = quiet(
            &mut agent,
            &notice,
            QUICK,
            Instant::now(),
            "ws://gateway",
            &alerts,
            &uplink,
            &token,
        );
        let gatewaying = async {
            // Heartbeats only, and nothing else, for a few of them.
            for _ in 0..3 {
                let message = timeout(Duration::from_secs(5), gateway.next())
                    .await
                    .expect("in time");
                assert!(
                    matches!(&message, Some(Ok(Message::Text(text))) if text.as_str() == PING),
                    "a disabled link sends only heartbeats: {message:?}"
                );
                gateway.send(Message::text(PONG)).await.expect("pong");
            }
            // A request is refused, not left waiting.
            let request = r#"{"t":"req","id":"req_1","op":"system.info","ver":1,"deadline_ms":15000,"data":{}}"#;
            gateway.send(Message::text(request)).await.expect("request");
            let answer = next(&mut gateway, |frame| match frame {
                Frame::Res(answer) => Some(answer),
                _ => None,
            })
            .await
            .expect("an answer");
            assert_eq!(answer.id, "req_1");
            assert_eq!(
                answer.err.map(|err| err.code),
                Some(ErrorCode::PolicyDenied)
            );
            // The gateway brings the device back by closing the link.
            let restart = CloseFrame {
                code: CloseCode::from(close::SERVICE_RESTART),
                reason: "back on".into(),
            };
            // The agent's end hangs up when the session ends; it doesn't answer here.
            gateway.close(Some(restart)).await.expect("close");
        };
        let (end, ()) = tokio::join!(quieting, gatewaying);
        assert!(
            matches!(end, End::Retry { at_least, stable: false, .. } if at_least == Duration::ZERO),
            "a 1012 reconnects"
        );
        assert!(
            matches!(
                uplink.status(),
                UplinkStatus::Disabled { message: Some(message), .. }
                    if message == "Free covers 3 devices[31m, and this isn't one of them."
            ),
            "disabled, and Console's words lose their control characters: {:?}",
            uplink.status()
        );
        assert!(alerts.resting().await, "the alert rules rest");
    }

    #[tokio::test]
    async fn a_disabled_link_still_tells_the_gateway_of_a_pause() {
        let dir = tempfile::tempdir().expect("temp dir");
        let alerts = Alerts::open(dir.path()).await;
        let uplink = Uplink::new();
        let token = CancellationToken::new();
        let (mut agent, mut gateway) = link().await;
        let notice = disabled("Free covers 3 devices.");
        let quieting = quiet(
            &mut agent,
            &notice,
            QUICK,
            Instant::now(),
            "ws://gateway",
            &alerts,
            &uplink,
            &token,
        );
        let pausing = uplink.pause(dir.path(), "alok".to_owned(), Some("moving it".to_owned()));
        let gatewaying = async {
            let pause = next(&mut gateway, |frame| match frame {
                Frame::Pause(pause) => Some(pause),
                _ => None,
            })
            .await
            .expect("a pause");
            assert_eq!(pause.by, "alok");
            gateway
                .send(Message::text(r#"{"t":"paused"}"#))
                .await
                .expect("paused");
            drain(&mut gateway).await;
        };
        let (end, told, ()) = tokio::join!(quieting, pausing, gatewaying);
        assert!(matches!(end, End::Paused));
        assert_eq!(told, Ok(true));
    }

    #[tokio::test]
    async fn an_uninstall_tells_the_gateway_from_a_disabled_link() {
        let dir = tempfile::tempdir().expect("temp dir");
        let alerts = Alerts::open(dir.path()).await;
        let uplink = Uplink::new();
        let token = CancellationToken::new();
        let (mut agent, mut gateway) = link().await;
        let notice = disabled("Free covers 3 devices.");
        let quieting = quiet(
            &mut agent,
            &notice,
            QUICK,
            Instant::now(),
            "ws://gateway",
            &alerts,
            &uplink,
            &token,
        );
        let uninstalling = uplink.uninstall("alok".to_owned());
        let gatewaying = async {
            let uninstall = next(&mut gateway, |frame| match frame {
                Frame::Uninstall(uninstall) => Some(uninstall),
                _ => None,
            })
            .await
            .expect("an uninstall");
            assert_eq!(uninstall.by, "alok");
            gateway
                .send(Message::text(r#"{"t":"uninstalled"}"#))
                .await
                .expect("uninstalled");
            drain(&mut gateway).await;
        };
        let (end, told, ()) = tokio::join!(quieting, uninstalling, gatewaying);
        assert!(matches!(end, End::Uninstalled));
        assert!(told, "the gateway recorded it");
    }

    #[tokio::test]
    async fn an_uninstall_from_an_unanswering_gateway_isnt_told() {
        let dir = tempfile::tempdir().expect("temp dir");
        let alerts = Alerts::open(dir.path()).await;
        let uplink = Uplink::new();
        let token = CancellationToken::new();
        let (mut agent, mut gateway) = link().await;
        let notice = disabled("Free covers 3 devices.");
        let quieting = quiet(
            &mut agent,
            &notice,
            QUICK,
            Instant::now(),
            "ws://gateway",
            &alerts,
            &uplink,
            &token,
        );
        let uninstalling = uplink.uninstall("alok".to_owned());
        // A gateway from before D87 ignores the frame it doesn't know.
        let gatewaying = async {
            let frame = next_frame_from(&mut gateway).await;
            assert!(matches!(frame, Some(Frame::Uninstall(_))), "{frame:?}");
            drain(&mut gateway).await;
        };
        let (end, told, ()) = tokio::join!(quieting, uninstalling, gatewaying);
        assert!(matches!(end, End::Uninstalled));
        assert!(!told, "nobody answered");
    }

    #[tokio::test]
    async fn a_session_disabled_midway_goes_quiet_and_refuses_requests() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = uplink_config(dir.path()).await;
        let uplink = Uplink::new();
        let token = CancellationToken::new();
        let (mut agent, mut gateway) = link().await;
        let welcome = Welcome {
            session: "ses_test".to_owned(),
            hb: HeartbeatConfig {
                interval_ms: 30_000,
                timeout_ms: 75_000,
            },
            acked_upto: None,
            limits: cntrl_protocol::frame::Limits {
                max_frame_bytes: MAX_FRAME_BYTES,
                max_inflight: 8,
                max_rec_batch: 50,
            },
            subs: Vec::new(),
            credential: None,
        };
        let policy = Arc::new(PolicyState::Invalid {
            reason: "none in a test".to_owned(),
        });
        let session = online(
            &mut agent,
            &welcome,
            policy,
            "ws://gateway",
            &config,
            &uplink,
            &token,
        );
        let gatewaying = async {
            // The hub disables it during the session; the heartbeat it gives is
            // past the agent's least, so nothing else should come for a while.
            let notice =
                serde_json::to_string(&Frame::Disabled(disabled("Free covers 3 devices.")))
                    .expect("frame");
            gateway.send(Message::text(notice)).await.expect("disabled");
            let request = r#"{"t":"req","id":"req_2","op":"system.info","ver":1,"deadline_ms":15000,"data":{}}"#;
            gateway.send(Message::text(request)).await.expect("request");
            let answer = next(&mut gateway, |frame| match frame {
                Frame::Res(answer) => Some(answer),
                _ => None,
            })
            .await
            .expect("an answer");
            assert_eq!(answer.id, "req_2");
            assert_eq!(
                answer.err.map(|err| (err.code, err.msg)),
                Some((ErrorCode::PolicyDenied, DISABLED_REFUSAL.to_owned())),
                "the quiet link answered it, not the session"
            );
            let restart = CloseFrame {
                code: CloseCode::from(close::SERVICE_RESTART),
                reason: "back on".into(),
            };
            gateway.close(Some(restart)).await.expect("close");
        };
        let (end, ()) = tokio::join!(session, gatewaying);
        assert!(matches!(end, End::Retry { .. }), "a 1012 reconnects");
        assert!(matches!(uplink.status(), UplinkStatus::Disabled { .. }));
        assert!(config.alerts.resting().await, "the alert rules rest");
    }

    async fn uplink_config(dir: &Path) -> UplinkConfig {
        UplinkConfig {
            state_dir: dir.to_path_buf(),
            privd_socket: dir.join("privd.sock"),
            gateway_url: None,
            stats: Arc::new(Latest::new(None)),
            processes: Arc::new(processes::Latest::new(None)),
            network: Arc::new(network::Latest::new(None)),
            storage: Arc::new(storage::Latest::new(None)),
            containers: Arc::new(containers::Latest::new(None)),
            outbox: Arc::new(Outbox::open(dir).await),
            alerts: Arc::new(Alerts::open(dir).await),
            history: Arc::new(History::open(dir)),
        }
    }

    #[tokio::test]
    async fn an_uninstall_with_nobody_to_tell_answers_at_once() {
        // Not enrolled, so parked.
        let dir = tempfile::tempdir().expect("temp dir");
        let uplink = Arc::new(Uplink::new());
        let token = CancellationToken::new();
        let running = tokio::spawn(run(
            uplink_config(dir.path()).await,
            Arc::clone(&uplink),
            token.clone(),
        ));
        let asked = Instant::now();
        assert!(!uplink.uninstall("alok".to_owned()).await);
        assert!(
            asked.elapsed() < Duration::from_secs(2),
            "it doesn't wait out the timeout"
        );
        token.cancel();
        running.await.expect("joined").expect("ran");

        // Paused, so nobody is online.
        let dir = tempfile::tempdir().expect("temp dir");
        let state = PausedState {
            by: "alok".to_owned(),
            reason: None,
            since_ms: now_ms(),
        };
        paused::save(dir.path(), &state).expect("paused");
        let uplink = Arc::new(Uplink::new());
        let token = CancellationToken::new();
        let running = tokio::spawn(run(
            uplink_config(dir.path()).await,
            Arc::clone(&uplink),
            token.clone(),
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        let asked = Instant::now();
        assert!(!uplink.uninstall("alok".to_owned()).await);
        assert!(
            asked.elapsed() < Duration::from_secs(2),
            "it doesn't wait out the timeout"
        );
        assert_eq!(
            uplink.status(),
            UplinkStatus::Stopped {
                reason: "uninstalling".to_owned()
            },
            "and the uplink stays down"
        );
        token.cancel();
        running.await.expect("joined").expect("ran");
    }

    #[test]
    fn a_restarting_gateway_brings_a_long_session_back_within_a_second() {
        let frame = CloseFrame {
            code: CloseCode::from(close::SERVICE_RESTART),
            reason: "".into(),
        };
        let end = after_close(Some(frame), true);
        assert!(
            matches!(end, End::Retry { at_least, stable: true, .. } if at_least == Duration::ZERO)
        );
        // A stable session resets the attempts, so the wait is the first step's at most.
        assert!(backoff(0) <= Duration::from_secs(1));
    }

    #[test]
    fn consoles_words_are_cleaned_before_a_terminal_shows_them() {
        assert_eq!(
            shown(Some("  plan\u{7}\u{1b}[2J limits\u{2066}  ")).as_deref(),
            Some("plan[2J limits")
        );
        assert_eq!(shown(Some("\u{0}\u{1b}")), None);
        assert_eq!(shown(None), None);
        let long = "x".repeat(DISABLED_MESSAGE_MAX * 2);
        assert_eq!(
            shown(Some(&long)).map(|text| text.chars().count()),
            Some(DISABLED_MESSAGE_MAX)
        );
    }

    #[test]
    fn the_agent_says_it_understands_disabled() {
        let request =
            connect_request("ws://localhost:8787/v1/agent", "dev_01", None).expect("request");
        assert_eq!(
            request
                .headers()
                .get("cntrl-features")
                .and_then(|value| value.to_str().ok()),
            Some("disabled")
        );
    }

    #[tokio::test]
    async fn a_new_log_past_the_limit_closes_the_oldest() {
        let mut subs = Subscriptions::default();
        let now = std::time::Instant::now();
        let add = |subs: &mut Subscriptions, id: &str, topic: Watching| {
            let sub = Subscription {
                topic,
                every_ms: 0,
                next_ts: 0,
                seq: 0,
            };
            subs.open.insert(id.to_owned(), sub);
        };
        add(&mut subs, "sub_stats", Watching::Stats);
        for i in 1..logs::MAX_OPEN {
            let started = now + Duration::from_secs(i as u64);
            add(
                &mut subs,
                &format!("sub_{i}"),
                Watching::Logs(logs::Reader::idle(started)),
            );
        }
        // Below the limit, nothing gives way.
        assert_eq!(subs.crowded_out(), None);
        add(&mut subs, "sub_0", Watching::Logs(logs::Reader::idle(now)));
        assert_eq!(subs.crowded_out().as_deref(), Some("sub_0"));
    }

    #[test]
    fn backoff_stays_under_its_ceiling() {
        for attempt in 0..20 {
            let ceiling = BACKOFF_BASE
                .saturating_mul(1 << attempt.min(6))
                .min(BACKOFF_CAP);
            assert!(backoff(attempt) <= ceiling);
        }
    }

    #[test]
    fn closes_that_mean_stop_park_the_uplink() {
        for code in [
            close::REVOKED,
            close::GENERATION_MISMATCH,
            close::UNSUPPORTED_VERSION,
        ] {
            let frame = CloseFrame {
                code: CloseCode::from(code),
                reason: "".into(),
            };
            assert!(
                matches!(after_close(Some(frame), true), End::Stop { .. }),
                "{code}"
            );
        }
    }

    #[test]
    fn credential_closes_are_rejections_and_replacements_wait() {
        let close = |code: u16| CloseFrame {
            code: CloseCode::from(code),
            reason: "".into(),
        };
        assert!(matches!(
            after_close(Some(close(close::CREDENTIAL_EXPIRED)), true),
            End::Rejected { stable: true }
        ));
        assert!(matches!(
            after_close(Some(close(close::REPLACED)), false),
            End::Retry { at_least, .. } if at_least == AUTH_RETRY
        ));
    }

    #[test]
    fn the_credential_goes_in_the_authorization_header() {
        let url = "ws://localhost:8787/v1/agent";
        let request = connect_request(url, "dev_01", Some("v1.e30.c2ln")).expect("request");
        let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok());
        assert_eq!(header("authorization"), Some("Bearer v1.e30.c2ln"));
        assert_eq!(header("cntrl-device"), Some("dev_01"));
        let request = connect_request(url, "dev_01", None).expect("request");
        assert!(request.headers().get("authorization").is_none());
    }

    #[test]
    fn an_unknown_device_waits_and_a_busy_gateway_sets_the_pace() {
        let response = |status: u16, retry_after: Option<&str>| {
            let mut builder = http::Response::builder().status(status);
            if let Some(value) = retry_after {
                builder = builder.header("retry-after", value);
            }
            builder
                .body(Some(
                    br#"{"code":"unknown_device","msg":"not enrolled"}"#.to_vec(),
                ))
                .expect("response")
        };
        let end = refused(&response(401, None));
        assert!(matches!(
            end,
            End::Retry { at_least, ref reason, .. }
                if at_least == AUTH_RETRY && reason.contains("not enrolled")
        ));
        let end = refused(&response(503, Some("30")));
        assert!(matches!(end, End::Retry { at_least, .. } if at_least == Duration::from_secs(30)));
    }
}
