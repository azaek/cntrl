//! The cntrl agent: a headless, system-level server-management agent. It runs
//! on Linux, macOS and Windows; on other platforms the binary builds and
//! refuses to run.

#[cfg(any(unix, windows))]
mod agent;

use std::process::ExitCode;

fn main() -> ExitCode {
    run()
}

#[cfg(any(unix, windows))]
fn run() -> ExitCode {
    agent::main()
}

#[cfg(not(any(unix, windows)))]
fn run() -> ExitCode {
    eprintln!(
        "cntrl-agent {}: this platform isn't supported yet",
        env!("CARGO_PKG_VERSION")
    );
    ExitCode::FAILURE
}
