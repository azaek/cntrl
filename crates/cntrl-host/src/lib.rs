//! Host capabilities for the cntrl agent: one trait per capability (stats,
//! processes, power, services) with a backend per OS. Returns `cntrl-protocol`
//! types and does no networking.

use std::fmt;

#[cfg(target_os = "macos")]
pub mod launchd;
pub mod linux;
pub mod machine;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod processes;
pub mod services;
pub mod stats;
pub mod system;
#[cfg(target_os = "linux")]
pub mod systemd;

/// Why a host capability couldn't answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// This OS has no backend for the capability yet.
    Unsupported,
    /// The request names something invalid, such as a malformed unit name.
    Invalid(String),
    /// What the request names doesn't exist, such as an unknown unit.
    NotFound(String),
    /// The backend failed, such as a file it reads being unreadable.
    Failed(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("this OS isn't supported yet"),
            Self::Invalid(reason) | Self::NotFound(reason) | Self::Failed(reason) => {
                f.write_str(reason)
            }
        }
    }
}

impl std::error::Error for HostError {}

/// The backend for an OS that has none yet.
#[derive(Debug, Default)]
pub struct Unsupported;
