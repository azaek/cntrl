//! The uplink: the agent's WebSocket to the gateway. It answers the gateway's
//! challenge with a hello signed by the device key, keeps the link alive with
//! heartbeats, and reconnects with full-jitter backoff. Close codes that mean
//! "stop" (revoked, locked, unsupported version) park it until the device is
//! enrolled again; it never ends the agent.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cntrl_protocol::auth::{gateway_host, hello_signing_string};
use cntrl_protocol::codes::{ErrorCode, close};
use cntrl_protocol::frame::{
    AgentInfo, Frame, GoAway, Hello, HelloAuth, OutboxState, Response, SigAlg, Welcome,
};
use cntrl_protocol::{MAX_FRAME_BYTES, PING, PONG, PROTOCOL_VERSION, SUBPROTOCOL};
use futures_util::{SinkExt, StreamExt};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, MutexGuard, Notify, watch};
use tokio::time::{MissedTickBehavior, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{self, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::host;
use super::identity::{self, DEVICE_KEY_FILE, Identity};
use super::ipc::{self, Call};
use super::keys::SigningKey;
use super::policy::PolicyState;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the gateway has to send its challenge, then its welcome.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(60);
/// The least wait after the gateway refuses this device's credentials.
const AUTH_RETRY: Duration = Duration::from_secs(300);
/// A session that lasted this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(60);
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
}

/// What the uplink shares with the local API.
pub struct Uplink {
    status: watch::Sender<UplinkStatus>,
    /// Signalled after an enrollment, so the uplink reconnects as the new identity.
    enrolled: Notify,
    /// Held while the identity file is written.
    identity: Mutex<()>,
}

impl Uplink {
    pub fn new() -> Self {
        Self {
            status: watch::Sender::new(UplinkStatus::NotEnrolled),
            enrolled: Notify::new(),
            identity: Mutex::new(()),
        }
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

    fn set(&self, status: UplinkStatus) {
        // `send` fails when nobody holds a receiver; the status must stick anyway.
        self.status.send_replace(status);
    }
}

/// What the uplink reads from the configuration.
pub struct UplinkConfig {
    pub state_dir: PathBuf,
    pub privd_socket: PathBuf,
    /// Replaces the gateway URL from enrollment.
    pub gateway_url: Option<String>,
}

/// How a session ended, and what to do next.
enum End {
    Shutdown,
    /// The device was enrolled again: reconnect now, as the new identity.
    Reenrolled,
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
            End::Reenrolled => {
                attempt = 0;
                continue;
            }
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
            () = token.cancelled() => return Ok(()),
        }
    }
}

/// Waits for an enrollment; false on shutdown.
async fn park(uplink: &Uplink, token: &CancellationToken) -> bool {
    tokio::select! {
        () = uplink.enrolled.notified() => true,
        () = token.cancelled() => false,
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
        outbox: OutboxState {
            next_seq: 0,
            oldest_unacked: None,
        },
    };
    if let Err(end) = send(&mut ws, &Frame::Hello(Box::new(hello))).await {
        return end;
    }
    let welcome = match timeout(HANDSHAKE_TIMEOUT, next_frame(&mut ws)).await {
        Ok(Ok(Frame::Welcome(welcome))) => welcome,
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
    info!(session = %welcome.session, gateway = url, "uplink online");
    uplink.set(UplinkStatus::Online {
        gateway: url.to_owned(),
        session: welcome.session.clone(),
        since_ms: now_ms(),
    });
    online(&mut ws, &welcome, uplink, token).await
}

/// The connected session: heartbeats, and frames from Console.
async fn online(ws: &mut Ws, welcome: &Welcome, uplink: &Uplink, token: &CancellationToken) -> End {
    let interval = Duration::from_millis(u64::from(welcome.hb.interval_ms))
        .clamp(HEARTBEAT_MIN, HEARTBEAT_MAX);
    let pong_timeout = Duration::from_millis(u64::from(welcome.hb.timeout_ms))
        .clamp(interval, HEARTBEAT_MAX.saturating_mul(2));
    let started = Instant::now();
    let stable = || started.elapsed() >= STABLE_AFTER;
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_pong = Instant::now();

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
                        if let Some(end) = handle(ws, json, stable()).await {
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
                return End::Reenrolled;
            }
            () = token.cancelled() => {
                close_link(ws, close::RESTARTING, "agent stopping").await;
                return End::Shutdown;
            }
        }
    }
}

/// Handles a frame from Console; `Some` ends the session.
async fn handle(ws: &mut Ws, json: &str, stable: bool) -> Option<End> {
    let frame = match serde_json::from_str::<Frame>(json) {
        Ok(frame) => frame,
        Err(e) => {
            debug!("ignoring an unreadable frame: {e}");
            return None;
        }
    };
    let reply = match frame {
        // Operations and topics land with the capabilities.
        Frame::Req(request) => Response::err(
            request.id,
            ErrorCode::UnknownOp,
            "this agent doesn't run operations yet",
        ),
        Frame::Sub(subscribe) => Response::err(
            subscribe.id,
            ErrorCode::UnknownOp,
            "this agent doesn't serve topics yet",
        ),
        Frame::Goaway(goaway) => return Some(go_away(ws, &goaway, stable).await),
        other => {
            debug!(?other, "ignoring a frame");
            return None;
        }
    };
    send(ws, &Frame::Res(reply)).await.err()
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
        close::REVOKED => stop("Console revoked this device; enroll it again to reconnect"),
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

#[cfg(test)]
mod tests {
    use super::*;

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
