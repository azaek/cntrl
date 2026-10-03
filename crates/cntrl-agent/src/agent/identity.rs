//! The device identity: `identity.json` in the agent's state directory, written
//! at enrollment, with the device key beside it in `device.key`.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::keys::write_private;

pub const IDENTITY_FILE: &str = "identity.json";
pub const DEVICE_KEY_FILE: &str = "device.key";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub device_id: String,
    pub key_id: String,
    pub audit_key_id: String,
    pub generation: u64,
    pub gateway_url: String,
    pub console_url: String,
    pub fingerprint: String,
    pub enrolled_at_ms: u64,
    /// The gateway's connection credential, renewed on connect. Opaque here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// The saved identity, or `None` before enrollment.
pub fn load(state_dir: &Path) -> Result<Option<Identity>, String> {
    let path = state_dir.join(IDENTITY_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("can't read {}: {e}", path.display())),
    }
}

pub fn save(state_dir: &Path, identity: &Identity) -> Result<(), String> {
    let text = serde_json::to_string_pretty(identity).map_err(|e| e.to_string())?;
    write_private(
        &state_dir.join(IDENTITY_FILE),
        format!("{text}\n").as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_round_trips() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(load(dir.path()).expect("load"), None);
        let identity = Identity {
            device_id: "dev_01".into(),
            key_id: "key_01".into(),
            audit_key_id: "key_02".into(),
            generation: 1,
            gateway_url: "ws://localhost:8787/v1/agent".into(),
            console_url: "http://localhost:8787".into(),
            fingerprint: "SHA256:abc".into(),
            enrolled_at_ms: 1,
            credential: Some("v1.e30.c2ln".into()),
        };
        save(dir.path(), &identity).expect("save");
        assert_eq!(load(dir.path()).expect("load"), Some(identity));
    }
}
