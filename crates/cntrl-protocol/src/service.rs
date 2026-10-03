//! Types for `service.*` operations.

use serde::{Deserialize, Serialize};

/// Names one systemd unit, such as `nginx.service`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServiceRef {
    pub unit: String,
}

/// The outcome of a unit job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServiceJob {
    pub unit: String,
    pub result: JobResult,
}

/// A job result, as systemd reports it in `JobRemoved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobResult {
    Done,
    Canceled,
    Timeout,
    Failed,
    Dependency,
    Skipped,
    #[serde(other)]
    Unknown,
}
