//! What the agent asks of its operating system, once for Unix and once for
//! Windows (D58): files only the service's account can read, a policy file
//! nobody but its owner can change, accounts, and the local endpoints privd and
//! the CLI are reached at. The rest of the agent stays the same on every
//! system.

use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

/// Which local endpoint a listener serves, which decides who may connect.
#[derive(Debug, Clone, Copy)]
pub enum Endpoint {
    /// privd's, for the agent.
    Privd,
    /// The agent's, for the `cntrl` CLI.
    Agent,
}

impl Connected<IncomingStream<'_, LocalListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, LocalListener>) -> Self {
        stream.remote_addr().clone()
    }
}
