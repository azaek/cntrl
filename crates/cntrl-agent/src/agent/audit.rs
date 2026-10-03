//! The local audit log: JSON Lines in `audit.jsonl`, owned by root, each record
//! carrying the SHA-256 of the line before it, so an edited or deleted line
//! breaks the chain. Each record is synced to disk before the action it
//! describes runs. Opening the log checks the whole chain and refuses a broken
//! one.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cntrl_protocol::frame::RecordKind;
use cntrl_protocol::records::AuditCheckpoint;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::digest::sha256_hex;
use super::identity;
use super::ipc::{self, Call};
use super::outbox::Outbox;

/// How often the agent asks privd for a checkpoint. While the log is unchanged,
/// privd has none to give.
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(300);

/// The `prev` of the first record.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub const FILE_NAME: &str = "audit.jsonl";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub seq: u64,
    pub ts_ms: u64,
    /// SHA-256 of the previous line, or [`GENESIS`].
    pub prev: String,
    /// Which half wrote it: `agent` or `privd`.
    pub source: String,
    pub kind: String,
    pub data: Value,
}

pub struct AuditLog {
    file: File,
    next_seq: u64,
    head: String,
}

impl AuditLog {
    /// The next record's sequence number, and the hash it will point at.
    pub fn head(&self) -> (u64, String) {
        (self.next_seq, self.head.clone())
    }

    /// Opens the log in `dir`, creating both if needed, and continues after its
    /// last record.
    pub fn open(dir: &Path) -> Result<Self, String> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("can't create {}: {e}", dir.display()))?;
        let path = dir.join(FILE_NAME);
        let (next_seq, head) = match File::open(&path) {
            Ok(file) => {
                check_chain(BufReader::new(file)).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (0, GENESIS.to_owned()),
            Err(e) => return Err(format!("can't read {}: {e}", path.display())),
        };
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("can't open {}: {e}", path.display()))?;
        Ok(Self {
            file,
            next_seq,
            head,
        })
    }

    /// Appends one record and syncs it to disk. Returns its sequence number and
    /// the hash that the next record will point at.
    pub fn append(
        &mut self,
        source: &str,
        kind: &str,
        data: Value,
    ) -> Result<(u64, String), String> {
        let record = Record {
            seq: self.next_seq,
            ts_ms: now_ms(),
            prev: self.head.clone(),
            source: source.to_owned(),
            kind: kind.to_owned(),
            data,
        };
        let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
        self.file
            .write_all(format!("{line}\n").as_bytes())
            .and_then(|()| self.file.sync_data())
            .map_err(|e| format!("can't write the audit log: {e}"))?;
        self.head = sha256_hex(line.as_bytes());
        self.next_seq += 1;
        Ok((record.seq, self.head.clone()))
    }
}

/// Asks privd for a signed checkpoint every 5 minutes and queues it for
/// Console. A checkpoint names the device, so none is asked for before
/// enrollment.
pub async fn checkpoints(
    outbox: Arc<Outbox>,
    state_dir: PathBuf,
    privd: PathBuf,
    token: CancellationToken,
) -> Result<(), String> {
    let mut ticker = tokio::time::interval(CHECKPOINT_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last: Option<u64> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = token.cancelled() => return Ok(()),
        }
        let identity = match identity::load(&state_dir) {
            Ok(Some(identity)) => identity,
            Ok(None) => continue,
            Err(e) => {
                warn!("can't read the identity for a checkpoint: {e}");
                continue;
            }
        };
        let call = Call::AuditCheckpoint {
            device_id: identity.device_id,
            key_id: identity.audit_key_id,
            after: last,
        };
        match ipc::call_once(&privd, call).await {
            Ok(Value::Null) => {}
            // Read back first, so a malformed answer never sits in the outbox.
            Ok(data) => match serde_json::from_value::<AuditCheckpoint>(data.clone()) {
                Ok(checkpoint) => {
                    last = Some(checkpoint.seq);
                    outbox.push(RecordKind::AuditCheckpoint, data).await;
                }
                Err(e) => warn!("privd sent an unreadable checkpoint: {e}"),
            },
            Err(e) => warn!("can't get an audit checkpoint: {e}"),
        }
    }
}

/// Checks the chain of a log file. Returns the next sequence number and the
/// head hash.
pub fn verify(path: &Path) -> Result<(u64, String), String> {
    let file = File::open(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    check_chain(BufReader::new(file))
}

fn check_chain(reader: impl BufRead) -> Result<(u64, String), String> {
    let (mut next_seq, mut head) = (0, GENESIS.to_owned());
    for (index, line) in reader.lines().enumerate() {
        let number = index + 1;
        let line = line.map_err(|e| format!("line {number}: {e}"))?;
        let record: Record =
            serde_json::from_str(&line).map_err(|e| format!("line {number}: {e}"))?;
        if record.seq != next_seq || record.prev != head {
            return Err(format!("line {number}: the chain is broken"));
        }
        head = sha256_hex(line.as_bytes());
        next_seq += 1;
    }
    Ok((next_seq, head))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn records_chain_and_survive_a_reopen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut log = AuditLog::open(dir.path()).expect("open");
        let (first, _) = log
            .append("agent", "agent.started", json!({"version": "0.1.0"}))
            .expect("append");
        let (second, head) = log
            .append("privd", "policy.denied", json!({"op": "service.restart"}))
            .expect("append");
        assert_eq!((first, second), (0, 1));
        drop(log);

        let mut reopened = AuditLog::open(dir.path()).expect("reopen");
        assert_eq!(reopened.head, head);
        let (third, _) = reopened
            .append("agent", "agent.stopped", json!({}))
            .expect("append");
        assert_eq!(third, 2);
        assert_eq!(verify(&dir.path().join(FILE_NAME)).expect("verify").0, 3);
    }

    #[test]
    fn an_edited_line_breaks_the_chain() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut log = AuditLog::open(dir.path()).expect("open");
        log.append("agent", "agent.started", json!({"version": "0.1.0"}))
            .expect("append");
        log.append("agent", "agent.stopped", json!({}))
            .expect("append");
        drop(log);

        let path = dir.path().join(FILE_NAME);
        let edited =
            fs::read_to_string(&path)
                .expect("read")
                .replacen("agent.started", "agent.trusted", 1);
        fs::write(&path, edited).expect("write");
        assert!(verify(&path).is_err());
        assert!(AuditLog::open(dir.path()).is_err());
    }
}
