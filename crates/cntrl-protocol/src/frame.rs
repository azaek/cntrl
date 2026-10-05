//! The frames of `cntrl.agent.v1.json`: one JSON object per WebSocket text
//! frame, tagged by `t`. Request, subscription and record payloads stay raw JSON
//! here and are decoded by the registry (`ops::Call`, `ops::Topic`), so a frame
//! naming an unknown operation still parses and can be answered with
//! `unknown_op`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codes::ErrorCode;

/// Any frame of protocol v1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Frame {
    Challenge(Challenge),
    // Boxed: it's by far the largest frame and goes out once per connection,
    // so it shouldn't set the size of every other one.
    Hello(Box<Hello>),
    Welcome(Welcome),
    Req(Request),
    Res(Response),
    Prog(Progress),
    Cancel(Cancel),
    Sub(Subscribe),
    Evt(Event),
    Unsub(Unsubscribe),
    Rec(Records),
    Ack(Ack),
    Goaway(GoAway),
    /// The agent is being paused on its machine (`cntrl pause`): who paused it
    /// and why. The gateway answers `paused`; the agent then hangs up and stays
    /// away until someone resumes it there.
    Pause(Pause),
    /// The gateway has recorded a pause.
    Paused(Paused),
    /// A frame type from a newer protocol revision. Receivers ignore it.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// The gateway's first frame. The agent signs `nonce` in its [`Hello`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Challenge {
    /// Session ID, also covered by the signature.
    pub sid: String,
    pub nonce: String,
}

/// The agent's first frame: who it is, proof of its device key, and what its
/// policy lets Console call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Hello {
    /// Protocol major version.
    pub v: u32,
    pub agent: AgentInfo,
    pub auth: HelloAuth,
    pub caps: Caps,
    pub policy: PolicySummary,
    pub outbox: OutboxState,
}

/// The agent build and the machine it runs on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AgentInfo {
    pub version: String,
    /// Rust target triple of the build, such as `x86_64-unknown-linux-gnu`. It
    /// picks the release artifact for an update.
    pub target: String,
    pub os: String,
    pub arch: String,
    /// Changes on every boot.
    pub boot_id: String,
    /// SHA-256 of the machine ID, never the raw ID.
    pub machine_id_hash: String,
}

/// Proof of possession of the device key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HelloAuth {
    pub device_id: String,
    pub key_id: String,
    pub alg: SigAlg,
    /// Key generation counter. A mismatch locks the device (close 4403).
    #[serde(rename = "gen")]
    pub generation: u64,
    /// Signature over the challenge's sid and nonce, the gateway host, the IDs
    /// and `gen`, base64url without padding.
    pub sig: String,
}

/// Signature algorithm of the device key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SigAlg {
    /// ECDSA with P-256 and SHA-256.
    #[serde(rename = "ES256")]
    Es256,
    #[serde(other)]
    Unknown,
}

/// What the device's policy lets Console call: operation and topic names with
/// the version of each, plus optional protocol features.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Caps {
    pub ops: BTreeMap<String, u32>,
    #[serde(default)]
    pub topics: BTreeMap<String, u32>,
    #[serde(default)]
    pub features: Vec<String>,
}

/// The device policy in force, by hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PolicySummary {
    /// SHA-256 of the normalized policy; empty when no valid policy is in force.
    pub hash: String,
    /// Why no valid policy is in force. The agent then denies everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Where the agent's outbox stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OutboxState {
    pub next_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_unacked: Option<u64>,
}

/// The gateway's answer to a valid [`Hello`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Welcome {
    pub session: String,
    pub hb: HeartbeatConfig,
    /// Highest outbox `seq` Console already has; the agent resends the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_upto: Option<u64>,
    pub limits: Limits,
    /// Subscriptions to restore after a reconnect.
    #[serde(default)]
    pub subs: Vec<Subscribe>,
    /// A renewed connection credential; the agent keeps it for its next connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// How often the agent pings, and how long it waits for a pong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HeartbeatConfig {
    pub interval_ms: u32,
    pub timeout_ms: u32,
}

/// Limits the gateway enforces on this session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Limits {
    pub max_frame_bytes: u32,
    pub max_inflight: u32,
    pub max_rec_batch: u32,
}

/// A call to one operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Request {
    /// ULID, also the audit ID on both sides.
    pub id: String,
    pub op: String,
    pub ver: u32,
    pub deadline_ms: u32,
    #[serde(default)]
    pub data: Value,
    /// Who asked, recorded for audit and never used for authorization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Actor>,
    /// Retries of non-idempotent operations reuse this key, so they act once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idem: Option<String>,
}

/// Who asked for a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Actor {
    pub kind: ActorKind,
    pub id: String,
    /// What carried the request, such as the dashboard or a named API token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// The kind of [`Actor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    User,
    ApiToken,
    System,
    #[serde(other)]
    Unknown,
}

/// The final answer to a [`Request`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<ErrorBody>,
}

impl Response {
    /// A successful response carrying `data`.
    pub fn ok(id: impl Into<String>, data: Value) -> Self {
        Self {
            id: id.into(),
            ok: true,
            data: Some(data),
            err: None,
        }
    }

    /// A failed response that a retry won't fix.
    pub fn err(id: impl Into<String>, code: ErrorCode, msg: impl Into<String>) -> Self {
        let err = ErrorBody {
            code,
            msg: msg.into(),
            retry: false,
        };
        Self {
            id: id.into(),
            ok: false,
            data: None,
            err: Some(err),
        }
    }
}

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub msg: String,
    /// Whether retrying the same request may succeed.
    #[serde(default)]
    pub retry: bool,
}

/// Progress on a running request, before its [`Response`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Progress {
    pub id: String,
    #[serde(default)]
    pub data: Value,
}

/// Asks to stop a running request. It is still answered by a [`Response`], with
/// `cancelled` if it stopped in time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Cancel {
    pub id: String,
}

/// Starts a live topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Subscribe {
    pub id: String,
    pub topic: String,
    pub ver: u32,
    #[serde(default)]
    pub data: Value,
}

/// One message on a subscription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Event {
    pub sub: String,
    pub seq: u64,
    pub data: Value,
}

/// Ends a subscription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Unsubscribe {
    pub id: String,
}

/// Records from the agent's outbox, delivered at least once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Records {
    pub recs: Vec<OutboxRecord>,
}

/// One outbox record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OutboxRecord {
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: RecordKind,
    pub data: Value,
}

/// What an outbox record holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    AuditCheckpoint,
    Metrics,
    #[serde(other)]
    Unknown,
}

/// Confirms every record up to and including `upto`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Ack {
    pub upto: u64,
}

/// Asks the agent to disconnect and come back within a window, for example
/// during a gateway deploy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoAway {
    pub reason: String,
    pub reconnect_after: ReconnectWindow,
}

/// Pausing the agent on its machine, as `cntrl pause` does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Pause {
    /// The account that paused it, as the machine names it.
    pub by: String,
    /// Why, when they said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The gateway's answer to `pause`, once it has recorded it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Paused {}

/// A random delay between `min_ms` and `max_ms` spreads reconnects out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReconnectWindow {
    pub min_ms: u32,
    pub max_ms: u32,
}
