//! The outbox: records Console must receive at least once. Each gets the next
//! number, stays on disk until Console acknowledges it, and goes out again after
//! a reconnect or a restart; Console drops repeats by number. Since Console
//! keeps no device data (D26), nothing records into it: the stats and audit
//! checkpoints it used to carry are gone, and any still queued are dropped on
//! opening. Alerts the device decides itself go through it (D43).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use cntrl_protocol::frame::{OutboxRecord, OutboxState, RecordKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, Notify};
use tracing::warn;

use super::os::Private;
use super::uplink::now_ms;

pub const OUTBOX_FILE: &str = "outbox.json";
/// How many records the outbox holds.
const MAX_RECORDS: usize = 10_000;
/// The records' data stays under about this many bytes.
const MAX_BYTES: usize = 5 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Saved {
    next_seq: u64,
    records: VecDeque<OutboxRecord>,
}

impl Saved {
    /// A new outbox numbers from the current time in milliseconds, so even
    /// one that lost its file never reuses a number Console has seen: records
    /// go out a few a minute, far slower than the clock.
    fn fresh() -> Self {
        Self {
            next_seq: now_ms(),
            records: VecDeque::new(),
        }
    }
}

pub struct Outbox {
    path: PathBuf,
    /// Held across each change and its write, so writes land in order.
    saved: Mutex<Saved>,
    /// Signalled when a record is added.
    pub added: Notify,
}

impl Outbox {
    /// Opens the outbox in `state_dir`. A file that can't be read starts it
    /// empty, with a warning: losing queued records beats not starting.
    pub async fn open(state_dir: &Path) -> Self {
        let path = state_dir.join(OUTBOX_FILE);
        let saved = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!("{} is unreadable, starting it empty: {e}", path.display());
                Saved::fresh()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved::fresh(),
            Err(e) => {
                warn!("can't read {}, starting it empty: {e}", path.display());
                Saved::fresh()
            }
        };
        let outbox = Self {
            path,
            saved: Mutex::new(saved),
            added: Notify::new(),
        };
        // Records from before D26 don't go out: Console keeps no device data.
        let mut saved = outbox.saved.lock().await;
        let queued = saved.records.len();
        saved.records.retain(|record| {
            !matches!(
                record.kind,
                RecordKind::Metrics | RecordKind::AuditCheckpoint
            )
        });
        if saved.records.len() != queued {
            outbox.save(&saved).await;
        }
        drop(saved);
        outbox
    }

    /// Adds a record. When the outbox is full the oldest records go first.
    pub async fn push(&self, kind: RecordKind, data: Value) {
        let mut saved = self.saved.lock().await;
        let seq = saved.next_seq;
        saved.next_seq += 1;
        saved.records.push_back(OutboxRecord { seq, kind, data });
        trim(&mut saved.records);
        self.save(&saved).await;
        drop(saved);
        self.added.notify_one();
    }

    /// The oldest records Console hasn't acknowledged, at most `limit`.
    pub async fn pending(&self, limit: usize) -> Vec<OutboxRecord> {
        let saved = self.saved.lock().await;
        saved.records.iter().take(limit).cloned().collect()
    }

    /// Drops every record up to and including `upto`, which Console has.
    pub async fn ack(&self, upto: u64) {
        let mut saved = self.saved.lock().await;
        let before = saved.records.len();
        saved.records.retain(|record| record.seq > upto);
        if saved.records.len() != before {
            self.save(&saved).await;
        }
    }

    /// Drops every record of `kind` Console hasn't acknowledged: the alerts
    /// from before the device was disabled, which mustn't open incidents once
    /// it's back (D87).
    pub async fn discard(&self, kind: RecordKind) {
        let mut saved = self.saved.lock().await;
        let before = saved.records.len();
        saved.records.retain(|record| record.kind != kind);
        if saved.records.len() != before {
            self.save(&saved).await;
        }
    }

    /// What the hello reports.
    pub async fn state(&self) -> OutboxState {
        let saved = self.saved.lock().await;
        OutboxState {
            next_seq: saved.next_seq,
            oldest_unacked: saved.records.front().map(|record| record.seq),
        }
    }

    async fn save(&self, saved: &Saved) {
        let written = match serde_json::to_vec(saved) {
            Ok(bytes) => write_atomically(&self.path, &bytes).await,
            Err(e) => Err(std::io::Error::other(e)),
        };
        if let Err(e) = written {
            warn!("can't save the outbox to {}: {e}", self.path.display());
        }
    }
}

/// Drops the oldest stats records until the outbox fits its limits.
fn trim(records: &mut VecDeque<OutboxRecord>) {
    let mut bytes: usize = records.iter().map(size).sum();
    while records.len() > MAX_RECORDS || bytes > MAX_BYTES {
        let Some(oldest) = records
            .iter()
            .position(|record| record.kind != RecordKind::AuditCheckpoint)
        else {
            return;
        };
        if let Some(dropped) = records.remove(oldest) {
            bytes -= size(&dropped);
        }
    }
}

/// Roughly what a record adds to the file.
fn size(record: &OutboxRecord) -> usize {
    serde_json::to_vec(&record.data).map_or(0, |data| data.len()) + 40
}

/// Replaces `path` through a temporary file, synced first, so a crash leaves
/// either the old outbox or the new one.
async fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("json.new");
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .private()
        .open(&temporary)
        .await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    tokio::fs::rename(&temporary, path).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn numbers_records_and_drops_what_console_acknowledged() {
        let dir = tempfile::tempdir().expect("temp dir");
        let outbox = Outbox::open(dir.path()).await;
        let first = outbox.state().await.next_seq;
        for n in 0..3 {
            outbox.push(RecordKind::Metrics, json!({ "n": n })).await;
        }
        let pending = outbox.pending(10).await;
        let seqs: Vec<u64> = pending.iter().map(|r| r.seq - first).collect();
        assert_eq!(seqs, [0, 1, 2]);
        outbox.ack(first + 1).await;
        let state = outbox.state().await;
        assert_eq!(state.next_seq - first, 3);
        assert_eq!(state.oldest_unacked, Some(first + 2));
        assert_eq!(outbox.pending(1).await.len(), 1);
    }

    #[tokio::test]
    async fn discarding_a_kind_drops_only_its_records_and_holds_after_a_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let outbox = Outbox::open(dir.path()).await;
        outbox.push(RecordKind::Alert, json!({ "rule": "a" })).await;
        outbox.push(RecordKind::Unknown, json!({})).await;
        outbox.push(RecordKind::Alert, json!({ "rule": "b" })).await;
        outbox.discard(RecordKind::Alert).await;
        let kinds =
            |records: Vec<OutboxRecord>| records.into_iter().map(|r| r.kind).collect::<Vec<_>>();
        assert_eq!(kinds(outbox.pending(10).await), [RecordKind::Unknown]);
        drop(outbox);
        let reopened = Outbox::open(dir.path()).await;
        assert_eq!(kinds(reopened.pending(10).await), [RecordKind::Unknown]);
    }

    #[tokio::test]
    async fn drops_old_records_but_keeps_numbering_after_a_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let outbox = Outbox::open(dir.path()).await;
        let first = outbox.state().await.next_seq;
        outbox
            .push(RecordKind::AuditCheckpoint, json!({ "seq": 7 }))
            .await;
        outbox.push(RecordKind::Metrics, json!({})).await;
        drop(outbox);
        // Records from before D26 don't go out, and the file forgets them too.
        let reopened = Outbox::open(dir.path()).await;
        assert!(reopened.pending(10).await.is_empty());
        drop(reopened);
        let again = Outbox::open(dir.path()).await;
        let state = again.state().await;
        assert_eq!(state.next_seq, first + 2);
        assert_eq!(state.oldest_unacked, None);
        // Numbers carry on, so Console never mistakes a new record for a repeat.
        again.push(RecordKind::Metrics, json!({})).await;
        let last = again.pending(10).await.last().map(|r| r.seq);
        assert_eq!(last, Some(first + 2));
    }

    #[tokio::test]
    async fn a_lost_outbox_numbers_past_everything_before() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(OUTBOX_FILE);
        std::fs::write(&path, r#"{"next_seq":5,"records":[]}"#).expect("write");
        assert_eq!(Outbox::open(dir.path()).await.state().await.next_seq, 5);
        std::fs::write(&path, "not json").expect("write");
        let outbox = Outbox::open(dir.path()).await;
        // It starts again from the clock, far past anything sent before.
        assert!(outbox.state().await.next_seq >= 1_700_000_000_000);
    }

    #[test]
    fn a_full_outbox_drops_old_stats_but_keeps_checkpoints() {
        let record = |seq, kind| OutboxRecord {
            seq,
            kind,
            data: json!({}),
        };
        let mut records: VecDeque<OutboxRecord> = (0..MAX_RECORDS as u64 + 2)
            .map(|seq| {
                let kind = if seq == 0 {
                    RecordKind::AuditCheckpoint
                } else {
                    RecordKind::Metrics
                };
                record(seq, kind)
            })
            .collect();
        trim(&mut records);
        assert_eq!(records.len(), MAX_RECORDS);
        assert_eq!(records[0].kind, RecordKind::AuditCheckpoint);
        // Stats records 1 and 2 went; 3 is now the oldest.
        assert_eq!(records[1].seq, 3);
    }
}
