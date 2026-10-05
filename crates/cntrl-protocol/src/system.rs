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
    /// The machine, such as `Mac mini (Mac16,10)`, `Dell Inc. OptiPlex 7090`
    /// or `Raspberry Pi 4 Model B Rev 1.4`, where the OS says. From 0.1.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// From 0.1.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuInfo>,
    /// The form factor, in systemd's words (machine-info(5)): `desktop`,
    /// `laptop`, `convertible`, `server`, `tablet`, `handset`, `watch`,
    /// `embedded`, `vm` or `container`, where the machine says. From 0.1.10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chassis: Option<String>,
}

/// The CPU.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CpuInfo {
    /// Such as `Apple M4` or `AMD Ryzen 7 5800X 8-Core Processor`; ARM boards
    /// often don't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Physical cores.
    pub cores: u32,
    /// Hardware threads, which the OS schedules as CPUs.
    pub threads: u32,
    /// On CPUs with two kinds of core, such as Apple silicon, how many of each.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance_cores: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efficiency_cores: Option<u32>,
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
