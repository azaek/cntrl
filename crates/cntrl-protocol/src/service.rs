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

/// `service.list`: the services the device's service manager knows at system
/// scope (systemd's system units; launchd's system domain), by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServiceList {
    pub services: Vec<ServiceStatus>,
}

/// One service and how it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServiceStatus {
    /// The name `service.restart` takes: a unit such as `nginx.service`, or a
    /// launchd label.
    pub unit: String,
    /// What it is, when the service manager says: systemd's description.
    /// launchd keeps none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub state: ServiceState,
    /// The service manager's own words for the state, such as systemd's
    /// `active (exited)` or launchd's last exit status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The main process, while it runs and the service manager reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// The device policy keeps service actions off it.
    pub protected: bool,
}

/// A service's state across service managers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Its process is running.
    Running,
    /// It ran and finished cleanly, such as a oneshot unit or an on-demand job
    /// between requests.
    Exited,
    /// Loaded but not running, and not since it last stopped cleanly.
    Stopped,
    /// It stopped with an error.
    Failed,
    Starting,
    Stopping,
    #[serde(other)]
    Unknown,
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
