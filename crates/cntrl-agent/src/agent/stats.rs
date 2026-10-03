//! The sampler behind the `stats` topic. It reads the host's counters every
//! 5 s, and every second while anyone is subscribed, and keeps the latest
//! sample in a watch channel. Each session with a `stats` subscription holds one
//! receiver, so the receiver count says whether anyone is watching.

use std::sync::Arc;
use std::time::Duration;

use cntrl_host::stats::{Stats, StatsReading};
use cntrl_protocol::stats::StatsSample;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::uplink::now_ms;

/// The latest sample, `None` until the second reading.
pub type Latest = watch::Sender<Option<Arc<StatsSample>>>;

/// How often the sampler wakes, and reads the counters while someone watches.
const LIVE_INTERVAL: Duration = Duration::from_secs(1);
/// How often it reads them while nobody does.
const IDLE_INTERVAL: Duration = Duration::from_secs(5);
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
    let mut previous: Option<StatsReading> = None;
    let mut last_read: Option<Instant> = None;
    let mut last_error: Option<String> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = token.cancelled() => return Ok(()),
        }
        let watched = latest.receiver_count() > 0;
        if !watched && last_read.is_some_and(|at| at.elapsed() < IDLE_INTERVAL) {
            continue;
        }
        last_read = Some(Instant::now());
        match read(&host).await {
            Ok(reading) => {
                last_error = None;
                if let Some(earlier) = &previous {
                    latest.send_replace(Some(Arc::new(reading.since(earlier, now_ms()))));
                }
                previous = Some(reading);
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

    /// Runs the sampler for `elapsed` on the paused clock.
    async fn sample_for(host: &Arc<FakeStats>, latest: &Arc<Latest>, elapsed: Duration) {
        let token = CancellationToken::new();
        let host: Arc<dyn Stats> = Arc::clone(host) as Arc<dyn Stats>;
        let sampler = tokio::spawn(run(host, Arc::clone(latest), token.clone()));
        tokio::time::sleep(elapsed).await;
        token.cancel();
        assert_eq!(sampler.await.expect("the sampler ran"), Ok(()));
    }

    #[tokio::test(start_paused = true)]
    async fn reads_every_five_seconds_while_unwatched() {
        let host = Arc::new(FakeStats::new([
            reading(0, 0),
            reading(10, 10),
            reading(30, 20),
        ]));
        let latest = Arc::new(Latest::new(None));
        sample_for(&host, &latest, Duration::from_millis(10_500)).await;
        // At 0 s, 5 s and 10 s.
        assert_eq!(host.reads(), 3);
        let busy = latest.borrow().as_ref().map(|sample| sample.cpu.busy);
        assert_eq!(busy, Some(0.6667));
    }

    #[tokio::test(start_paused = true)]
    async fn reads_every_second_while_watched() {
        let host = Arc::new(FakeStats::new([reading(0, 0), reading(10, 10)]));
        let latest = Arc::new(Latest::new(None));
        let _watcher = latest.subscribe();
        sample_for(&host, &latest, Duration::from_millis(3_500)).await;
        // At 0, 1, 2 and 3 s.
        assert_eq!(host.reads(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn needs_two_readings_for_a_sample() {
        let host = Arc::new(FakeStats::new([reading(0, 0)]));
        let latest = Arc::new(Latest::new(None));
        sample_for(&host, &latest, Duration::from_millis(500)).await;
        assert_eq!(host.reads(), 1);
        assert!(latest.borrow().is_none());
    }
}
