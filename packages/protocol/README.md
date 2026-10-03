# @cntrl-pw/protocol

TypeScript types for the cntrl agent protocol (`cntrl.agent.v1.json`): frames, operations, topics, error and close codes.

Everything in `src/generated/` comes from the Rust crate `crates/cntrl-protocol`. Change the Rust types, then run `cargo xtask codegen` from the repo root; CI fails if the generated files are out of date.
