//! What the outbox carries: records Console must receive at least once, sent in
//! `rec` frames and confirmed by `ack`. A record's `type` says which of these its
//! `data` is. Ratios run from 0.0 to 1.0, sizes are bytes and timestamps are
//! milliseconds since the Unix epoch.

use serde::{Deserialize, Serialize};

use crate::stats::{LoadAverage, MemoryStats};

/// `metrics`: host stats over about a minute. CPU is averaged from counter
/// deltas across the period, with its busiest sample beside it; load and memory
/// are the last values read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StatsRecord {
    /// When the period ended.
    pub ts: u64,
    /// How long the period was, in milliseconds.
    pub period_ms: u32,
    pub cpu: CpuRecord,
    pub memory: MemoryStats,
}

/// CPU over a record's period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CpuRecord {
    /// Busy time across all cores over the whole period.
    pub busy: f64,
    /// The busiest sample in the period.
    pub busy_max: f64,
    pub load: LoadAverage,
}

/// `audit_checkpoint`: privd's signature over the head of the device's audit
/// log. Console keeps them, so the log can later be checked against them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuditCheckpoint {
    pub device_id: String,
    /// The sequence number of the last record it covers.
    pub seq: u64,
    /// SHA-256 of that record's line, lowercase hex.
    pub head: String,
    /// When privd signed it.
    pub ts: u64,
    /// The audit key's ID, from enrollment.
    pub key_id: String,
    /// ES256 over [`checkpoint_signing_string`] with the audit key, base64url.
    pub sig: String,
}

/// The string the audit key signs in an [`AuditCheckpoint`].
pub fn checkpoint_signing_string(
    device_id: &str,
    seq: u64,
    head: &str,
    ts: u64,
    key_id: &str,
) -> String {
    format!("cntrl-audit-checkpoint-v1\n{device_id}\n{seq}\n{head}\n{ts}\n{key_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signing_string_binds_every_field() {
        assert_eq!(
            checkpoint_signing_string("dev_1", 41, "ab", 1_700_000_000_000, "key_1"),
            "cntrl-audit-checkpoint-v1\ndev_1\n41\nab\n1700000000000\nkey_1"
        );
    }
}

/// An `alert` record: a rule this device decides fired or resolved (D43). The
/// outbox keeps it until the hub has it, so it survives a dropped link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AlertRecord {
    pub rule: String,
    /// The rule's `rev` the agent judged it by.
    pub rev: u64,
    pub state: AlertState,
    /// When the condition began, in Unix milliseconds.
    pub since: u64,
    /// When the agent decided, in Unix milliseconds.
    pub at: u64,
    /// The minute's reading that decided it, in the rule's unit; absent for a
    /// service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// On resolving, the furthest past the line it went while firing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AlertState {
    Firing,
    Resolved,
}
