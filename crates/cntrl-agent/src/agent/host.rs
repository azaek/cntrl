//! What the agent reports about the machine it runs on.

use cntrl_host::machine;
use cntrl_protocol::enroll::HostInfo;

use super::digest::sha256_hex;

pub fn host_info() -> HostInfo {
    HostInfo {
        hostname: machine::hostname().unwrap_or_else(|| "unknown".to_owned()),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        machine_id_hash: machine_id_hash(),
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// The Rust target triple this binary was built for.
pub const TARGET: &str = env!("CNTRL_TARGET");

/// SHA-256 of the machine ID, never the raw ID; empty without one.
pub fn machine_id_hash() -> String {
    machine::machine_id()
        .map(|id| sha256_hex(id.as_bytes()))
        .unwrap_or_default()
}

/// Changes on every boot.
pub fn boot_id() -> String {
    machine::boot_id().unwrap_or_default()
}
