//! Repo tasks. `cargo xtask codegen` writes the protocol's JSON Schema, its
//! operations table and the TypeScript package's generated files from
//! `cntrl-protocol`. With `--check` it writes nothing and fails if any of them is
//! out of date.

mod ts;

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use cntrl_protocol::{Frame, OpSchema, TopicSchema, ops};
use schemars::generate::SchemaSettings;
use serde_json::{Value, json};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("codegen") => codegen(args.iter().any(|arg| arg == "--check")),
        _ => Err("usage: cargo xtask codegen [--check]".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn codegen(check: bool) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask has no parent directory")?;

    let mut generator = SchemaSettings::draft2020_12().into_generator();
    generator.subschema_for::<Frame>();
    let ops = ops::op_schemas(&mut generator);
    let topics = ops::topic_schemas(&mut generator);
    generator.subschema_for::<cntrl_protocol::enroll::EnrollRequest>();
    generator.subschema_for::<cntrl_protocol::enroll::EnrollResponse>();
    generator.subschema_for::<cntrl_protocol::enroll::EnrollError>();
    generator.subschema_for::<cntrl_protocol::records::StatsRecord>();
    generator.subschema_for::<cntrl_protocol::records::AuditCheckpoint>();
    generator.subschema_for::<cntrl_protocol::records::AlertRecord>();
    let defs = generator.definitions().clone();

    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://cntrl.pw/protocol/v1/schema.json",
        "title": "cntrl agent protocol v1",
        "$ref": "#/$defs/Frame",
        "$defs": &defs,
    });

    let files = [
        ("protocol/v1/schema.json", pretty(&schema)?),
        (
            "protocol/v1/operations.json",
            pretty(&operations(&ops, &topics))?,
        ),
        (
            "packages/protocol/src/generated/types.ts",
            ts::types(&defs)?,
        ),
        (
            "packages/protocol/src/generated/ops.ts",
            ts::ops(&ops, &topics)?,
        ),
        (
            "packages/protocol/src/generated/constants.ts",
            ts::constants(),
        ),
    ];

    let mut stale = Vec::new();
    for (relative, content) in files {
        let path = root.join(relative);
        if fs::read_to_string(&path).is_ok_and(|current| current == content) {
            continue;
        }
        if check {
            stale.push(relative);
            continue;
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{relative}: {e}"))?;
        }
        fs::write(&path, content).map_err(|e| format!("{relative}: {e}"))?;
        println!("wrote {relative}");
    }
    if stale.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "generated files are out of date; run `cargo xtask codegen`: {}",
            stale.join(", ")
        ))
    }
}

/// The operations table: names, capabilities, revisions and schema references.
fn operations(ops: &[OpSchema], topics: &[TopicSchema]) -> Value {
    json!({
        "protocol_version": cntrl_protocol::PROTOCOL_VERSION,
        "subprotocol": cntrl_protocol::SUBPROTOCOL,
        "ops": ops.iter().map(|op| json!({
            "name": op.info.name,
            "capability": op.info.capability,
            "since": op.info.since,
            "params": op.params,
            "result": op.result,
        })).collect::<Vec<_>>(),
        "topics": topics.iter().map(|topic| json!({
            "name": topic.info.name,
            "capability": topic.info.capability,
            "since": topic.info.since,
            "params": topic.params,
            "event": topic.event,
        })).collect::<Vec<_>>(),
    })
}

fn pretty(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|text| text + "\n")
        .map_err(|e| e.to_string())
}
