//! The channel between the agent and privd: length-delimited JSON frames over
//! privd's Unix socket. Each request carries an ID that its response repeats.

use std::path::Path;
use std::time::Duration;

use bytes::Bytes;
use cntrl_host::HostError;
use cntrl_protocol::codes::ErrorCode;
use cntrl_protocol::frame::Actor;
use cntrl_protocol::service::ServiceScope;
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Largest frame either side accepts.
const MAX_FRAME: usize = 1 << 20;
/// How long the agent waits for privd, socket activation included.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// What the agent asks privd to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Call {
    Ping,
    /// The policy in force, as privd reads it.
    PolicyShow,
    /// Appends a record from the agent to the audit log.
    AuditAppend {
        kind: String,
        data: Value,
    },
    /// The public half of privd's audit key, created on first use.
    AuditKey,
    /// Restarts a unit for a request from Console. privd checks the policy
    /// itself and audits its decision before acting.
    ServiceRestart {
        /// The request's ID, which is also its audit ID. Not `id`: the call is
        /// flattened into an envelope that has one.
        request_id: String,
        unit: String,
        #[serde(default, skip_serializing_if = "ServiceScope::is_system")]
        scope: ServiceScope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<Actor>,
    },
    /// What runs in each logged-in user's desktop session, which only root can
    /// read: on macOS, their LaunchAgents and open apps.
    ServiceListSessions,
    /// Quits an app in a user's session for a request from Console. privd
    /// checks the policy itself and audits its decision before acting.
    AppQuit {
        request_id: String,
        app: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<Actor>,
    },
    /// A signed checkpoint over the audit log's head, or `null` when the log
    /// hasn't grown since the checkpoint at `after`.
    AuditCheckpoint {
        device_id: String,
        /// The audit key's ID, from enrollment.
        key_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<u64>,
    },
}

/// Why privd refused or failed a call, with the protocol code to answer with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
    pub code: ErrorCode,
    pub msg: String,
}

impl CallError {
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
        }
    }

    /// A failure no other code describes.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, msg)
    }
}

impl From<HostError> for CallError {
    fn from(error: HostError) -> Self {
        let code = match &error {
            HostError::Invalid(_) => ErrorCode::BadRequest,
            HostError::NotFound(_) => ErrorCode::NotFound,
            HostError::Unsupported | HostError::Failed(_) => ErrorCode::Internal,
        };
        Self::new(code, error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub call: Call,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
    /// The protocol code for `err`; absent means `internal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<ErrorCode>,
}

impl Response {
    pub fn from_result(id: u64, result: Result<Value, CallError>) -> Self {
        match result {
            Ok(value) => Self {
                id,
                ok: Some(value),
                err: None,
                code: None,
            },
            Err(e) => Self {
                id,
                ok: None,
                err: Some(e.msg),
                code: Some(e.code),
            },
        }
    }
}

pub type Channel = Framed<UnixStream, LengthDelimitedCodec>;

pub fn channel(stream: UnixStream) -> Channel {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME)
        .new_codec();
    Framed::new(stream, codec)
}

pub async fn send<T: Serialize>(channel: &mut Channel, message: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    channel
        .send(Bytes::from(bytes))
        .await
        .map_err(|e| e.to_string())
}

/// The next message, or `None` once the other side has closed the channel.
pub async fn receive<T: DeserializeOwned>(channel: &mut Channel) -> Result<Option<T>, String> {
    match channel.next().await {
        None => Ok(None),
        Some(Err(e)) => Err(e.to_string()),
        Some(Ok(frame)) => serde_json::from_slice(&frame)
            .map(Some)
            .map_err(|e| format!("bad frame: {e}")),
    }
}

/// Connects, makes one call and disconnects, within [`CALL_TIMEOUT`].
pub async fn call_once(socket: &Path, call: Call) -> Result<Value, String> {
    call_within(socket, call, CALL_TIMEOUT)
        .await
        .map_err(|e| e.msg)
}

/// Connects, makes one call and disconnects, within `limit`.
pub async fn call_within(socket: &Path, call: Call, limit: Duration) -> Result<Value, CallError> {
    let exchange = async {
        let stream = UnixStream::connect(socket).await.map_err(|e| {
            CallError::internal(format!("can't reach privd at {}: {e}", socket.display()))
        })?;
        let mut channel = channel(stream);
        send(&mut channel, &Request { id: 1, call })
            .await
            .map_err(CallError::internal)?;
        let response: Response = receive(&mut channel)
            .await
            .map_err(CallError::internal)?
            .ok_or_else(|| CallError::internal("privd closed the connection"))?;
        match (response.id, response.ok, response.err) {
            (_, _, Some(e)) => Err(CallError::new(
                response.code.unwrap_or(ErrorCode::Internal),
                e,
            )),
            (1, ok, None) => Ok(ok.unwrap_or(Value::Null)),
            (id, _, None) => Err(CallError::internal(format!(
                "privd answered request {id}, not 1"
            ))),
        }
    };
    tokio::time::timeout(limit, exchange).await.map_err(|_| {
        CallError::new(
            ErrorCode::Timeout,
            format!("privd didn't answer within {limit:?}"),
        )
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_call_round_trips_through_the_envelope() {
        // Calls are flattened into the request, so a field named `id` would collide.
        let calls = [
            Call::Ping,
            Call::PolicyShow,
            Call::AuditAppend {
                kind: "agent.started".to_owned(),
                data: Value::Null,
            },
            Call::AuditKey,
            Call::ServiceRestart {
                request_id: "req_1".to_owned(),
                unit: "nginx.service".to_owned(),
                scope: ServiceScope::System,
                user: None,
                actor: None,
            },
            Call::ServiceRestart {
                request_id: "req_2".to_owned(),
                unit: "com.azaek.tmux".to_owned(),
                scope: ServiceScope::User,
                user: Some("azaek".to_owned()),
                actor: None,
            },
            Call::ServiceListSessions,
            Call::AppQuit {
                request_id: "req_3".to_owned(),
                app: "com.azaek.head".to_owned(),
                user: None,
                force: true,
                actor: None,
            },
            Call::AuditCheckpoint {
                device_id: "dev_1".to_owned(),
                key_id: "key_1".to_owned(),
                after: Some(3),
            },
        ];
        for call in calls {
            let request = Request { id: 7, call };
            let json = serde_json::to_string(&request).expect("encodes");
            let back: Request = serde_json::from_str(&json).expect("decodes");
            assert_eq!(back, request, "{json}");
        }
    }
}
