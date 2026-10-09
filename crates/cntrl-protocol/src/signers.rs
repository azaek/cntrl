//! Signed commands (D108): an organization's signers, and what they sign.
//!
//! The people who act on machines sign each operation that changes one, in
//! their browser, with a P-256 key that never leaves it. A machine that
//! requires signatures (`cntrl policy require-signatures`) acts on such an
//! operation only when a key its organization's signer log trusts signed it.
//!
//! The signer log is append-only, as Tailnet Lock's is. Its first entry adds
//! the owner's key and is signed by that key; each later entry adds or removes
//! a key, names the hash of the entry before it, and is signed by a key the
//! log trusted until then. Console stores the log and the gateway relays it,
//! but every machine checks each entry itself, so neither can add a key. A
//! machine trusts the log it first pins, then only entries that follow on
//! from it.
//!
//! Signatures are ECDSA P-256 with SHA-256 over the signing strings below, as
//! WebCrypto makes them: r and s, 32 bytes each, base64url without padding.
//! Keys are uncompressed points (65 bytes), base64url. Strings, not JSON, are
//! signed, so the browser and the agent never have to agree on how JSON is
//! written; an operation's data is signed by the SHA-256 of its canonical JSON.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The feature an agent reports in its hello when it checks signatures; Console
/// signs for that agent and the gateway sends it the signer log.
pub const FEATURE: &str = "signed_commands";

/// How far a signed command's time may be from the machine's clock, either
/// way, in milliseconds. Generous, since boards without a real-time clock
/// drift, and a replay is refused by its request ID anyway.
pub const WINDOW_MS: u64 = 5 * 60 * 1000;

/// The capabilities whose operations change the machine, and so need a
/// signature where the machine requires them. Reads never do. `checks.run`
/// isn't an operation: its checks come in a frame, unsigned.
pub const SIGNED_CAPABILITIES: &[&str] = &[
    "services.manage",
    "containers.manage",
    "processes.signal",
    "power.reboot",
    "power.poweroff",
    "power.suspend",
    "power.hibernate",
    "history.manage",
];

/// Whether an operation needing `capability` must be signed.
pub fn needs_signature(capability: &str) -> bool {
    SIGNED_CAPABILITIES.contains(&capability)
}

/// The longest name a signer's key may have.
pub const NAME_MAX: usize = 80;

/// The organization's signer log, the whole of it, oldest first: what the
/// gateway sends an agent that checks signatures, after its hello and when
/// the log grows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SignerLog {
    pub entries: Vec<SignerEntry>,
}

/// One entry of the signer log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SignerEntry {
    /// The organization's ID.
    pub org: String,
    /// Its place in the log, from 0.
    pub seq: u64,
    /// The hash of the entry before ([`entry_hash`]); none for the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    pub change: SignerChange,
    /// When it was signed, in Unix milliseconds.
    pub at: u64,
    /// The ID of the key that signed it.
    pub by: String,
    /// The signature over [`entry_signing_string`].
    pub sig: String,
}

/// What an entry changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignerChange {
    /// A key joins the signers.
    Add { key: SignerKey },
    /// A key stops being trusted.
    Remove { key_id: String },
}

/// A signer's public key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SignerKey {
    /// [`key_id`] of `public`.
    pub id: String,
    /// The uncompressed P-256 point, base64url.
    pub public: String,
    /// Whose it is and where, such as "Ana, Chrome on Windows": shown, never trusted.
    pub name: String,
}

/// A command's signature, on the `req` frame of an operation that needs one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CommandSignature {
    /// The signing key's ID.
    pub key: String,
    /// When it was signed, in Unix milliseconds.
    pub at: u64,
    /// The signature over [`command_signing_string`].
    pub sig: String,
}

/// Where a machine stands on signatures, in its hello's policy summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SignatureState {
    /// Whether it acts on changing operations only when they're signed.
    pub required: bool,
    /// The last entry of the signer log it trusts, by place and hash; none
    /// before it has pinned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Why it can't check signatures, when its trusted log is missing or
    /// unreadable; it then refuses every changing operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// SHA-256, lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A key's ID: `sk_` and the first 10 bytes (80 bits) of the SHA-256 of its
/// uncompressed point, lowercase hex. `None` when `public` isn't base64url.
pub fn key_id(public: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(public).ok()?;
    Some(format!("sk_{}", &sha256_hex(&bytes)[..20]))
}

/// What people compare to know a key is the one they mean: the first 8 bytes
/// (64 bits) of the SHA-256 of its point, as four groups of four hex digits,
/// such as `7f3a 9c21 0b4e d8f7`. 64 bits, so nobody can make a key to match
/// one. `None` when `public` isn't base64url.
pub fn fingerprint(public: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(public).ok()?;
    let hex = sha256_hex(&bytes);
    Some(format!(
        "{} {} {} {}",
        &hex[0..4],
        &hex[4..8],
        &hex[8..12],
        &hex[12..16]
    ))
}

/// The string a signer's key signs for an entry.
pub fn entry_signing_string(entry: &SignerEntry) -> String {
    let (kind, key_id, public, name) = match &entry.change {
        SignerChange::Add { key } => (
            "add",
            key.id.as_str(),
            key.public.as_str(),
            key.name.as_str(),
        ),
        SignerChange::Remove { key_id } => ("remove", key_id.as_str(), "-", "-"),
    };
    format!(
        "cntrl-signers-v1\n{}\n{}\n{}\n{kind}\n{key_id}\n{public}\n{name}\n{}\n{}",
        entry.org,
        entry.seq,
        entry.prev.as_deref().unwrap_or("-"),
        entry.at,
        entry.by
    )
}

/// An entry's hash, which the next entry names: the SHA-256 of its signing
/// string and its signature, so the log commits to both.
pub fn entry_hash(entry: &SignerEntry) -> String {
    sha256_hex(format!("{}\n{}", entry_signing_string(entry), entry.sig).as_bytes())
}

/// An operation's data as both sides write it to sign: JSON with no spaces and
/// each object's keys in order, compared as UTF-16 code units, the order
/// JavaScript's `sort()` gives. Sorted here, not by `Value`: with serde_json's
/// `preserve_order`, which another crate in a build can turn on, its objects
/// keep the order they arrived in.
pub fn canonical_json(data: &Value) -> String {
    let mut out = String::new();
    write_canonical(data, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The SHA-256 of an operation's canonical data, lowercase hex.
pub fn data_hash(data: &Value) -> String {
    sha256_hex(canonical_json(data).as_bytes())
}

/// The string a signer's key signs for a command: the organization, the
/// device, the request (whose ID the browser picks, so it can't be reused),
/// the operation and its data, when, and which key.
pub fn command_signing_string(
    org: &str,
    device_id: &str,
    request_id: &str,
    op: &str,
    data: &Value,
    signature: &CommandSignature,
) -> String {
    format!(
        "cntrl-command-v1\n{org}\n{device_id}\n{request_id}\n{op}\n{}\n{}\n{}",
        data_hash(data),
        signature.at,
        signature.key
    )
}

/// Whether a name is fit to show: not empty, at most [`NAME_MAX`] characters,
/// and no control characters, which could hide a key behind a misleading
/// line in a terminal.
pub fn name_is_clean(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= NAME_MAX
        && !name.chars().any(char::is_control)
}
