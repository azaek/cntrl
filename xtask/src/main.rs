//! Repo tasks. `cargo xtask codegen [--check]` generates the protocol's JSON
//! Schema and TypeScript types from `cntrl-protocol`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // The protocol has no operations yet, so there is nothing to generate.
        Some("codegen") => ExitCode::SUCCESS,
        _ => {
            eprintln!("usage: cargo xtask codegen [--check]");
            ExitCode::FAILURE
        }
    }
}
