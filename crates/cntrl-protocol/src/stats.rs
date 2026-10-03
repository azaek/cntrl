//! Types for the `stats` topic. Ratios run from 0.0 to 1.0, sizes are bytes,
//! rates are bytes per second, temperatures are degrees Celsius and timestamps
//! are milliseconds since the Unix epoch. Fields after `memory` arrived in
//! 0.1.4; each is absent where the machine has nothing to report.

use serde::{Deserialize, Serialize};

/// Parameters of the `stats` topic.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StatsParams {
    /// Sample interval; the agent raises anything below 1,000 ms to 1,000 ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u32>,
}

/// One live sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StatsSample {
    /// When the sample was taken.
    pub ts: u64,
    pub cpu: CpuStats,
    pub memory: MemoryStats,
    /// Absent when the machine has no swap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap: Option<SwapStats>,
    /// Reads and writes across the machine's physical disks, over the sample
    /// interval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_io: Option<DiskIo>,
    /// Traffic across the machine's physical network interfaces, over the
    /// sample interval. Tunnels and container networks aren't added, since
    /// their traffic crosses a physical interface too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkIo>,
    /// The machine's filesystems, as of the last look; the agent looks every
    /// 10 s.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystems: Vec<Filesystem>,
    /// Temperature sensors, as of the last look; the agent looks every 5 s.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub temperatures: Vec<Temperature>,
}

/// CPU use over the sample interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CpuStats {
    /// Busy time across all cores.
    pub busy: f64,
    pub load: LoadAverage,
}

/// Load averages over 1, 5 and 15 minutes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

/// Memory at sample time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MemoryStats {
    pub total: u64,
    /// What the kernel estimates new work can use (`MemAvailable`).
    pub available: u64,
}

/// Swap at sample time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SwapStats {
    pub total: u64,
    pub used: u64,
}

/// Disk throughput, in bytes per second.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DiskIo {
    pub read: u64,
    pub write: u64,
}

/// Network throughput, in bytes per second.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NetworkIo {
    pub received: u64,
    pub sent: u64,
}

/// A mounted filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Filesystem {
    /// Where it's mounted, such as `/` or `/mnt/media`.
    pub mount: String,
    /// The volume's name where the OS gives one, such as `Macintosh HD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its type, such as `ext4`, `zfs`, `nfs4` or `apfs`.
    pub kind: String,
    pub total: u64,
    /// In use. On a Mac it's the whole APFS container's use, since its volumes
    /// share the space.
    pub used: u64,
    /// What a user other than root can still write; on Linux the blocks kept
    /// for root aren't in it, so used and available don't add up to the total.
    pub available: u64,
}

/// One temperature sensor's reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Temperature {
    pub sensor: SensorKind,
    /// The sensor's name for people, such as `CPU` or `nvme0`.
    pub label: String,
    pub celsius: f64,
}

/// What a temperature sensor measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    /// The CPU, or the chip on Apple silicon.
    Cpu,
    Disk,
    Gpu,
    /// Anything else, including kinds from newer agents.
    #[serde(other)]
    Other,
}
