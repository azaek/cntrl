//! Types for the `stats` topic. Ratios run from 0.0 to 1.0, sizes are bytes and
//! timestamps are milliseconds since the Unix epoch.

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

/// Memory at sample time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MemoryStats {
    pub total: u64,
    /// What the kernel estimates new work can use (`MemAvailable`).
    pub available: u64,
}
