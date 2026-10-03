//! Enrollment, as the running agent does it when `cntrl enroll` hands it a
//! token: a new device key, privd's audit key, one signed `POST /v1/enroll`, and
//! on success the identity saved beside the key. A machine that's enrolled
//! already also proves which device it is now, signed with that device's key,
//! so Console can retire it as the new one is created (D23). A failure leaves
//! the earlier identity untouched.

use std::fmt;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cntrl_protocol::enroll::{
    self, EnrollError, EnrollRequest, EnrollResponse, EnrollToken, PreviousDevice, PublicKey,
};
use cntrl_protocol::frame::SigAlg;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::config::Config;
use super::host;
use super::identity::{self, DEVICE_KEY_FILE, Identity};
use super::ipc::{self, Call};
use super::keys::SigningKey;

/// What `cntrl enroll` sends the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollCommand {
    pub token: String,
    /// Replace the current enrollment, moving the machine if the token is for
    /// another organization, without Console asking first.
    #[serde(default)]
    pub force: bool,
}

/// What the agent answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollOutcome {
    pub device_id: String,
    pub fingerprint: String,
    pub gateway_url: String,
    /// The device this enrollment replaced, now gone from its organization.
    #[serde(default)]
    pub replaced: Option<String>,
}

/// Why an enrollment didn't happen.
#[derive(Debug)]
pub enum EnrollFailure {
    /// Console said no. `confirm_move` and `already_enrolled` leave the token
    /// unused, for the caller to ask the user or report there's nothing to do.
    Refused(EnrollError),
    Failed(String),
}

impl From<String> for EnrollFailure {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl fmt::Display for EnrollFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(error) => write!(f, "Console refused the enrollment: {}", error.msg),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

pub async fn enroll(
    config: &Config,
    command: EnrollCommand,
) -> Result<EnrollOutcome, EnrollFailure> {
    let state_dir = &config.paths.state_dir;
    let existing = identity::load(state_dir)?;
    let token_text = command.token.trim();
    let token = EnrollToken::parse(token_text).map_err(|e| e.to_string())?;

    let new_key_path = state_dir.join(format!("{DEVICE_KEY_FILE}.new"));
    let device_key = SigningKey::generate(&new_key_path)?;
    let audit = ipc::call_once(&config.paths.privd_socket, Call::AuditKey).await?;
    let audit_key = audit["key"]
        .as_str()
        .ok_or_else(|| "privd sent no audit key".to_owned())?
        .to_owned();

    let host = host::host_info();
    let device_public = device_key.public_key();
    let previous = existing
        .as_ref()
        .and_then(|existing| previous_device(state_dir, existing, &token.id, &device_public));
    let signed = enroll::signing_string(&token.id, &device_public, &audit_key, &host);
    let request = EnrollRequest {
        token: token_text.to_owned(),
        device_key: PublicKey {
            alg: SigAlg::Es256,
            key: device_public,
        },
        audit_key: PublicKey {
            alg: SigAlg::Es256,
            key: audit_key,
        },
        host,
        pop: device_key.sign(signed.as_bytes())?,
        previous,
        replace: command.force,
    };
    let response = match post_enroll(&config.console.url, &request).await {
        Ok(response) => response,
        Err(e) => {
            let _ = fs::remove_file(&new_key_path);
            return Err(e);
        }
    };

    fs::rename(&new_key_path, state_dir.join(DEVICE_KEY_FILE))
        .map_err(|e| format!("can't install the device key: {e}"))?;
    let identity = Identity {
        device_id: response.device_id,
        key_id: response.key_id,
        audit_key_id: response.audit_key_id,
        generation: response.generation,
        gateway_url: response.gateway_url,
        console_url: config.console.url.clone(),
        fingerprint: device_key.fingerprint(),
        enrolled_at_ms: now_ms(),
        credential: Some(response.credential),
    };
    identity::save(state_dir, &identity)?;

    let record = Call::AuditAppend {
        kind: "agent.enrolled".to_owned(),
        data: json!({
            "device_id": identity.device_id,
            "fingerprint": identity.fingerprint,
            "replaced": response.replaced,
        }),
    };
    if let Err(e) = ipc::call_once(&config.paths.privd_socket, record).await {
        tracing::warn!("couldn't record the enrollment in the audit log: {e}");
    }
    tracing::info!(device_id = identity.device_id, "enrolled");
    Ok(EnrollOutcome {
        device_id: identity.device_id,
        fingerprint: identity.fingerprint,
        gateway_url: identity.gateway_url,
        replaced: response.replaced,
    })
}

/// Proof of the device this machine is enrolled as now, signed with its key.
/// Without it Console keeps that device, so a missing key is only a warning.
fn previous_device(
    state_dir: &Path,
    existing: &Identity,
    token_id: &str,
    new_key: &str,
) -> Option<PreviousDevice> {
    let signed = enroll::replace_signing_string(token_id, &existing.device_id, new_key);
    let proof = SigningKey::load(&state_dir.join(DEVICE_KEY_FILE))
        .and_then(|key| key.sign(signed.as_bytes()));
    match proof {
        Ok(sig) => Some(PreviousDevice {
            device_id: existing.device_id.clone(),
            sig,
        }),
        Err(e) => {
            tracing::warn!("can't sign as the current device, so Console keeps it: {e}");
            None
        }
    }
}

async fn post_enroll(
    console_url: &str,
    request: &EnrollRequest,
) -> Result<EnrollResponse, EnrollFailure> {
    let url = format!("{}/v1/enroll", console_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("cntrl-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| EnrollFailure::Failed(e.to_string()))?;
    let response = client
        .post(&url)
        .json(request)
        .send()
        .await
        .map_err(|e| EnrollFailure::Failed(format!("can't reach Console at {url}: {e}")))?;
    let status = response.status();
    if status.is_success() {
        return response
            .json::<EnrollResponse>()
            .await
            .map_err(|e| EnrollFailure::Failed(format!("unexpected answer from Console: {e}")));
    }
    match response.json::<EnrollError>().await {
        Ok(error) => Err(EnrollFailure::Refused(error)),
        Err(_) => Err(EnrollFailure::Failed(format!("Console answered {status}"))),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
