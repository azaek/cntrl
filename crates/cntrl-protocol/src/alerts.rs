//! Alert rules a device decides itself (D43; plans/alerts.md phase 2): a
//! reading beyond a line for some minutes in a row, and a service that isn't
//! running. The hub sends a device the rules that cover it in an `alerts`
//! frame, the whole set each time; the agent reports firing and resolving as
//! `alert` outbox records (`records::AlertRecord`). Offline rules stay in the
//! hub.

use serde::{Deserialize, Serialize};

/// Every rule this device decides, replacing any before.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AlertRules {
    pub rules: Vec<AlertRule>,
}

/// One rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AlertRule {
    pub id: String,
    /// When the rule last changed, in Unix milliseconds: a changed rule starts
    /// over, resolving what it had firing.
    pub rev: u64,
    pub kind: AlertRuleKind,
    /// How many minutes in a row the condition has to hold.
    pub minutes: u32,
    /// For a `metric` rule, the reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<AlertMetric>,
    /// What the reading is of: a mount point for a disk, a sensor kind (`cpu`,
    /// `gpu`, `disk`) for a temperature, a GPU's name; absent for any of them.
    /// For a `service` rule, the service's name, as `service.restart` takes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<AlertOp>,
    /// The line, in the metric's unit: percent for `cpu`, `memory`, `swap`,
    /// `disk_used`, `gpu_busy` and `gpu_memory`; GiB for `disk_free`; the load
    /// average itself for `load`; degrees Celsius for temperatures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AlertRuleKind {
    /// A reading beyond a line.
    Metric,
    /// A service that isn't running.
    Service,
    /// A kind from a newer hub; the agent skips the rule.
    #[serde(other)]
    Unknown,
}

/// A reading a rule watches, judged by each minute's average.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AlertMetric {
    Cpu,
    Memory,
    Swap,
    DiskUsed,
    DiskFree,
    Load,
    Temperature,
    GpuBusy,
    GpuMemory,
    GpuTemperature,
    /// A reading from a newer hub; the agent skips the rule.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AlertOp {
    Above,
    Below,
}
