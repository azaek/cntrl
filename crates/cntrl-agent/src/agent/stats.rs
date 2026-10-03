//! The sampler behind the `stats` topic. While anyone is subscribed it reads
//! the host's counters every second and keeps the latest sample in a watch
//! channel; each session with a `stats` subscription holds one receiver, so the
//! receiver count says whether anyone is watching. With nobody watching it
//! reads nothing: the samples are for live views, and Console keeps none (D26).

use std::sync::Arc;
use std::time::Duration;

use cntrl_host::stats::{Stats, StatsReading};
use cntrl_protocol::stats::StatsSample;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::uplink::now_ms;

/// The latest sample: `None` until two readings can be compared, and again
/// once nobody watches.
pub type Latest = watch::Sender<Option<Arc<StatsSample>>>;

/// How often the sampler wakes, and reads the counters while someone watches.
const LIVE_INTERVAL: Duration = Duration::from_secs(1);
/// A reading older than this isn't compared with a new one: CPU averaged over a
/// long gap would hide what's happening now.
const STALE: Duration = Duration::from_secs(3);
/// How long one reading may take.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Samples until shutdown. Read errors are logged, and sampling carries on.
pub async fn run(
    host: Arc<dyn Stats>,
    latest: Arc<Latest>,
    token: CancellationToken,
) -> Result<(), String> {
    let mut ticker = tokio::time::interval(LIVE_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut previous: Option<(StatsReading, Instant)> = None;
    let mut last_error: Option<String> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = token.cancelled() => return Ok(()),
        }
        if latest.receiver_count() == 0 {
            // The next viewer waits for a fresh sample rather than an old one.
            if previous.take().is_some() {
                latest.send_replace(None);
            }
            continue;
        }
        match read(&host).await {
            Ok(reading) => {
                last_error = None;
                let now = Instant::now();
                if let Some((earlier, at)) = &previous
                    && now.duration_since(*at) <= STALE
                {
                    latest.send_replace(Some(Arc::new(reading.since(earlier, now_ms()))));
                }
                previous = Some((reading, now));
            }
            Err(e) => {
                // Logged once per distinct error, since a failure repeats every tick.
                if last_error.as_deref() != Some(e.as_str()) {
                    warn!("can't read host stats: {e}");
                    last_error = Some(e);
                }
            }
        }
    }
}

/// Takes a reading on the blocking pool.
async fn read(host: &Arc<dyn Stats>) -> Result<StatsReading, String> {
    let host = Arc::clone(host);
    let reading = tokio::task::spawn_blocking(move || host.read());
    match tokio::time::timeout(READ_TIMEOUT, reading).await {
        Ok(Ok(result)) => result.map_err(|e| e.to_string()),
        Ok(Err(e)) => Err(format!("the reader stopped: {e}")),
        Err(_) => Err(format!("no reading within {READ_TIMEOUT:?}")),
    }
}

#[cfg(test)]
mod tests {
    use cntrl_host::stats::CpuTicks;
    use cntrl_host::stats::fake::FakeStats;
    use cntrl_protocol::stats::{LoadAverage, MemoryStats};

    use super::*;

    fn reading(busy: u64, idle: u64) -> StatsReading {
        StatsReading {
            cpu: CpuTicks { busy, idle },
            load: LoadAverage {
                one: 0.5,
                five: 0.25,
                fifteen: 0.1,
            },
            memory: MemoryStats {
                total: 8_000,
                available: 6_000,
            },
        }
    }

    fn start(
        host: &Arc<FakeStats>,
        latest: &Arc<Latest>,
    ) -> (
        tokio::task::JoinHandle<Result<(), String>>,
        CancellationToken,
    ) {
        let token = CancellationToken::new();
        let host: Arc<dyn Stats> = Arc::clone(host) as Arc<dyn Stats>;
        (
            tokio::spawn(run(host, Arc::clone(latest), token.clone())),
            token,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn reads_nothing_while_unwatched() {
        let host = Arc::new(FakeStats::new([reading(0, 0), reading(10, 10)]));
        let latest = Arc::new(Latest::new(None));
        let (sampler, token) = start(&host, &latest);
        tokio::time::sleep(Duration::from_millis(10_500)).await;
        token.cancel();
        assert_eq!(sampler.await.expect("the sampler ran"), Ok(()));
        assert_eq!(host.reads(), 0);
        assert!(latest.borrow().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn reads_every_second_while_watched() {
        let host = Arc::new(FakeStats::new([
            reading(0, 0),
            reading(10, 10),
            reading(30, 20),
        ]));
        let latest = Arc::new(Latest::new(None));
        let watcher = latest.subscribe();
        let (sampler, token) = start(&host, &latest);
        tokio::time::sleep(Duration::from_millis(3_500)).await;
        // At 0, 1, 2 and 3 s.
        assert_eq!(host.reads(), 4);
        assert!(watcher.borrow().is_some());
        token.cancel();
        assert_eq!(sampler.await.expect("the sampler ran"), Ok(()));
    }

    #[tokio::test(start_paused = true)]
    async fn needs_two_readings_for_a_sample() {
        let host = Arc::new(FakeStats::new([reading(0, 0)]));
        let latest = Arc::new(Latest::new(None));
        let _watcher = latest.subscribe();
        let (sampler, token) = start(&host, &latest);
        tokio::time::sleep(Duration::from_millis(500)).await;
        token.cancel();
        assert_eq!(sampler.await.expect("the sampler ran"), Ok(()));
        assert_eq!(host.reads(), 1);
        assert!(latest.borrow().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn forgets_the_sample_once_nobody_watches() {
        let host = Arc::new(FakeStats::new([
            reading(0, 0),
            reading(10, 10),
            reading(30, 20),
        ]));
        let latest = Arc::new(Latest::new(None));
        let watcher = latest.subscribe();
        let (sampler, token) = start(&host, &latest);
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert!(latest.borrow().is_some());
        drop(watcher);
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        assert!(latest.borrow().is_none());
        token.cancel();
        assert_eq!(sampler.await.expect("the sampler ran"), Ok(()));
    }
}
