//! Types for the `logs` topic (angle 11): a service's or the system's log,
//! the latest lines first, then new ones as they come, only while subscribed.
//! Nothing is kept anywhere but the viewer's screen (D26).

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
}

/// Log lines, in the order they were logged.
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
