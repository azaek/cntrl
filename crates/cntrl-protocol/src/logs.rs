//! Types for the `logs` topic (angle 11): a service's or the system's log,
//! the latest lines first, then new ones as they come, only while subscribed.
//! Nothing is kept anywhere but the viewer's screen (D26).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Parameters of the `logs` topic.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LogsParams {
    /// One service's log, such as `nginx.service`; the whole system's when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// The user whose session runs `unit`, for a user's LaunchAgent on a Mac;
    /// the system's service when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// A container's log instead, by its ID or name (D54); it needs
    /// `containers.read` as well.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// The least important to show, as a syslog priority: 0 (emergency) to
    /// 7 (debug); everything when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    /// Only lines whose message contains this, ignoring case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grep: Option<String>,
    /// How many earlier lines come first: 0 to 1,000, and 100 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// Whether the whole system's log takes in the operating system's own
    /// processes: on a Mac, Apple's processes and subsystems and the kernel,
    /// which make nearly all of it. Everything comes when absent; Linux takes
    /// everything either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_os: Option<bool>,
    /// Only lines from these sources, named as entries name them; every
    /// source when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    /// No lines from these sources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hide: Vec<String>,
}

/// Log lines, in the order they were logged. From agent 0.1.18 the first
/// batch comes once the earlier lines are all read, with none in it when
/// there weren't any, so a viewer can tell an empty log from one whose lines
/// are still on their way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LogsBatch {
    pub entries: Vec<LogEntry>,
    /// Lines left out since the last batch because they came faster than the
    /// agent sends them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skipped: u64,
    /// Why no more lines will come, such as the log reader stopping; absent
    /// while the log is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
    /// While new lines come faster than the agent sends them, it sends an
    /// even sample of about one in this many; absent while it sends them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_in: Option<u32>,
    /// How many new lines each source logged since the last batch, sampled
    /// or not: the busiest 20.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<String, u64>,
}

/// One log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LogEntry {
    /// When it was logged, in milliseconds since the Unix epoch; 0 when
    /// that isn't known, as for the earlier lines of a service's output file.
    pub ts: u64,
    /// Its syslog priority: 0 (emergency) to 7 (debug).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    /// Who logged it: the service, the program's name, or the output file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// The message, cut at 4 KiB.
    pub message: String,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}
