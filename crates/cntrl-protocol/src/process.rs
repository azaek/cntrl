//! Types for processes (D24): the `processes` topic, live only while someone
//! watches, and `process.signal`, which stops a process if it's still the one
//! the caller saw.

use serde::{Deserialize, Serialize};

/// What a `processes` subscription asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProcessesParams {
    /// Highest CPU first (the default), or highest memory.
    #[serde(default)]
    pub sort: ProcessSort,
    /// At most this many rows, 1 to 500; 50 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Keeps the processes whose name, user or unit contains it, ignoring
    /// case, or whose PID it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Keeps only the system's processes, or only people's (D60), before the
    /// limit. Agents before 0.1.17 keep every process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ProcessOwner>,
}

/// Whose processes a subscription keeps (D60).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProcessOwner {
    /// The operating system's: kernel threads, and processes that run as
    /// root, SYSTEM or another system or service account.
    System,
    /// People's: processes that run as a person's account.
    User,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProcessSort {
    #[default]
    Cpu,
    Memory,
}

/// One `processes` event: the top of the process table, as the subscription
/// asked for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProcessesSample {
    /// When the table was read, in Unix milliseconds.
    pub ts: u64,
    /// Processes on the machine.
    pub total: u32,
    /// Processes that matched the query, before the limit.
    pub matched: u32,
    pub processes: Vec<ProcessInfo>,
}

/// One process, with its threads folded in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProcessInfo {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    pub name: String,
    /// Who it runs as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// CPU since the previous reading, as a percentage of one core, so it can
    /// pass 100 on a machine with several.
    pub cpu: f64,
    /// Resident memory, in bytes.
    pub memory: u64,
    /// When it started, in Unix seconds. With `pid`, it names the process to
    /// `process.signal`.
    pub started: u64,
    /// The systemd unit it runs in, on Linux.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// A kernel thread, which can't be stopped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kernel: bool,
    /// It runs as the operating system, not a person (D60): a kernel thread,
    /// or root, SYSTEM or another system or service account. Agents before
    /// 0.1.17 leave it false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub system: bool,
    /// The device won't stop it: it's part of the operating system, it's the
    /// agent, or its unit is one the device policy protects.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub protected: bool,
}

/// `process.signal`: one process to stop, as the process table named it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProcessSignal {
    pub pid: u32,
    /// Its start time from the process table, in Unix seconds. A process that
    /// started at another time has taken the PID over, and is left alone.
    pub started: u64,
    /// Kill it at once with SIGKILL, losing anything unsaved. Otherwise it gets
    /// SIGTERM and a few seconds to exit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
}

/// How a stop went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProcessSignalResult {
    pub pid: u32,
    pub result: StopResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StopResult {
    /// It exited.
    Stopped,
    /// It's still running after the wait: it may be ignoring SIGTERM.
    StillRunning,
    /// It had already exited, or its PID now names another process.
    NotRunning,
    #[serde(other)]
    Unknown,
}
