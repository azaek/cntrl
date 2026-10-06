//! Checks a device runs for its organization (D56, phase B of D55): an
//! HTTP(S) URL or a TCP host and port, from the device's network, where
//! private addresses are. The hub sends a device its checks in a `checks`
//! frame, the whole set each time; the device runs each at its interval while
//! connected and sends results in `check_results` frames. The hub keeps their
//! state and alerts, as it does for the checks it runs itself.

use serde::{Deserialize, Serialize};

/// The most checks a device runs.
pub const CHECKS_MAX: usize = 50;
/// The shortest interval, in seconds.
pub const INTERVAL_MIN_S: u32 = 30;
/// How long a run may take, in milliseconds.
pub const TIMEOUT_MS: u32 = 10_000;
/// Redirects an HTTP check follows.
pub const REDIRECTS: usize = 10;
/// The feature an agent reports in its hello when its policy lets it run
/// checks.
pub const FEATURE: &str = "checks";

/// Every check this device runs, replacing any before.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CheckSet {
    pub checks: Vec<DeviceCheck>,
}

/// One check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeviceCheck {
    pub id: String,
    /// When the check last changed, in Unix milliseconds; results carry it,
    /// so the hub drops a result of a check changed since.
    pub rev: u64,
    pub kind: CheckKind,
    /// An http(s) URL, or `host:port` (`[v6]:port`).
    pub target: String,
    /// Seconds between runs, at least [`INTERVAL_MIN_S`].
    pub interval_s: u32,
    /// For an https URL: accept a certificate that doesn't verify, such as a
    /// self-signed one. Its expiry is still read.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignore_tls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// Up when the final answer, after up to [`REDIRECTS`] redirects, is 2xx or
    /// 3xx within [`TIMEOUT_MS`].
    Http,
    /// Up when the port takes a connection within [`TIMEOUT_MS`].
    Tcp,
    /// A kind from a newer hub; the agent skips the check.
    #[serde(other)]
    Unknown,
}

/// Results of checks this device ran, or why it runs none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CheckResults {
    #[serde(default)]
    pub results: Vec<CheckResult>,
    /// Why the device runs none of the set, such as a policy that doesn't
    /// allow `checks.run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// What one run found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CheckResult {
    pub id: String,
    /// The check's `rev` when it ran.
    pub rev: u64,
    /// When the run started, in Unix milliseconds by the device's clock.
    pub at_ms: u64,
    pub ok: bool,
    /// How long it took to answer, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u32>,
    /// An HTTP check's final status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<u16>,
    /// What failed, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// For an https URL that answered, the server's certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert: Option<CertInfo>,
}

/// The leaf certificate a server presented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CertInfo {
    /// When it expires, in Unix milliseconds.
    pub not_after_ms: u64,
    /// Its subject's common name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Its issuer's organization, or common name without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
}
