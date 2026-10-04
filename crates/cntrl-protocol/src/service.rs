//! Types for `service.*` operations.

use serde::{Deserialize, Serialize};

/// Names one service: a systemd unit such as `nginx.service`, or on macOS a
/// launchd job's label, such as `homebrew.mxcl.nginx`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServiceRef {
    pub unit: String,
    /// Where the service runs; system-wide when absent.
    #[serde(default, skip_serializing_if = "ServiceScope::is_system")]
    pub scope: ServiceScope,
    /// For `user` scope, whose session; the user at the console when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// Where a service runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    /// System-wide: systemd's system units, launchd's system domain.
    #[default]
    System,
    /// A logged-in user's session: on macOS, their GUI domain, with their
    /// LaunchAgents and the apps they have open.
    User,
}

impl ServiceScope {
    pub fn is_system(&self) -> bool {
        *self == Self::System
    }
}

/// What a service is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    /// A daemon, agent or unit that the service manager runs.
    #[default]
    Service,
    /// An app open in a user's session. Its `unit` is the app's bundle ID and
    /// its `description` the app's name.
    App,
}

impl ServiceKind {
    pub fn is_service(&self) -> bool {
        *self == Self::Service
    }
}

/// The outcome of a service job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServiceJob {
    pub unit: String,
    pub result: JobResult,
}

/// `service.list`: the services the device's service manager knows: system-wide
/// (systemd's system units; launchd's system domain), then on macOS what runs
/// in each logged-in user's session.
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
    /// Where it runs; system-wide when absent.
    #[serde(default, skip_serializing_if = "ServiceScope::is_system")]
    pub scope: ServiceScope,
    /// For `user` scope, whose session it runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// What it is; a service when absent.
    #[serde(default, skip_serializing_if = "ServiceKind::is_service")]
    pub kind: ServiceKind,
    /// Whether it starts at boot, where that can be changed: a systemd unit
    /// file that's enabled or disabled, or a third-party LaunchDaemon on a Mac.
    /// Absent for units that can't be enabled (systemd's `static` ones), for
    /// Apple's own jobs, and from agents before 0.1.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// What `service.start`, `service.stop`, `service.restart`, `service.enable`
/// and `service.disable` do (angle 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    /// Starts at boot; on a Mac it also loads the job now.
    Enable,
    /// Doesn't start at boot; on a Mac it also unloads the job now.
    Disable,
}

impl ServiceAction {
    /// Its operation.
    pub fn op(self) -> &'static str {
        match self {
            Self::Start => "service.start",
            Self::Stop => "service.stop",
            Self::Restart => "service.restart",
            Self::Enable => "service.enable",
            Self::Disable => "service.disable",
        }
    }

    /// Whether it can cut the machine off when it's one the policy protects,
    /// such as SSH: stopping, restarting and disabling can; starting and
    /// enabling can't.
    pub fn interrupts(self) -> bool {
        matches!(self, Self::Stop | Self::Restart | Self::Disable)
    }
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
