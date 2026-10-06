//! The machine and its OS, behind `system.info`.

use std::sync::Arc;

use cntrl_protocol::system::SystemInfo;

use crate::HostError;

/// Reads what the machine is.
pub trait System: Send + Sync {
    /// Describes the machine. It blocks on the OS, so call it on a blocking
    /// thread. `agent_version` is passed through, since only the caller knows it.
    fn info(&self, agent_version: &str) -> Result<SystemInfo, HostError>;
}

/// The backend for this OS.
pub fn backend() -> Arc<dyn System> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(crate::linux::LinuxSystem::default())
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(crate::macos::MacSystem)
    }
    #[cfg(windows)]
    {
        Arc::new(crate::windows::WinSystem)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Arc::new(crate::Unsupported)
    }
}

impl System for crate::Unsupported {
    fn info(&self, _agent_version: &str) -> Result<SystemInfo, HostError> {
        Err(HostError::Unsupported)
    }
}
