//! Types for `service.*` operations.

use serde::{Deserialize, Serialize};

/// Names one service: a systemd unit such as `nginx.service`, or on macOS a
/// launchd job's label, such as `homebrew.mxcl.nginx`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServiceRef {
    pub unit: String,
}

/// The outcome of a service job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServiceJob {
    pub unit: String,
    pub result: JobResult,
}

/// A job result, as systemd reports it in `JobRemoved`. On macOS it is `done`
/// once launchd shows the job running again (or finished cleanly), `failed` if
/// it exited with an error, and `timeout` if launchd doesn't say in time.
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
