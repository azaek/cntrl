//! Signed commands (D108) against the vector WebCrypto made, which Console's
//! tests read too: both sides must sign and hash exactly the same strings.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use cntrl_protocol::signers::{
    CommandSignature, PICTURE_HEIGHT, PICTURE_WIDTH, SignerEntry, canonical_json,
    command_signing_string, data_hash, entry_hash, entry_signing_string, fingerprint, key_id,
    name_is_clean, picture,
};
use serde_json::Value;

fn vector() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/protocol/v1/signers/vector.json");
    serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("parse")
}

#[test]
fn key_ids_and_fingerprints_match() {
    for key in vector()["keys"].as_array().expect("keys") {
        let public = key["public"].as_str().expect("public");
        assert_eq!(key_id(public).as_deref(), key["id"].as_str());
        assert_eq!(fingerprint(public).as_deref(), key["fingerprint"].as_str());
    }
    assert_eq!(key_id("not base64url!"), None);
}

#[test]
fn entries_sign_and_hash_the_same_strings() {
    for case in vector()["entries"].as_array().expect("entries") {
        let entry: SignerEntry = serde_json::from_value(case["entry"].clone()).expect("entry");
        assert_eq!(
            entry_signing_string(&entry),
            case["signing_string"].as_str().expect("signing_string")
        );
        assert_eq!(entry_hash(&entry), case["hash"].as_str().expect("hash"));
        // Each names the hash of the one before.
        let round = serde_json::to_value(&entry).expect("to value");
        assert_eq!(round, case["entry"], "round trip");
    }
}

#[test]
fn a_command_signs_its_canonical_data() {
    let command = &vector()["command"];
    let data = &command["data"];
    assert_eq!(
        canonical_json(data),
        command["canonical_data"].as_str().expect("canonical_data")
    );
    assert_eq!(
        data_hash(data),
        command["data_hash"].as_str().expect("data_hash")
    );
    let signature: CommandSignature =
        serde_json::from_value(command["signature"].clone()).expect("signature");
    let text = |key: &str| command[key].as_str().expect("string field");
    assert_eq!(
        command_signing_string(
            text("org"),
            text("device_id"),
            text("request_id"),
            text("op"),
            data,
            &signature
        ),
        text("signing_string")
    );
}

#[test]
fn names_with_control_characters_are_refused() {
    assert!(name_is_clean("Ana, Firefox on Windows"));
    assert!(!name_is_clean("Ana\nTrusted: Alok"));
    assert!(!name_is_clean("Ana\u{1b}[2K"));
    assert!(!name_is_clean("   "));
    assert!(!name_is_clean(&"x".repeat(81)));
}

/// Console draws the same picture of a fingerprint (D108), on the same board.
#[test]
fn pictures_match_consoles() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/protocol/v1/signers/pictures.json");
    let fixture: Value =
        serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("parse");
    assert_eq!(fixture["board"]["width"], PICTURE_WIDTH);
    assert_eq!(fixture["board"]["height"], PICTURE_HEIGHT);
    for each in fixture["pictures"].as_array().expect("pictures") {
        let rows: Vec<String> = each["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row.as_str().expect("row").to_owned())
            .collect();
        assert_eq!(
            picture(each["fingerprint"].as_str().expect("fingerprint")),
            Some(rows)
        );
    }
    assert_eq!(picture("4b3b 5a12 d603"), None);
    assert_eq!(picture("zzzz 5a12 d603 5ff9"), None);
}
