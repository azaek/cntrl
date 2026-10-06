//! Types for the Containers tab (D54, angle 19): the `containers` topic, the
//! machine's Docker or Podman containers with what each uses, read only while
//! someone watches; and starting, stopping and restarting one.

use serde::{Deserialize, Serialize};

/// What a `containers` subscription asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContainersParams {
    /// How often to send, in milliseconds: 3000 when absent, 2000 at least.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u32>,
}

/// One `containers` event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContainersSample {
    /// When it was read, in Unix milliseconds.
    pub ts: u64,
    /// The engine that answered; none when there's none to ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Engine>,
    pub containers: Vec<Container>,
    /// Why there's no list, in a few words: no engine found, or it didn't
    /// answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A container engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Engine {
    pub kind: EngineKind,
    /// As the engine gives it, such as `29.8.2`.
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Docker,
    Podman,
    /// An engine from a newer agent.
    #[serde(other)]
    Unknown,
}

/// A container, and for a running one what it used since the previous
/// reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Container {
    /// The first 12 characters of its ID, as `docker ps` shows them.
    pub id: String,
    pub name: String,
    pub image: String,
    /// Its Compose project, for a container Compose made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Its service in that project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    pub state: ContainerState,
    /// What its health check says, for one that has a check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ContainerHealth>,
    /// The engine's own words, such as `Up 3 hours (healthy)`.
    pub status: String,
    /// When it was made, in Unix milliseconds.
    pub created: u64,
    pub ports: Vec<ContainerPort>,
    /// Its share of the whole machine's CPU, from 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<f64>,
    /// Memory in use, without the file cache, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<u64>,
    /// What it may use, in bytes: its limit, or the machine's memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<u64>,
    /// Bytes per second in and out, over all its networks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<ContainerNetwork>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
    /// A state from a newer engine or agent.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ContainerHealth {
    Starting,
    Healthy,
    Unhealthy,
    /// A health from a newer engine or agent.
    #[serde(other)]
    Unknown,
}

/// A port a container exposes, and where the machine publishes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContainerPort {
    /// The machine's address it's published on; absent for every address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// The port inside the container.
    pub private: u16,
    /// The machine's port, when it's published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<u16>,
    /// `tcp`, `udp` or `sctp`.
    pub protocol: String,
}

/// Bytes per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContainerNetwork {
    pub received: u64,
    pub sent: u64,
}

/// The container `container.start`, `container.stop` and `container.restart`
/// act on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContainerRef {
    /// Its ID, or the first 12 characters of it, or its name.
    pub id: String,
}

/// How an action on a container went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContainerJob {
    pub id: String,
    /// Its state once the engine was done.
    pub state: ContainerState,
}
