//! The sampler behind the `stats` topic and the 60 s stats records. It reads
//! the host's counters every 5 s, and every second while anyone is subscribed,
//! and keeps the latest sample in a watch channel. Each session with a `stats`
//! subscription holds one receiver, so the receiver count says whether anyone is
//! watching. About once a minute it sums the readings into a record for the
//! outbox.

use std::sync::Arc;
use std::time::Duration;

use cntrl_host::stats::{Stats, StatsReading};
use cntrl_protocol::frame::RecordKind;
use cntrl_protocol::records::{CpuRecord, StatsRecord};
use cntrl_protocol::stats::StatsSample;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::outbox::Outbox;
use super::uplink::now_ms;

/// The latest sample, `None` until the second reading.
pub type Latest = watch::Sender<Option<Arc<StatsSample>>>;

/// How often the sampler wakes, and reads the counters while someone watches.
const LIVE_INTERVAL: Duration = Duration::from_secs(1);
/// How often it reads them while nobody does.
const IDLE_INTERVAL: Duration = Duration::from_secs(5);
/// How long one reading may take.
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// How much time each stats record covers.
const RECORD_PERIOD: Duration = Duration::from_secs(60);

/// The record being gathered: the reading it started from, and its busiest
/// sample so far.
struct Window {
    start: StatsReading,
    started: Instant,
    busy_max: f64,
}

impl Window {
    fn new(start: StatsReading) -> Self {
        Self {
            start,
            started: Instant::now(),
            busy_max: 0.0,
        }
    }

    /// The record for the window, ending at `end`: CPU from the counters at
    /// both ends, load and memory as last read.
    fn record(&self, end: &StatsReading, ts: u64) -> StatsRecord {
        let over = end.since(&self.start, ts);
        StatsRecord {
            ts,
            period_ms: u32::try_from(self.started.elapsed().as_millis()).unwrap_or(u32::MAX),
            cpu: CpuRecord {
                busy: over.cpu.busy,
                // Rounding can leave the busiest sample a hair under the average.
                busy_max: self.busy_max.max(over.cpu.busy),
                load: over.cpu.load,
            },
            memory: over.memory,
        }
    }
}

/// Samples until shutdown. Read errors are logged, and sampling carries on.
pub async fn run(
    host: Arc<dyn Stats>,
    latest: Arc<Latest>,
    outbox: Arc<Outbox>,
    token: CancellationToken,
) -> Result<(), String> {
    let mut ticker = tokio::time::interval(LIVE_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut previous: Option<StatsReading> = None;
    let mut window: Option<Window> = None;
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
                let ts = now_ms();
                if let Some(earlier) = &previous {
                    let sample = reading.since(earlier, ts);
                    if let Some(window) = &mut window {
                        window.busy_max = window.busy_max.max(sample.cpu.busy);
                    }
                    latest.send_replace(Some(Arc::new(sample)));
                }
                match &window {
                    Some(open) if open.started.elapsed() >= RECORD_PERIOD => {
                        match serde_json::to_value(open.record(&reading, ts)) {
                            Ok(record) => outbox.push(RecordKind::Metrics, record).await,
                            Err(e) => warn!("can't encode a stats record: {e}"),
                        }
                        window = Some(Window::new(reading.clone()));
                    }
                    Some(_) => {}
                    None => window = Some(Window::new(reading.clone())),
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

    /// Runs the sampler for `elapsed` on the paused clock; returns its outbox.
    async fn sample_for(
        host: &Arc<FakeStats>,
        latest: &Arc<Latest>,
        elapsed: Duration,
    ) -> Arc<Outbox> {
        let dir = tempfile::tempdir().expect("temp dir");
        let outbox = Arc::new(Outbox::open(dir.path()).await);
        let token = CancellationToken::new();
        let host: Arc<dyn Stats> = Arc::clone(host) as Arc<dyn Stats>;
        let sampler = tokio::spawn(run(
            host,
            Arc::clone(latest),
            Arc::clone(&outbox),
            token.clone(),
        ));
        tokio::time::sleep(elapsed).await;
        token.cancel();
        assert_eq!(sampler.await.expect("the sampler ran"), Ok(()));
        outbox
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
    async fn sums_a_minute_of_readings_into_a_record() {
        // Every 5 s, 10 ticks busy and 10 idle, except one interval with 30 busy.
        let readings = (0..30u64).map(|i| reading(10 * i + if i >= 3 { 20 } else { 0 }, 10 * i));
        let host = Arc::new(FakeStats::new(readings));
        let latest = Arc::new(Latest::new(None));
        let outbox = sample_for(&host, &latest, Duration::from_millis(125_500)).await;
        let records: Vec<StatsRecord> = outbox
            .pending(10)
            .await
            .into_iter()
            .map(|record| serde_json::from_value(record.data).expect("a stats record"))
            .collect();
        // Windows close at 60 s and 120 s.
        assert_eq!(records.len(), 2);
        let first = &records[0];
        assert_eq!(first.period_ms, 60_000);
        // 140 busy of 260 ticks; the busiest 5 s was 30 of 40.
        assert_eq!(first.cpu.busy, 0.5385);
        assert_eq!(first.cpu.busy_max, 0.75);
        assert_eq!(first.memory.available, 6_000);
        assert_eq!(records[1].cpu.busy, 0.5);
        assert_eq!(records[1].cpu.busy_max, 0.5);
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
