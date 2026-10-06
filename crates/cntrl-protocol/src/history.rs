//! Types for the history each device keeps (D52, angle 17): `history.read`,
//! for charts, and `history.keep` and `history.clear`, for how much the device
//! keeps. Console stores no readings (D26); it asks the device.

use serde::{Deserialize, Serialize};

/// The most columns `history.read` cuts a span into.
pub const COLUMNS_MAX: u32 = 500;
/// The most days a device keeps.
pub const KEEP_DAYS_MAX: u32 = 365;
/// How many days a device keeps until told otherwise.
pub const KEEP_DAYS_DEFAULT: u32 = 90;

/// What `history.read` asks for: a span of time, cut into columns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HistoryParams {
    /// Where the span starts, in Unix milliseconds.
    pub from: u64,
    /// Where it ends, in Unix milliseconds.
    pub to: u64,
    /// How many columns to cut it into, from 1 to 500.
    pub columns: u32,
}

/// A span of the device's history, cut into columns.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct History {
    pub from: u64,
    pub to: u64,
    pub columns: u32,
    /// Each metric the device recorded, with one entry per column.
    pub series: Vec<HistorySeries>,
    pub store: HistoryStore,
}

/// One metric across a span's columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HistorySeries {
    pub metric: HistoryMetric,
    /// Oldest first; null where nothing was recorded.
    pub spans: Vec<Option<Span>>,
}

/// A metric over one column: its lowest and highest reading and their average.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Span {
    pub min: f64,
    pub max: f64,
    pub avg: f64,
}

/// What the device keeps a history of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum HistoryMetric {
    /// CPU in use, from 0 to 1.
    Cpu,
    /// Memory in use, from 0 to 1.
    Memory,
    /// Swap in use, from 0 to 1.
    Swap,
    /// The 1-minute load average.
    Load,
    /// Bytes per second on the physical interfaces, in and out together.
    Network,
    /// Bytes per second on the physical disks, read and written together.
    Disk,
    /// The fullest filesystem's used share, from 0 to 1.
    DiskUsed,
    /// The CPU's temperature, in degrees Celsius.
    Temperature,
    /// The busiest GPU, from 0 to 1.
    Gpu,
    /// A metric from a newer agent.
    #[serde(other)]
    Unknown,
}

/// What the device keeps, and the room it takes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HistoryStore {
    /// How many days of 15-minute points it keeps; 1-minute points are kept
    /// for 48 hours.
    pub keep_days: u32,
    /// The oldest point kept, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest: Option<u64>,
    /// Bytes on disk.
    pub bytes: u64,
}

/// What `history.keep` asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HistoryKeep {
    /// How many days of history to keep, from 1 to 365. Fewer than before
    /// deletes the older days at once.
    pub days: u32,
}
