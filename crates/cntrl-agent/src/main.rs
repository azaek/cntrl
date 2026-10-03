//! The cntrl agent: a headless, system-level server-management agent. It runs
//! on Linux and macOS; on other platforms the binary builds and refuses to run.

#[cfg(unix)]
mod agent;

use std::process::ExitCode;

fn main() -> ExitCode {
    run()
}

#[cfg(unix)]
fn run() -> ExitCode {
    agent::main()
}

#[cfg(not(unix))]
fn run() -> ExitCode {
    eprintln!(
        "cntrl-agent {}: this platform isn't supported yet",
        env!("CARGO_PKG_VERSION")
    );
    ExitCode::FAILURE
}
