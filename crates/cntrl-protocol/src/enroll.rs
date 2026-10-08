//! Enrollment: the agent's one HTTPS call that turns a single-use token into a
//! device identity. `POST /v1/enroll` takes an [`EnrollRequest`] and answers with
//! an [`EnrollResponse`], or with an [`EnrollError`] and a 4xx or 5xx status.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::frame::SigAlg;

/// The prefix of every enrollment token.
pub const TOKEN_PREFIX: &str = "cntrl_et_";

/// A parsed enrollment token, `cntrl_et_<id>_<secret><check>`: `id` is 12
/// lowercase letters and digits, `secret` is 64 lowercase hex digits, and
/// `check` is the CRC-32 of everything before it as 8 lowercase hex digits.
/// Console stores only a hash of the secret; the checksum lets a mistyped token
/// fail before any network call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollToken {
    pub id: String,
    pub secret: String,
}

impl EnrollToken {
    pub fn parse(text: &str) -> Result<Self, TokenError> {
        let rest = text.strip_prefix(TOKEN_PREFIX).ok_or(TokenError::Format)?;
        let (id, tail) = rest.split_once('_').ok_or(TokenError::Format)?;
        let lower_alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        let lower_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
        if id.len() != 12
            || !id.bytes().all(lower_alnum)
            || tail.len() != 72
            || !tail.bytes().all(lower_hex)
        {
            return Err(TokenError::Format);
        }
        let (secret, check) = tail.split_at(64);
        let body = text.get(..text.len() - 8).ok_or(TokenError::Format)?;
        if check != checksum(body) {
            return Err(TokenError::Checksum);
        }
        Ok(Self {
            id: id.to_owned(),
            secret: secret.to_owned(),
        })
    }

    /// The token as text, checksum included.
    pub fn format(&self) -> String {
        let body = format!("{TOKEN_PREFIX}{}_{}", self.id, self.secret);
        let check = checksum(&body);
        format!("{body}{check}")
    }
}

fn checksum(body: &str) -> String {
    format!("{:08x}", crc32fast::hash(body.as_bytes()))
}

/// Why a token didn't parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    Format,
    Checksum,
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format => f.write_str("that isn't an enrollment token"),
            Self::Checksum => {
                f.write_str("the token's checksum doesn't match; check it for a typo")
            }
        }
    }
}

impl std::error::Error for TokenError {}

/// `POST /v1/enroll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EnrollRequest {
    pub token: String,
    pub device_key: PublicKey,
    pub audit_key: PublicKey,
    pub host: HostInfo,
    /// The device key's signature over [`signing_string`], base64url without
    /// padding.
    pub pop: String,
    /// The device this machine is enrolled as now, if any, so the gateway can
    /// retire it when the new one is created (D23).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<PreviousDevice>,
    /// Go ahead when `previous` is in another organization, or replace it in
    /// the same one. Without it the gateway asks first: `confirm_move` or
    /// `already_enrolled`, leaving the token unused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}

/// The device a machine is enrolled as, proven with that device's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PreviousDevice {
    pub device_id: String,
    /// The previous device key's signature over [`replace_signing_string`],
    /// base64url without padding.
    pub sig: String,
}

/// A public key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PublicKey {
    pub alg: SigAlg,
    /// The uncompressed P-256 point, base64url without padding.
    pub key: String,
}

/// The machine being enrolled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HostInfo {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    /// SHA-256 of the machine ID, never the raw ID.
    pub machine_id_hash: String,
    pub agent_version: String,
}

/// What a successful enrollment returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EnrollResponse {
    pub device_id: String,
    /// ID of the device key, carried in every hello.
    pub key_id: String,
    pub audit_key_id: String,
    /// Key generation counter; starts at 1.
    pub generation: u64,
    /// Where the agent connects.
    pub gateway_url: String,
    /// The connection credential: a ticket the gateway checks before a
    /// connection reaches its hub, sent as `Authorization: Bearer`. Opaque to the
    /// agent; the gateway renews it in a `welcome`.
    pub credential: String,
    /// The device this enrollment replaced, now removed from its organization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<String>,
}

/// Why an enrollment failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EnrollError {
    pub code: EnrollErrorCode,
    pub msg: String,
    /// For `confirm_move`: the organization the machine is in now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// For `confirm_move` and `already_enrolled`: the token's organization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// Machine-readable reason in an [`EnrollError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EnrollErrorCode {
    BadRequest,
    InvalidToken,
    TokenExpired,
    TokenUsed,
    BadSignature,
    /// The machine is already in the token's organization; nothing changed.
    AlreadyEnrolled,
    /// The machine is in another organization; enroll again with `replace`
    /// to move it.
    ConfirmMove,
    /// The organization's plan has no room for another device (D86); the
    /// message says what to do. Agents before 0.1.19 show the message.
    PlanLimit,
    Internal,
    #[serde(other)]
    Unknown,
}

/// The string the device key signs at enrollment. It binds both keys and the
/// machine to the token.
pub fn signing_string(
    token_id: &str,
    device_key: &str,
    audit_key: &str,
    host: &HostInfo,
) -> String {
    format!(
        "cntrl-enroll-v1\n{token_id}\n{device_key}\n{audit_key}\n{}\n{}",
        host.hostname, host.machine_id_hash
    )
}

/// The string the previous device key signs when its machine enrolls again
/// (D23). It binds the old device to this token and the new key, so the proof
/// works for nothing else.
pub fn replace_signing_string(
    token_id: &str,
    previous_device_id: &str,
    device_key: &str,
) -> String {
    format!("cntrl-replace-v1\n{token_id}\n{previous_device_id}\n{device_key}")
}
