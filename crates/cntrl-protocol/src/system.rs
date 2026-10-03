//! Types for `system.*` operations.

use serde::{Deserialize, Serialize};

/// Parameters of operations that take none: `{}` or an absent `data`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NoParams {}

/// What `system.info` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SystemInfo {
    pub hostname: String,
    pub os: OsInfo,
    /// CPU architecture as Rust names it, such as `x86_64` or `aarch64`.
    pub arch: String,
    /// Kernel release, as `uname -r` prints it.
    pub kernel: String,
    /// When the machine booted, in Unix seconds.
    pub boot_time: u64,
    pub agent_version: String,
}

/// The OS, from os-release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OsInfo {
    /// `ID`, such as `ubuntu`.
    pub id: String,
    /// `PRETTY_NAME`, such as `Ubuntu 24.04.1 LTS`.
    pub name: String,
    /// `VERSION_ID`, such as `24.04`; absent on rolling releases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}
