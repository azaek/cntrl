//! The hello signing string against the golden vector the gateway's tests read too.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use cntrl_protocol::auth::{gateway_host, hello_signing_string};
use serde_json::Value;

fn vector() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/protocol/v1/hello/signing.json");
    serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("parse")
}

#[test]
fn the_hello_signing_string_matches_the_vector() {
    let vector = vector();
    let text = |key: &str| vector[key].as_str().expect("string field").to_owned();
    let signed = hello_signing_string(
        &text("sid"),
        &text("nonce"),
        &text("gateway_host"),
        &text("device_id"),
        &text("key_id"),
        vector["generation"].as_u64().expect("generation"),
    );
    assert_eq!(signed, text("signing_string"));
}

#[test]
fn gateway_hosts_match_the_vector() {
    let vector = vector();
    let cases = vector["gateway_hosts"].as_array().expect("gateway_hosts");
    for case in cases {
        let url = case["url"].as_str().expect("url");
        let host = case["host"].as_str().expect("host");
        assert_eq!(gateway_host(url).as_deref(), Some(host), "{url}");
    }
}

#[test]
fn gateway_host_refuses_what_isnt_a_gateway_url() {
    for url in [
        "",
        "gw.cntrl.pw",
        "ftp://gw.cntrl.pw",
        "wss://",
        "wss://gw.cntrl.pw:99999",
    ] {
        assert_eq!(gateway_host(url), None, "{url}");
    }
}
