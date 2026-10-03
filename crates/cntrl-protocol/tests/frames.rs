//! Golden frames: every file in testdata/protocol/v1/frames parses, isn't
//! mistaken for an unknown frame, and serializes back to the same JSON.

#![allow(clippy::panic)]

use std::fs;
use std::path::PathBuf;

use cntrl_protocol::frame::RecordKind;
use cntrl_protocol::ops::{Call, OPS, TOPICS, Topic};
use cntrl_protocol::process::ProcessesSample;
use cntrl_protocol::records::{AuditCheckpoint, StatsRecord};
use cntrl_protocol::stats::{SensorKind, StatsSample};
use cntrl_protocol::system::SystemInfo;
use cntrl_protocol::{ErrorCode, Frame};
use serde_json::{Value, json};

fn frames_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/protocol/v1/frames")
}

#[test]
fn golden_frames_round_trip() {
    let mut count = 0;
    for entry in fs::read_dir(frames_dir()).expect("read the frames directory") {
        let path = entry.expect("directory entry").path();
        let name = path.display().to_string();
        let text = fs::read_to_string(&path).expect("read the frame");
        let original: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let frame: Frame =
            serde_json::from_value(original.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_ne!(frame, Frame::Unknown, "{name} parsed as an unknown frame");
        let back = serde_json::to_value(&frame).expect("serialize");
        assert_eq!(back, original, "{name} doesn't round-trip");
        match &frame {
            Frame::Req(req) => {
                Call::decode(&req.op, req.data.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            Frame::Sub(sub) => {
                Topic::decode(&sub.topic, sub.data.clone())
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            Frame::Rec(rec) => {
                for record in &rec.recs {
                    let data = record.data.clone();
                    let decoded = match record.kind {
                        RecordKind::Metrics => {
                            serde_json::from_value::<StatsRecord>(data).map(drop)
                        }
                        RecordKind::AuditCheckpoint => {
                            serde_json::from_value::<AuditCheckpoint>(data).map(drop)
                        }
                        RecordKind::Unknown => panic!("{name}: a record of unknown type"),
                    };
                    decoded.unwrap_or_else(|e| panic!("{name}: record {}: {e}", record.seq));
                }
            }
            _ => {}
        }
        count += 1;
    }
    assert!(
        count >= 13,
        "expected a golden file for every frame type, found {count}"
    );
}

#[test]
fn unknown_frame_type_is_ignored() {
    let frame: Frame =
        serde_json::from_value(json!({"t": "from_a_newer_revision", "x": 1})).expect("parses");
    assert_eq!(frame, Frame::Unknown);
}

#[test]
fn unknown_error_code_decodes_as_unknown() {
    let code: ErrorCode = serde_json::from_value(json!("quota_exceeded")).expect("parses");
    assert_eq!(code, ErrorCode::Unknown);
}

#[test]
fn absent_data_decodes_as_no_params() {
    assert!(matches!(
        Call::decode("system.info", Value::Null),
        Ok(Call::SystemInfo(_))
    ));
}

#[test]
fn decode_errors_map_to_error_codes() {
    let unknown = Call::decode("system.erase_everything", Value::Null).expect_err("unknown op");
    assert_eq!(unknown.code(), ErrorCode::UnknownOp);
    let bad = Call::decode("service.restart", json!({"unit": 7})).expect_err("bad params");
    assert_eq!(bad.code(), ErrorCode::BadRequest);
}

#[test]
fn registry_entries_are_unique_and_namespaced() {
    for table in [OPS, TOPICS] {
        let mut names: Vec<&str> = table.iter().map(|entry| entry.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), table.len(), "duplicate name in the registry");
        assert!(
            table.iter().all(|entry| entry.capability.contains('.')),
            "capability without a namespace"
        );
    }
}

#[test]
fn every_capability_named_by_the_registry_exists() {
    for entry in OPS.iter().chain(TOPICS) {
        assert!(
            cntrl_protocol::capability::is_capability(entry.capability),
            "{} names an unknown capability {}",
            entry.name,
            entry.capability
        );
    }
    for capability in cntrl_protocol::MONITOR_ONLY {
        assert!(
            cntrl_protocol::capability::is_capability(capability),
            "{capability}"
        );
    }
}

#[test]
fn the_processes_event_decodes() {
    let text = fs::read_to_string(frames_dir().join("evt-processes.json")).expect("read the frame");
    let Frame::Evt(event) = serde_json::from_str(&text).expect("a frame") else {
        panic!("evt-processes.json isn't an event");
    };
    let sample: ProcessesSample = serde_json::from_value(event.data).expect("a processes sample");
    assert_eq!(sample.processes.len(), 2);
    assert_eq!(sample.processes[0].unit.as_deref(), Some("nginx.service"));
    assert!(!sample.processes[0].kernel);
}

#[test]
fn stats_events_decode_old_and_new() {
    let sample = |file: &str| -> StatsSample {
        let text = fs::read_to_string(frames_dir().join(file)).expect("read the frame");
        let Frame::Evt(event) = serde_json::from_str(&text).expect("a frame") else {
            panic!("{file} isn't an event");
        };
        serde_json::from_value(event.data).expect("a stats sample")
    };
    // A 0.1.3 agent's sample has none of the newer fields.
    let old = sample("evt.json");
    assert!(old.swap.is_none() && old.disk_io.is_none() && old.network.is_none());
    assert!(old.filesystems.is_empty() && old.temperatures.is_empty() && old.gpus.is_empty());
    let new = sample("evt-stats-full.json");
    assert_eq!(new.disk_io.map(|io| io.write), Some(1_048_576));
    assert_eq!(new.filesystems[1].name.as_deref(), Some("Backup"));
    assert_eq!(new.temperatures[0].sensor, SensorKind::Cpu);
    assert_eq!(new.gpus[1].memory_total, None);
    // A sensor kind from a newer agent reads as other.
    let kind: SensorKind = serde_json::from_value(json!("battery")).expect("parses");
    assert_eq!(kind, SensorKind::Other);
}

#[test]
fn system_info_decodes_with_and_without_hardware() {
    let text = fs::read_to_string(frames_dir().join("res-system-info.json")).expect("read");
    let Frame::Res(res) = serde_json::from_str(&text).expect("a frame") else {
        panic!("res-system-info.json isn't a response");
    };
    let info: SystemInfo = serde_json::from_value(res.data.expect("data")).expect("system info");
    let cpu = info.cpu.expect("cpu");
    assert_eq!(
        (cpu.performance_cores, cpu.efficiency_cores),
        (Some(4), Some(6))
    );
    let older: SystemInfo = serde_json::from_value(json!({
        "hostname": "web-01", "os": {"id": "debian", "name": "Debian"}, "arch": "x86_64",
        "kernel": "6.1.0", "boot_time": 1, "agent_version": "0.1.3"
    }))
    .expect("a 0.1.3 agent's system info");
    assert!(older.machine.is_none() && older.cpu.is_none());
}
