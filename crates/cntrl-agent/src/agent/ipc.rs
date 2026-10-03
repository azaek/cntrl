//! The channel between the agent and privd: length-delimited JSON frames over
//! privd's Unix socket. Each request carries an ID that its response repeats.

use std::path::Path;
use std::time::Duration;

use bytes::Bytes;
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
}

impl Response {
    pub fn from_result(id: u64, result: Result<Value, String>) -> Self {
        match result {
            Ok(value) => Self {
                id,
                ok: Some(value),
                err: None,
            },
            Err(e) => Self {
                id,
                ok: None,
                err: Some(e),
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
    let exchange = async {
        let stream = UnixStream::connect(socket)
            .await
            .map_err(|e| format!("can't reach privd at {}: {e}", socket.display()))?;
        let mut channel = channel(stream);
        send(&mut channel, &Request { id: 1, call }).await?;
        let response: Response = receive(&mut channel)
            .await?
            .ok_or("privd closed the connection")?;
        match (response.id, response.ok, response.err) {
            (_, _, Some(e)) => Err(e),
            (1, ok, None) => Ok(ok.unwrap_or(Value::Null)),
            (id, _, None) => Err(format!("privd answered request {id}, not 1")),
        }
    };
    tokio::time::timeout(CALL_TIMEOUT, exchange)
        .await
        .map_err(|_| "privd didn't answer in time".to_owned())?
}
