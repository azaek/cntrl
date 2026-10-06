//! ES256 signing keys, kept as PKCS#8 files readable only by their owner: the
//! device key belongs to the agent's user, the audit key to privd.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use sha2::{Digest, Sha256};

use super::os::Private;

pub struct SigningKey {
    pair: EcdsaKeyPair,
    rng: SystemRandom,
}

impl SigningKey {
    /// Generates a key and writes it to `path`, replacing any older one.
    pub fn generate(path: &Path) -> Result<Self, String> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| "can't generate a key".to_owned())?;
        write_private(path, pkcs8.as_ref())?;
        Self::from_pkcs8(pkcs8.as_ref(), rng)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
        Self::from_pkcs8(&bytes, SystemRandom::new())
    }

    /// Loads the key at `path`, generating it the first time.
    pub fn load_or_generate(path: &Path) -> Result<Self, String> {
        if path.exists() {
            Self::load(path)
        } else {
            Self::generate(path)
        }
    }

    fn from_pkcs8(bytes: &[u8], rng: SystemRandom) -> Result<Self, String> {
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, bytes, &rng)
            .map_err(|e| format!("invalid key file: {e}"))?;
        Ok(Self { pair, rng })
    }

    /// The uncompressed public point, base64url without padding.
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.pair.public_key().as_ref())
    }

    /// `SHA256:` and the unpadded base64 of the public point's SHA-256, the way
    /// OpenSSH prints fingerprints.
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(self.pair.public_key().as_ref());
        format!("SHA256:{}", STANDARD_NO_PAD.encode(digest))
    }

    /// Signs `message` with ES256 (`r || s`), base64url without padding.
    pub fn sign(&self, message: &[u8]) -> Result<String, String> {
        let signature = self
            .pair
            .sign(&self.rng, message)
            .map_err(|_| "signing failed".to_owned())?;
        Ok(URL_SAFE_NO_PAD.encode(signature.as_ref()))
    }
}

/// Writes `bytes` to `path`, readable by its owner only, through a temporary
/// file and a rename.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .private()
        .open(&tmp)
        .map_err(|e| format!("can't write {}: {e}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("can't write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("can't replace {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    use super::*;

    #[test]
    fn a_generated_key_reloads_and_signs_verifiably() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("device.key");
        let key = SigningKey::generate(&path).expect("generate");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let reloaded = SigningKey::load_or_generate(&path).expect("reload");
        assert_eq!(reloaded.public_key(), key.public_key());
        assert!(key.fingerprint().starts_with("SHA256:"));

        let signature = URL_SAFE_NO_PAD
            .decode(key.sign(b"hello").expect("sign"))
            .expect("base64");
        let public = URL_SAFE_NO_PAD.decode(key.public_key()).expect("base64");
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
            .verify(b"hello", &signature)
            .expect("signature verifies");
    }
}
