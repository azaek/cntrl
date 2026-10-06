//! The service manager. systemd hears through sd_notify that the agent is
//! ready, feeds its watchdog while the health check passes, and sends SIGTERM
//! to stop it, as launchd does. Windows' Service Control Manager starts the
//! process through its dispatcher and asks it to stop through a control
//! (D58). Run by hand, the agent runs in the foreground until interrupted.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
