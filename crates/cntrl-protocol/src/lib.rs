//! Wire protocol for the cntrl agent: the envelope, the operations registry,
//! and error and close codes. Shared by the agent and, through generated types,
//! by the gateway and the SDKs. No OS code and no async runtime.

/// Major version of the agent protocol, carried in the WebSocket subprotocol
/// `cntrl.agent.v1.json`.
pub const PROTOCOL_VERSION: u32 = 1;
