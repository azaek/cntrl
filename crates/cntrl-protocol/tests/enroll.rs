//! Enrollment against the golden vector that Console's TypeScript tests read too.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use cntrl_protocol::enroll::{
    EnrollRequest, EnrollToken, HostInfo, TokenError, replace_signing_string, signing_string,
};
use serde_json::Value;

fn vector() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/protocol/v1/enroll/signing.json");
    serde_json::from_str(&fs::read_to_string(path).expect("read the vector"))
        .expect("parse the vector")
}

fn text(vector: &Value, key: &str) -> String {
    vector[key].as_str().expect("string field").to_owned()
}

#[test]
fn the_vector_token_parses_and_formats_back() {
    let vector = vector();
    let token = EnrollToken::parse(&text(&vector, "token")).expect("parses");
    assert_eq!(token.id, text(&vector, "token_id"));
    assert_eq!(token.secret, text(&vector, "token_secret"));
    assert_eq!(token.format(), text(&vector, "token"));
}

#[test]
fn a_bad_checksum_or_shape_is_refused() {
    let vector = vector();
    assert_eq!(
        EnrollToken::parse(&text(&vector, "bad_checksum_token")),
        Err(TokenError::Checksum)
    );
    for bad in [
        "",
        "cntrl_et_",
        "cntrl_pat_k3x9m2q7v4ab_00",
        &text(&vector, "token").to_uppercase(),
    ] {
        assert_eq!(EnrollToken::parse(bad), Err(TokenError::Format), "{bad}");
    }
}

#[test]
fn the_signing_string_matches_the_vector() {
    let vector = vector();
    let host: HostInfo = serde_json::from_value(vector["host"].clone()).expect("host");
    let signed = signing_string(
        &text(&vector, "token_id"),
        &text(&vector, "device_key"),
        &text(&vector, "audit_key"),
        &host,
    );
    assert_eq!(signed, text(&vector, "signing_string"));
}

#[test]
fn the_replace_signing_string_matches_the_vector() {
    let vector = vector();
    let signed = replace_signing_string(
        &text(&vector, "token_id"),
        &text(&vector, "previous_device_id"),
        &text(&vector, "device_key"),
    );
    assert_eq!(signed, text(&vector, "replace_signing_string"));
}

#[test]
fn a_request_from_an_older_agent_still_parses_and_round_trips() {
    let vector = vector();
    let request: EnrollRequest = serde_json::from_value(serde_json::json!({
        "token": text(&vector, "token"),
        "device_key": { "alg": "ES256", "key": text(&vector, "device_key") },
        "audit_key": { "alg": "ES256", "key": text(&vector, "audit_key") },
        "host": vector["host"].clone(),
        "pop": "c2ln",
    }))
    .expect("parses");
    assert_eq!(request.previous, None);
    assert!(!request.replace);
    let back = serde_json::to_value(&request).expect("serializes");
    assert!(back.get("previous").is_none());
    assert!(back.get("replace").is_none());
}
