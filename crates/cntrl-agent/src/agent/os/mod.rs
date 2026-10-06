//! What the agent asks of its operating system, once for Unix and once for
//! Windows (D58): files only the service's account can read, and a policy file
//! nobody but its owner can change. The rest of the agent stays the same on
//! every system.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
