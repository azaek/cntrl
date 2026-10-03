//! What the agent reports about the machine it runs on.

use std::fs;

use cntrl_protocol::enroll::HostInfo;

use super::digest::sha256_hex;

pub fn host_info() -> HostInfo {
    HostInfo {
        hostname: read_trimmed("/proc/sys/kernel/hostname").unwrap_or_else(|| "unknown".to_owned()),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        machine_id_hash: read_trimmed("/etc/machine-id")
            .map(|id| sha256_hex(id.as_bytes()))
            .unwrap_or_default(),
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}
