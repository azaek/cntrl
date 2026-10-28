//! The sampler behind the `processes` topic (D24): the whole process table
//! every 2 s, only while some session has a `processes` subscription. It keeps
//! the latest table in a watch channel, whose receiver count says whether
//! anyone is watching. On Linux the agent reads the table itself; on macOS
//! and Windows only root, or SYSTEM, sees other users' processes, so it asks
//! privd.

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

/// Reads the whole table once, as a follow-up's setup and its stop step need
/// it now rather than at the next sample (D121).
pub async fn read_once(privd: PathBuf) -> Result<Vec<ProcessInfo>, String> {
    Reader::new(privd).read().await
}

/// One process's running totals, as privd reads them for a follow-up (D121).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Used {
    pub pid: u32,
    pub started: u64,
    pub cpu_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children_cpu_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_bytes: Option<u64>,
}

/// The running totals of these processes, by PID and start time; one that
/// has ended, or whose PID now names another, is left out. privd reads them,
/// since another user's take root.
pub async fn usage(
    privd: &std::path::Path,
    processes: Vec<(u32, u64)>,
) -> Result<Vec<Used>, String> {
    use super::ipc::{self, Call};

    let value = ipc::call_within(
        privd,
        Call::ProcessUsage { processes },
        Duration::from_secs(10),
    )
    .await
    .map_err(|e| e.msg)?;
    serde_json::from_value(value).map_err(|e| format!("privd's process totals are unreadable: {e}"))
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

#[cfg(any(target_os = "macos", windows))]
struct Reader {
    privd: PathBuf,
}

#[cfg(any(target_os = "macos", windows))]
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

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
struct Reader;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
impl Reader {
    fn new(_privd: PathBuf) -> Self {
        Self
    }

    #[allow(clippy::unused_async)]
    async fn read(&self) -> Result<Vec<ProcessInfo>, String> {
        Err("this OS has no process table yet".to_owned())
    }
}
