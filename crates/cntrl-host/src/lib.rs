//! Host capabilities for the cntrl agent: one trait per capability (stats,
//! processes, power, services) with a backend per OS. Returns `cntrl-protocol`
//! types and does no networking.

use std::fmt;

pub mod linux;
pub mod stats;

/// Why a host capability couldn't answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// This OS has no backend for the capability yet.
    Unsupported,
    /// The backend failed, such as a file it reads being unreadable.
    Failed(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("this OS isn't supported yet"),
            Self::Failed(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for HostError {}
