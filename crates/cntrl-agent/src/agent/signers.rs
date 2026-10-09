//! Signed commands on this machine (D108). Once `cntrl policy
//! require-signatures` pins its organization's signer log, the agent acts on
//! an operation that changes the machine only when a key that log trusts
//! signed it, for this device, recently, and not before. The pinned log is
//! `signers.json` in privd's state directory, which only root reaches: the
//! command pins it, and privd extends it with entries the gateway relays,
//! checking each one first. Not beside the policy: privd's sandbox can write
//! only its own directory. An entry that doesn't follow on from the pinned log (another
//! first entry, a gap, a fork or an older log) is refused, so Console can
//! neither add a key nor roll one back.
//!
//! Signatures are checked with ring's P-256 check for the fixed r‖s form,
//! which is what WebCrypto makes.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cntrl_protocol::frame::Request;
use cntrl_protocol::signers::{
    self as protocol, CommandSignature, SignatureState, SignerChange, SignerEntry, SignerLog,
};
use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
use serde::{Deserialize, Serialize};

use super::os::{self, Private};

/// The pinned log's file, in privd's state directory, such as
/// `/var/lib/cntrl-privd/signers.json`.
pub fn path_in(privd_state_dir: &Path) -> PathBuf {
    privd_state_dir.join("signers.json")
}

/// A key the pinned log trusts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signer {
    pub id: String,
    pub public: String,
    pub name: String,
    pub fingerprint: String,
}

/// A signer log checked from its first entry: the organization, its keys, and
/// its last entry by place and hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trust {
    pub org: String,
    pub entries: Vec<SignerEntry>,
    pub signers: BTreeMap<String, Signer>,
    pub head: String,
}

impl Trust {
    pub fn seq(&self) -> u64 {
        self.entries.len() as u64 - 1
    }

    /// Checks a whole log from its first entry, refusing anything that
    /// doesn't hold: the first must add a key and be signed by it, and each
    /// after it must name the hash of the one before and be signed by a key
    /// trusted until then.
    pub fn from_log(entries: &[SignerEntry]) -> Result<Self, String> {
        let first = entries.first().ok_or("the signer log is empty")?;
        let SignerChange::Add { key } = &first.change else {
            return Err("the signer log's first entry doesn't add a key".to_owned());
        };
        if first.seq != 0 || first.prev.is_some() || first.by != key.id {
            return Err("the signer log's first entry isn't signed by the key it adds".to_owned());
        }
        let mut trust = Trust {
            org: first.org.clone(),
            entries: Vec::new(),
            signers: BTreeMap::new(),
            head: String::new(),
        };
        let added = signer(key)?;
        verify(
            &added.public,
            &protocol::entry_signing_string(first),
            &first.sig,
        )
        .map_err(|()| "the signer log's first entry has a bad signature".to_owned())?;
        trust.signers.insert(added.id.clone(), added);
        trust.entries.push(first.clone());
        trust.head = protocol::entry_hash(first);
        for entry in &entries[1..] {
            trust.apply(entry)?;
        }
        Ok(trust)
    }

    /// Applies one entry that follows on from the log.
    fn apply(&mut self, entry: &SignerEntry) -> Result<(), String> {
        let at = entry.seq;
        if entry.org != self.org {
            return Err(format!("entry {at} is for another organization"));
        }
        if entry.seq != self.seq() + 1 || entry.prev.as_deref() != Some(self.head.as_str()) {
            return Err(format!("entry {at} doesn't follow on from the trusted log"));
        }
        let by = self
            .signers
            .get(&entry.by)
            .ok_or_else(|| format!("entry {at} is signed by a key the log doesn't trust"))?;
        verify(
            &by.public,
            &protocol::entry_signing_string(entry),
            &entry.sig,
        )
        .map_err(|()| format!("entry {at} has a bad signature"))?;
        match &entry.change {
            SignerChange::Add { key } => {
                if self.signers.contains_key(&key.id) {
                    return Err(format!("entry {at} adds a key the log trusts already"));
                }
                let added = signer(key)?;
                self.signers.insert(added.id.clone(), added);
            }
            SignerChange::Remove { key_id } => {
                if !self.signers.contains_key(key_id) {
                    return Err(format!("entry {at} removes a key the log doesn't trust"));
                }
                if self.signers.len() == 1 {
                    return Err(format!("entry {at} would leave no signers"));
                }
                self.signers.remove(key_id);
            }
        }
        self.entries.push(entry.clone());
        self.head = protocol::entry_hash(entry);
        Ok(())
    }

    /// The trust once `log` is applied: `log` must hold this one's entries
    /// unchanged, then any new ones after them. `None` when there's nothing
    /// new. An older log, or one that forks, is refused.
    pub fn extend(&self, log: &[SignerEntry]) -> Result<Option<Self>, String> {
        let known = self.entries.len();
        if log.len() < known {
            return Err(format!(
                "the gateway's signer log ends at entry {}, before the trusted one at {}",
                log.len().saturating_sub(1),
                self.seq()
            ));
        }
        for (ours, theirs) in self.entries.iter().zip(log) {
            if protocol::entry_hash(ours) != protocol::entry_hash(theirs) {
                return Err(format!(
                    "the gateway's signer log differs at entry {}",
                    ours.seq
                ));
            }
        }
        if log.len() == known {
            return Ok(None);
        }
        let mut next = self.clone();
        for entry in &log[known..] {
            next.apply(entry)?;
        }
        Ok(Some(next))
    }

    /// Checks a command's signature for this device: from a trusted key, over
    /// this organization, device, request, operation and data, within
    /// [`protocol::WINDOW_MS`] of `now_ms`. Replays are the caller's to refuse.
    pub fn check(
        &self,
        device_id: &str,
        request: &Request,
        now_ms: u64,
    ) -> Result<&Signer, Refusal> {
        let Some(signature) = &request.sig else {
            return Err(Refusal::Required(
                "this machine requires signed commands, and this one isn't signed".to_owned(),
            ));
        };
        let Some(signer) = self.signers.get(&signature.key) else {
            return Err(Refusal::Required(format!(
                "it's signed by {}, a key this machine doesn't trust",
                signature.key
            )));
        };
        if signature.at.abs_diff(now_ms) > protocol::WINDOW_MS {
            return Err(Refusal::Invalid(clock_note(signature, now_ms)));
        }
        let message = protocol::command_signing_string(
            &self.org,
            device_id,
            &request.id,
            &request.op,
            &request.data,
            signature,
        );
        verify(&signer.public, &message, &signature.sig).map_err(|()| {
            Refusal::Invalid(
                "the signature doesn't match this command, this machine or its organization"
                    .to_owned(),
            )
        })?;
        Ok(signer)
    }

    /// What the hello and `cntrl policy show` say.
    pub fn state(&self) -> SignatureState {
        SignatureState {
            required: true,
            seq: Some(self.seq()),
            head: Some(self.head.clone()),
            error: None,
        }
    }
}

/// Why a command is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Unsigned, or signed by a key this machine doesn't trust.
    Required(String),
    /// Signed, but the signature doesn't hold.
    Invalid(String),
}

/// A clock that's off says so, since boards without a real-time clock drift.
fn clock_note(signature: &CommandSignature, now_ms: u64) -> String {
    let minutes = signature.at.abs_diff(now_ms) / 60_000;
    format!(
        "the signature is {minutes} minutes {} this machine's clock; if the command is new, this machine's clock is off",
        if signature.at > now_ms {
            "ahead of"
        } else {
            "behind"
        }
    )
}

/// A key from the log, its ID and name checked.
fn signer(key: &cntrl_protocol::signers::SignerKey) -> Result<Signer, String> {
    if protocol::key_id(&key.public).as_deref() != Some(key.id.as_str()) {
        return Err(format!("{} isn't the ID of its key", key.id));
    }
    if !protocol::name_is_clean(&key.name) {
        return Err(format!("{}'s name isn't fit to show", key.id));
    }
    Ok(Signer {
        id: key.id.clone(),
        public: key.public.clone(),
        name: key.name.clone(),
        fingerprint: protocol::fingerprint(&key.public).unwrap_or_default(),
    })
}

/// Checks an ECDSA P-256 signature, r‖s base64url, over `message`.
fn verify(public: &str, message: &str, signature: &str) -> Result<(), ()> {
    let public = URL_SAFE_NO_PAD.decode(public).map_err(|_| ())?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| ())?;
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
        .verify(message.as_bytes(), &signature)
        .map_err(|_| ())
}

/// The pinned file: its log, in full.
#[derive(Debug, Serialize, Deserialize)]
struct Pinned {
    entries: Vec<SignerEntry>,
}

/// Where this machine stands, from its pinned file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pin {
    /// No file: changing operations need no signature.
    Off,
    /// A pinned log that checks out.
    On(Trust),
    /// A file that's unreadable, untrusted or doesn't check out: every
    /// changing operation is refused until someone at the machine fixes it.
    Broken(String),
}

impl Pin {
    pub fn state(&self) -> SignatureState {
        match self {
            Pin::Off => SignatureState {
                required: false,
                seq: None,
                head: None,
                error: None,
            },
            Pin::On(trust) => trust.state(),
            Pin::Broken(reason) => SignatureState {
                required: true,
                seq: None,
                head: None,
                error: Some(reason.clone()),
            },
        }
    }
}

/// What privd answers about the pinned log: where the machine stands, and the
/// log itself, which the agent checks again before trusting it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub state: SignatureState,
    #[serde(default)]
    pub entries: Vec<SignerEntry>,
}

impl View {
    pub fn of(pin: &Pin) -> Self {
        Self {
            state: pin.state(),
            entries: match pin {
                Pin::On(trust) => trust.entries.clone(),
                _ => Vec::new(),
            },
        }
    }

    /// The machine's stance, the log checked again here: a log that doesn't
    /// check out, where one is required, is broken.
    pub fn pin(&self) -> Pin {
        if !self.state.required {
            return Pin::Off;
        }
        if let Some(error) = &self.state.error {
            return Pin::Broken(error.clone());
        }
        match Trust::from_log(&self.entries) {
            Ok(trust) => Pin::On(trust),
            Err(reason) => Pin::Broken(reason),
        }
    }
}

/// Reads the pinned file, which must belong to `owner` and be writable by
/// nobody else, as the policy must.
pub fn load(path: &Path, owner: os::Owner) -> Pin {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Pin::Off,
        Err(e) => return Pin::Broken(format!("can't read {}: {e}", path.display())),
    };
    if let Err(reason) = os::check_trusted(path, &metadata, owner) {
        return Pin::Broken(reason);
    }
    let pinned: Pinned = match fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
    {
        Ok(pinned) => pinned,
        Err(e) => return Pin::Broken(format!("{}: {e}", path.display())),
    };
    match Trust::from_log(&pinned.entries) {
        Ok(trust) => Pin::On(trust),
        Err(reason) => Pin::Broken(format!("{}: {reason}", path.display())),
    }
}

/// Writes the pinned file whole, through a temporary one, so a crash leaves
/// either the old log or the new.
pub fn save(path: &Path, trust: &Trust) -> Result<(), String> {
    let context = |e: io::Error| {
        let hint = if e.kind() == io::ErrorKind::PermissionDenied {
            format!(" (run it {})", os::AS_ROOT)
        } else {
            String::new()
        };
        format!("can't write {}: {e}{hint}", path.display())
    };
    let text = serde_json::to_string_pretty(&Pinned {
        entries: trust.entries.clone(),
    })
    .map_err(|e| e.to_string())?;
    let temporary = path.with_extension("json.new");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .shared()
        .open(&temporary)
        .map_err(context)?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(context)?;
    fs::rename(&temporary, path).map_err(context)
}

/// Stops requiring signatures: the pinned file goes.
pub fn unpin(path: &Path) -> Result<bool, String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("can't remove {}: {e}", path.display())),
    }
}

/// Extends a pinned log with the gateway's, for privd. Nothing pinned, or
/// nothing new, writes nothing.
pub fn apply(path: &Path, owner: os::Owner, log: &SignerLog) -> Pin {
    match load(path, owner) {
        Pin::On(trust) => match trust.extend(&log.entries) {
            Ok(Some(next)) => {
                // Checked, so trusted now even if it can't be kept: the
                // gateway sends the log again on the next connection.
                if let Err(reason) = save(path, &next) {
                    tracing::warn!("the extended signer log wasn't kept: {reason}");
                }
                Pin::On(next)
            }
            Ok(None) => Pin::On(trust),
            Err(reason) => {
                // The pinned log stands; the gateway's is refused.
                tracing::warn!("signer log refused: {reason}");
                Pin::On(trust)
            }
        },
        other => other,
    }
}

/// Request IDs of signed commands acted on lately, kept on disk so a replay
/// is refused across restarts too: `signed-requests.json` in the agent's
/// state directory. An ID is forgotten once its signature is too old to pass
/// anyway.
pub struct Seen {
    path: PathBuf,
    ids: BTreeMap<String, u64>,
}

impl Seen {
    pub fn open(state_dir: &Path) -> Self {
        let path = state_dir.join("signed-requests.json");
        let ids = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, ids }
    }

    /// Records `id`, or says it was seen before. The record is on disk before
    /// the command runs.
    pub fn first_use(&mut self, id: &str, now_ms: u64) -> Result<(), String> {
        self.ids
            .retain(|_, at| now_ms.saturating_sub(*at) <= 2 * protocol::WINDOW_MS);
        if self.ids.contains_key(id) {
            return Err(format!(
                "request {id} was acted on already; a replay is refused"
            ));
        }
        self.ids.insert(id.to_owned(), now_ms);
        let text = serde_json::to_string(&self.ids).map_err(|e| e.to_string())?;
        let temporary = self.path.with_extension("json.new");
        fs::write(&temporary, text)
            .and_then(|()| fs::rename(&temporary, &self.path))
            .map_err(|e| format!("can't record request {id}: {e}"))
    }
}

/// What the uplink checks commands against: where the machine stands, as
/// privd last said (on every connection, and with every signer log), and the
/// requests acted on lately. Until privd has said, nothing that needs a
/// signature passes.
pub struct Gate {
    state: std::sync::Mutex<(Pin, Seen)>,
}

impl Gate {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            state: std::sync::Mutex::new((
                Pin::Broken("the trusted signers haven't been read yet".to_owned()),
                Seen::open(state_dir),
            )),
        }
    }

    pub fn set(&self, pin: Pin) {
        self.lock().0 = pin;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (Pin, Seen)> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Lets a command that changes the machine through, or says why not: none
    /// needed where signatures aren't required; otherwise a trusted signer's,
    /// whose request is recorded so it can't be used again.
    pub fn admit(
        &self,
        device_id: &str,
        request: &Request,
        now_ms: u64,
    ) -> Result<Option<Signer>, Refusal> {
        let mut state = self.lock();
        let (pin, seen) = &mut *state;
        match pin {
            Pin::Off => Ok(None),
            Pin::Broken(reason) => Err(Refusal::Required(format!(
                "this machine requires signed commands, and can't read whom it trusts: {reason}"
            ))),
            Pin::On(trust) => {
                let signer = trust.check(device_id, request, now_ms)?.clone();
                seen.first_use(&request.id, now_ms)
                    .map_err(Refusal::Invalid)?;
                Ok(Some(signer))
            }
        }
    }
}

/// The log the gateway sent last, kept in the agent's state directory so
/// `cntrl policy require-signatures` can show and pin it.
pub fn latest_path(state_dir: &Path) -> PathBuf {
    state_dir.join("signer-log.json")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use serde_json::Value;

    fn vector() -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/protocol/v1/signers/vector.json");
        serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("parse")
    }

    fn entries() -> Vec<SignerEntry> {
        vector()["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|case| serde_json::from_value(case["entry"].clone()).expect("entry"))
            .collect()
    }

    fn command() -> (Request, String, u64) {
        let command = vector()["command"].clone();
        let signature: CommandSignature =
            serde_json::from_value(command["signature"].clone()).expect("signature");
        let at = signature.at;
        let request = Request {
            id: command["request_id"].as_str().unwrap().to_owned(),
            op: command["op"].as_str().unwrap().to_owned(),
            ver: 1,
            deadline_ms: 30_000,
            data: command["data"].clone(),
            actor: None,
            idem: None,
            sig: Some(signature),
        };
        (
            request,
            command["device_id"].as_str().unwrap().to_owned(),
            at,
        )
    }

    #[test]
    fn a_log_from_webcrypto_checks_out() {
        let log = entries();
        let trust = Trust::from_log(&log).expect("trust");
        // Added two, removed the second.
        assert_eq!(trust.signers.len(), 1);
        assert_eq!(trust.seq(), 2);
        let only = Trust::from_log(&log[..2]).expect("first two");
        assert_eq!(only.signers.len(), 2);
    }

    #[test]
    fn a_command_from_webcrypto_checks_out() {
        let trust = Trust::from_log(&entries()).expect("trust");
        let (request, device, at) = command();
        let signer = trust.check(&device, &request, at + 1_000).expect("signed");
        assert_eq!(signer.name, "Alok, Chrome on Mac");
    }

    #[test]
    fn tampered_commands_are_refused() {
        let trust = Trust::from_log(&entries()).expect("trust");
        let (request, device, at) = command();
        let invalid = |request: &Request, device: &str| {
            matches!(trust.check(device, request, at), Err(Refusal::Invalid(_)))
        };
        let mut data = request.clone();
        data.data["unit"] = Value::from("sshd.service");
        assert!(invalid(&data, &device), "other data");
        let mut op = request.clone();
        op.op = "service.stop".into();
        assert!(invalid(&op, &device), "another operation");
        let mut id = request.clone();
        id.id = "req_01m4signersvector00000002".into();
        assert!(invalid(&id, &device), "another request");
        assert!(invalid(&request, "dev_someoneelse"), "another device");
        // Too far from the clock, either way.
        assert!(matches!(
            trust.check(&device, &request, at + protocol::WINDOW_MS + 1),
            Err(Refusal::Invalid(note)) if note.contains("clock")
        ));
        let mut unsigned = request.clone();
        unsigned.sig = None;
        assert!(matches!(
            trust.check(&device, &unsigned, at),
            Err(Refusal::Required(_))
        ));
    }

    #[test]
    fn a_removed_key_is_refused() {
        let log = entries();
        let trust = Trust::from_log(&log).expect("trust");
        let (mut request, device, at) = command();
        let ana = vector()["keys"][1]["id"].as_str().unwrap().to_owned();
        request.sig.as_mut().unwrap().key = ana;
        assert!(matches!(
            trust.check(&device, &request, at),
            Err(Refusal::Required(_))
        ));
    }

    #[test]
    fn logs_that_fork_skip_or_roll_back_are_refused() {
        let log = entries();
        let pinned = Trust::from_log(&log[..2]).expect("pinned at 1");
        // Following on is fine; the same log is nothing new.
        assert!(pinned.extend(&log).expect("extend").is_some());
        assert!(pinned.extend(&log[..2]).expect("same").is_none());
        // Older than what's trusted.
        assert!(pinned.extend(&log[..1]).is_err());
        // A different history: a changed entry 1.
        let mut forked = log.clone();
        forked[1].at += 1;
        assert!(pinned.extend(&forked).is_err());
        // A gap: entry 2 without 1.
        let mut gap = vec![log[0].clone(), log[2].clone()];
        assert!(Trust::from_log(&gap).is_err());
        gap[1].seq = 1;
        assert!(
            Trust::from_log(&gap).is_err(),
            "names the wrong previous hash"
        );
        // Another organization's entry.
        let mut other = log.clone();
        other[2].org = "ws_other".into();
        assert!(Trust::from_log(&other).is_err());
        // An entry signed by a key that isn't trusted, or a bad signature.
        let mut forged = log.clone();
        forged[2].sig = forged[1].sig.clone();
        assert!(Trust::from_log(&forged).is_err());
        assert!(Trust::from_log(&[]).is_err());
    }

    #[test]
    fn the_last_signer_cant_be_removed() {
        let log = entries();
        let mut trust = Trust::from_log(&log[..1]).expect("first");
        let mut remove = log[2].clone();
        remove.seq = 1;
        remove.prev = Some(trust.head.clone());
        remove.change = SignerChange::Remove {
            key_id: trust.signers.keys().next().unwrap().clone(),
        };
        let refused = trust.apply(&remove);
        assert!(refused.is_err());
    }

    #[test]
    fn a_replay_is_refused_after_a_restart_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut seen = Seen::open(dir.path());
        seen.first_use("req_a", 1_000).expect("first");
        assert!(seen.first_use("req_a", 2_000).is_err());
        let mut again = Seen::open(dir.path());
        assert!(again.first_use("req_a", 3_000).is_err(), "kept on disk");
        // Long after its signature could pass, it's forgotten.
        assert!(
            again
                .first_use("req_a", 1_000 + 2 * protocol::WINDOW_MS + 1)
                .is_ok()
        );
    }
}
