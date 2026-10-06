//! Wire protocol for the cntrl agent: the envelope, the operations registry,
//! and error and close codes. Shared by the agent and, through generated types,
//! by the gateway and the SDKs. No OS code and no async runtime.

pub mod alerts;
pub mod app;
pub mod auth;
pub mod capability;
pub mod codes;
pub mod enroll;
pub mod frame;
pub mod history;
pub mod logs;
pub mod network;
pub mod ops;
pub mod power;
pub mod process;
pub mod records;
mod registry;
pub mod service;
pub mod stats;
pub mod storage;
pub mod system;

pub use capability::{CAPABILITIES, MONITOR_ONLY};
pub use codes::{ErrorCode, close};
pub use frame::Frame;
pub use registry::{DecodeError, OpInfo, TopicInfo};
#[cfg(feature = "schema")]
pub use registry::{OpSchema, TopicSchema};

/// Major version of the agent protocol.
pub const PROTOCOL_VERSION: u32 = 1;

/// WebSocket subprotocol that carries [`PROTOCOL_VERSION`].
pub const SUBPROTOCOL: &str = "cntrl.agent.v1.json";

/// Largest text frame either side accepts, in bytes.
pub const MAX_FRAME_BYTES: u32 = 1_048_576;

/// Heartbeat frame sent by the agent. It is literal text rather than JSON, so a
/// hibernating gateway can answer it without waking.
pub const PING: &str = "ping";

/// The gateway's answer to [`PING`].
pub const PONG: &str = "pong";
