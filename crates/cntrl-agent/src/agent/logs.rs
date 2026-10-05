//! The `logs` topic's readers (angle 11): one `journalctl` per subscription,
//! running only while it's open, its lines batched into events. A session ends
//! its readers with its subscriptions, and stopping a reader kills its
//! `journalctl`.

use std::time::Instant;

use cntrl_protocol::logs::{LogsBatch, LogsParams};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

/// How many `logs` subscriptions a session holds open at once. A new one past
/// it closes the oldest, whose viewer may have gone without the gateway
/// noticing yet (D36).
pub const MAX_OPEN: usize = 8;

/// Where readers send their batches: the subscription's ID with each.
pub type Batches = mpsc::Sender<(String, LogsBatch)>;

/// A subscription's running reader.
pub struct Reader {
    task: AbortHandle,
    /// When it started, so the oldest can give way.
    pub started: Instant,
}

impl Reader {
    /// Stops the reader and kills its `journalctl`.
    pub fn stop(&self) {
        self.task.abort();
    }

    /// A reader that reads nothing, for tests.
    #[cfg(test)]
    pub fn idle(started: Instant) -> Self {
        Self {
            task: tokio::spawn(std::future::pending::<()>()).abort_handle(),
            started,
        }
    }
}

/// Starts reading for subscription `id`. When the reader stops on its own, its
/// last batch says why.
pub fn start(id: String, params: LogsParams, out: Batches) -> Reader {
    let task = tokio::spawn(async move {
        let reason = read(&id, &params, &out).await;
        let last = LogsBatch {
            ended: Some(reason),
            ..LogsBatch::default()
        };
        let _ = out.send((id, last)).await;
    })
    .abort_handle();
    Reader {
        task,
        started: Instant::now(),
    }
}

#[cfg(target_os = "linux")]
mod reader {
    use std::time::Duration;

    use cntrl_host::journal;
    use cntrl_protocol::logs::{LogsBatch, LogsParams};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::time::{Instant, MissedTickBehavior};

    use super::super::uplink::now_ms;
    use super::Batches;

    /// How often a batch goes out while lines come.
    const FLUSH: Duration = Duration::from_millis(250);
    /// A batch goes out early once it holds this many lines or bytes.
    const BATCH_LINES: usize = 200;
    const BATCH_BYTES: usize = 64 * 1024;
    /// New lines per second a subscription passes on; more are counted as
    /// skipped. The earlier lines it starts with aren't counted.
    const RATE: u32 = 500;

    /// Reads until `journalctl` stops or the subscription closes; returns why
    /// it stopped.
    pub async fn read(id: &str, params: &LogsParams, out: &Batches) -> String {
        let started = now_ms();
        let mut child = match journal::command(params).spawn() {
            Ok(child) => child,
            Err(e) => return format!("can't run journalctl: {e}"),
        };
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return "journalctl gave nothing to read".to_owned();
        };
        let mut lines = BufReader::new(stdout).lines();
        let mut errors = BufReader::new(stderr).lines();
        let needle = params
            .grep
            .as_deref()
            .map(str::to_lowercase)
            .filter(|needle| !needle.is_empty());
        let mut batch = LogsBatch::default();
        let mut bytes = 0;
        let mut window = (Instant::now(), 0_u32);
        let mut flush = tokio::time::interval(FLUSH);
        flush.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut stderr_open = true;
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        let Some(entry) = journal::parse(&line) else { continue };
                        if needle.as_ref().is_some_and(|n| !entry.message.to_lowercase().contains(n)) {
                            continue;
                        }
                        if entry.ts >= started {
                            if window.0.elapsed() >= Duration::from_secs(1) {
                                window = (Instant::now(), 0);
                            }
                            window.1 += 1;
                            if window.1 > RATE {
                                batch.skipped += 1;
                                continue;
                            }
                        }
                        bytes += entry.message.len() + 128;
                        batch.entries.push(entry);
                        if (batch.entries.len() >= BATCH_LINES || bytes >= BATCH_BYTES)
                            && out.send((id.to_owned(), std::mem::take(&mut batch))).await.is_err()
                        {
                            return "the subscription closed".to_owned();
                        }
                        if batch.entries.is_empty() {
                            bytes = 0;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => return format!("can't read journalctl: {e}"),
                },
                line = errors.next_line(), if stderr_open => match line {
                    // Without access to the system journal, journalctl warns
                    // and goes on showing nothing, so stop with a reason.
                    Ok(Some(line)) if line.contains("insufficient permissions") => {
                        return "the agent can't read the system journal; run the install command again, which gives it access".to_owned();
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => stderr_open = false,
                },
                _ = flush.tick() => {
                    if (!batch.entries.is_empty() || batch.skipped > 0)
                        && out.send((id.to_owned(), std::mem::take(&mut batch))).await.is_err()
                    {
                        return "the subscription closed".to_owned();
                    }
                    bytes = 0;
                }
            }
        }
        if !batch.entries.is_empty() {
            let _ = out.send((id.to_owned(), batch)).await;
        }
        match child.wait().await {
            Ok(status) => format!("journalctl stopped ({status})"),
            Err(e) => format!("journalctl stopped: {e}"),
        }
    }
}

#[cfg(target_os = "linux")]
use reader::read;

/// Logs come from the systemd journal; a Mac's unified log needs privd, which
/// comes later (angle 11).
#[cfg(not(target_os = "linux"))]
async fn read(_id: &str, _params: &LogsParams, _out: &Batches) -> String {
    "this agent can't read logs on this OS yet".to_owned()
}
