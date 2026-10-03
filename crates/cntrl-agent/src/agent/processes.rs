//! The sampler behind the `processes` topic (D24): the whole process table
//! every 2 s, only while some session has a `processes` subscription. It keeps
//! the latest table in a watch channel, whose receiver count says whether
//! anyone is watching. On Linux the agent reads the table itself; on macOS
//! only root sees other users' processes, so it asks privd.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cntrl_protocol::process::ProcessInfo;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::uplink::now_ms;

/// How often the table is read while someone watches.
pub const INTERVAL: Duration = Duration::from_secs(2);
/// How often the sampler looks for someone watching.
const CHECK: Duration = Duration::from_millis(250);

/// One read of the whole table.
#[derive(Debug)]
pub struct Table {
    /// When it was read, in Unix milliseconds.
    pub ts: u64,
    pub processes: Vec<ProcessInfo>,
}

pub type Latest = watch::Sender<Option<Arc<Table>>>;

/// Reads the table every [`INTERVAL`] while anyone watches, until shutdown.
/// Read errors are logged once each, and reading carries on.
pub async fn run(
    latest: Arc<Latest>,
    privd: PathBuf,
    token: CancellationToken,
) -> Result<(), String> {
    let reader = Reader::new(privd);
    let mut ticker = tokio::time::interval(CHECK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_read: Option<Instant> = None;
    let mut last_error: Option<String> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = token.cancelled() => return Ok(()),
        }
        if latest.receiver_count() == 0 || last_read.is_some_and(|at| at.elapsed() < INTERVAL) {
            continue;
        }
        last_read = Some(Instant::now());
        match reader.read().await {
            Ok(processes) => {
                last_error = None;
                latest.send_replace(Some(Arc::new(Table {
                    ts: now_ms(),
                    processes,
                })));
            }
            Err(e) => {
                if last_error.as_deref() != Some(e.as_str()) {
                    warn!("can't read the process table: {e}");
                    last_error = Some(e);
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
struct Reader {
    sampler: Arc<std::sync::Mutex<cntrl_host::processes::Sampler>>,
}

#[cfg(target_os = "linux")]
impl Reader {
    fn new(_privd: PathBuf) -> Self {
        Self {
            sampler: Arc::default(),
        }
    }

    async fn read(&self) -> Result<Vec<ProcessInfo>, String> {
        let sampler = Arc::clone(&self.sampler);
        tokio::task::spawn_blocking(move || {
            sampler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .read()
        })
        .await
        .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "macos")]
struct Reader {
    privd: PathBuf,
}

#[cfg(target_os = "macos")]
impl Reader {
    fn new(privd: PathBuf) -> Self {
        Self { privd }
    }

    async fn read(&self) -> Result<Vec<ProcessInfo>, String> {
        use super::ipc::{self, Call};

        #[derive(serde::Deserialize)]
        struct Listed {
            processes: Vec<ProcessInfo>,
        }
        let value = ipc::call_within(&self.privd, Call::ProcessList, Duration::from_secs(10))
            .await
            .map_err(|e| e.msg)?;
        serde_json::from_value::<Listed>(value)
            .map(|listed| listed.processes)
            .map_err(|e| format!("privd's process table is unreadable: {e}"))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
struct Reader;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Reader {
    fn new(_privd: PathBuf) -> Self {
        Self
    }

    #[allow(clippy::unused_async)]
    async fn read(&self) -> Result<Vec<ProcessInfo>, String> {
        Err("this OS has no process table yet".to_owned())
    }
}
