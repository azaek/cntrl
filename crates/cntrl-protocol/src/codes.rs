//! Error codes carried in `res.err.code`, and WebSocket close codes.

use serde::{Deserialize, Serialize};

/// Why an operation failed. Stable within a protocol major version; a code from
/// a newer revision decodes as `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    UnknownOp,
    UnsupportedVersion,
    PolicyDenied,
    NotFound,
    Busy,
    Timeout,
    Cancelled,
    Internal,
    DeviceOffline,
    LinkLost,
    /// The device is disabled in Console: its plan doesn't cover it (D86).
    /// The gateway says this; agents never do.
    DeviceDisabled,
    #[serde(other)]
    Unknown,
}

/// WebSocket close codes. 1000 to 1013 are from the IANA registry; 4000 to 4999
/// are private use.
pub mod close {
    /// The agent was turned off or disconnected locally.
    pub const DISCONNECTED_BY_DEVICE: u16 = 1000;
    /// The agent is restarting.
    pub const RESTARTING: u16 = 1001;
    /// The gateway is restarting.
    pub const SERVICE_RESTART: u16 = 1012;
    /// Try again later.
    pub const TRY_LATER: u16 = 1013;
    /// No pong arrived within the heartbeat timeout.
    pub const HEARTBEAT_TIMEOUT: u16 = 4000;
    /// The device credential expired; refresh it and reconnect.
    pub const CREDENTIAL_EXPIRED: u16 = 4001;
    /// The device was revoked; stay disconnected until it is enrolled again.
    pub const REVOKED: u16 = 4003;
    /// A newer session for the same device replaced this one.
    pub const REPLACED: u16 = 4009;
    /// This protocol version is no longer served; check for an update.
    pub const UNSUPPORTED_VERSION: u16 = 4026;
    /// The key generation didn't match, so the device is locked.
    pub const GENERATION_MISMATCH: u16 = 4403;

    /// Every close code with its name, for code generation.
    pub const ALL: &[(&str, u16)] = &[
        ("DISCONNECTED_BY_DEVICE", DISCONNECTED_BY_DEVICE),
        ("RESTARTING", RESTARTING),
        ("SERVICE_RESTART", SERVICE_RESTART),
        ("TRY_LATER", TRY_LATER),
        ("HEARTBEAT_TIMEOUT", HEARTBEAT_TIMEOUT),
        ("CREDENTIAL_EXPIRED", CREDENTIAL_EXPIRED),
        ("REVOKED", REVOKED),
        ("REPLACED", REPLACED),
        ("UNSUPPORTED_VERSION", UNSUPPORTED_VERSION),
        ("GENERATION_MISMATCH", GENERATION_MISMATCH),
    ];
}
