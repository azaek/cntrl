//! The history this device keeps (D52, angle 17), so Console's charts reach
//! back further than a viewer's browser, while Console stores no readings
//! (D26). Every 10 seconds, watched or not, the agent takes a reading through
//! the stats backend it shares with the live sampler. Each minute's and each
//! quarter hour's readings become a point: every metric's lowest, highest and
//! average. Points are JSON lines, one file per tier per UTC day under
//! `history/` in the state directory: `1m/2026-10-06.jsonl`, kept 48 hours,
//! and `15m/…`, kept for the days the device keeps, 90 until told otherwise.
//! Pruning and clearing delete whole files, and a line a crash tore is
//! skipped when read.

use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cntrl_host::stats::{Stats, StatsReading};
use cntrl_protocol::ErrorCode;
use cntrl_protocol::history::{
    COLUMNS_MAX, History as Chart, HistoryMetric, HistoryParams, HistorySeries, HistoryStore,
    KEEP_DAYS_DEFAULT, KEEP_DAYS_MAX, Span,
};
use cntrl_protocol::stats::{SensorKind, StatsSample};
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::ipc::CallError;
use super::uplink::now_ms;

pub const HISTORY_DIR: &str = "history";
const SETTINGS_FILE: &str = "settings.json";
/// How often a reading is taken.
const EVERY: Duration = Duration::from_secs(10);
/// A reading older than this isn't compared with a new one: the gap, as after
/// a sleep, stays a gap.
const STALE: Duration = Duration::from_secs(60);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const MINUTE_MS: u64 = 60_000;
const QUARTER_MS: u64 = 15 * MINUTE_MS;
const DAY_MS: u64 = 24 * 60 * MINUTE_MS;
/// A span that starts within this is read from 1-minute points.
const MINUTES_REACH_MS: u64 = 2 * DAY_MS;
/// Days of 1-minute files kept before today's: past 48 hours whatever the hour.
const MINUTE_DAYS: u64 = 2;
/// The longest span `history.read` answers.
const SPAN_MAX_MS: u64 = (KEEP_DAYS_MAX as u64 + 1) * DAY_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Minute,
    Quarter,
}

impl Tier {
    const ALL: [Self; 2] = [Self::Minute, Self::Quarter];

    fn folder(self) -> &'static str {
        match self {
            Self::Minute => "1m",
            Self::Quarter => "15m",
        }
    }

    fn length(self) -> u64 {
        match self {
            Self::Minute => MINUTE_MS,
            Self::Quarter => QUARTER_MS,
        }
    }
}

/// A minute's or a quarter hour's point: a line in its day's file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Point {
    /// When the minute or quarter hour began, in Unix milliseconds.
    t: u64,
    /// Each metric's lowest, highest and average reading.
    #[serde(flatten)]
    metrics: BTreeMap<HistoryMetric, [f64; 3]>,
}

/// A metric's readings over a minute or quarter hour so far.
#[derive(Debug, Clone, Copy)]
struct Aggregate {
    min: f64,
    max: f64,
    sum: f64,
    count: u32,
}

/// The readings of the minute or quarter hour in progress.
#[derive(Debug, Clone)]
struct Gather {
    start: u64,
    metrics: BTreeMap<HistoryMetric, Aggregate>,
}

impl Gather {
    fn add(&mut self, values: &[(HistoryMetric, f64)]) {
        for &(metric, value) in values {
            self.metrics
                .entry(metric)
                .and_modify(|aggregate| {
                    aggregate.min = aggregate.min.min(value);
                    aggregate.max = aggregate.max.max(value);
                    aggregate.sum += value;
                    aggregate.count += 1;
                })
                .or_insert(Aggregate {
                    min: value,
                    max: value,
                    sum: value,
                    count: 1,
                });
        }
    }

    /// The point so far; none before any reading.
    fn point(&self) -> Option<Point> {
        let metrics: BTreeMap<_, _> = self
            .metrics
            .iter()
            .filter(|(_, aggregate)| aggregate.count > 0)
            .map(|(&metric, aggregate)| {
                let avg = aggregate.sum / f64::from(aggregate.count);
                let places = places(metric);
                (
                    metric,
                    [
                        round(aggregate.min, places),
                        round(aggregate.max, places),
                        round(avg, places),
                    ],
                )
            })
            .collect();
        (!metrics.is_empty()).then_some(Point {
            t: self.start,
            metrics,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Settings {
    keep_days: u32,
}

#[derive(Debug)]
struct State {
    keep_days: u32,
    minute: Option<Gather>,
    quarter: Option<Gather>,
}

/// The history on disk and the minute and quarter hour in progress, shared by
/// the sampler, the uplink's operations and the local API.
pub struct History {
    dir: PathBuf,
    state: Mutex<State>,
}

impl History {
    /// Opens the history under `state_dir`, making its folders. Settings that
    /// don't read mean the default days.
    pub fn open(state_dir: &Path) -> Self {
        let dir = state_dir.join(HISTORY_DIR);
        for tier in Tier::ALL {
            if let Err(e) = DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir.join(tier.folder()))
            {
                warn!("can't make {}: {e}", dir.join(tier.folder()).display());
            }
        }
        let keep_days = fs::read(dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
            .map(|settings| settings.keep_days)
            .filter(|days| (1..=KEEP_DAYS_MAX).contains(days))
            .unwrap_or(KEEP_DAYS_DEFAULT);
        Self {
            dir,
            state: Mutex::new(State {
                keep_days,
                minute: None,
                quarter: None,
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds a sample to the minute and quarter hour it falls in, and answers
    /// the points it finished, to write.
    fn add(&self, sample: &StatsSample) -> Vec<(Tier, Point)> {
        let values = values(sample);
        let mut state = self.state();
        let mut finished = Vec::new();
        for tier in Tier::ALL {
            let start = sample.ts - sample.ts % tier.length();
            let gather = match tier {
                Tier::Minute => &mut state.minute,
                Tier::Quarter => &mut state.quarter,
            };
            if gather.as_ref().is_some_and(|gather| gather.start != start)
                && let Some(point) = gather.take().and_then(|gather| gather.point())
            {
                finished.push((tier, point));
            }
            gather
                .get_or_insert_with(|| Gather {
                    start,
                    metrics: BTreeMap::new(),
                })
                .add(&values);
        }
        finished
    }

    /// The minute and quarter hour in progress, as points so far.
    fn pending(&self) -> Vec<(Tier, Point)> {
        let state = self.state();
        [
            (Tier::Minute, &state.minute),
            (Tier::Quarter, &state.quarter),
        ]
        .into_iter()
        .filter_map(|(tier, gather)| Some((tier, gather.as_ref()?.point()?)))
        .collect()
    }

    /// Writes the points in progress and forgets them, as the agent stops; a
    /// restart within the same minute adds a second point, which reading
    /// merges.
    fn save_pending(&self) {
        let points = self.pending();
        {
            let mut state = self.state();
            state.minute = None;
            state.quarter = None;
        }
        if let Err(e) = self.write(&points) {
            warn!("can't save the history in progress: {e}");
        }
    }

    /// Appends each point to its day's file.
    fn write(&self, points: &[(Tier, Point)]) -> io::Result<()> {
        for (tier, point) in points {
            let mut line = serde_json::to_vec(point).map_err(io::Error::other)?;
            line.push(b'\n');
            let path = self.file(*tier, point.t / DAY_MS);
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&path)?;
            file.write_all(&line)?;
        }
        Ok(())
    }

    fn file(&self, tier: Tier, day: u64) -> PathBuf {
        self.dir
            .join(tier.folder())
            .join(format!("{}.jsonl", day_name(day)))
    }

    /// Deletes the days past what each tier keeps.
    fn prune(&self, now: u64) -> io::Result<()> {
        let today = now / DAY_MS;
        let keep_days = u64::from(self.state().keep_days);
        for (tier, days) in [(Tier::Minute, MINUTE_DAYS), (Tier::Quarter, keep_days)] {
            let first = format!("{}.jsonl", day_name(today.saturating_sub(days)));
            for name in self.files(tier)? {
                if name < first {
                    remove(&self.dir.join(tier.folder()).join(&name))?;
                }
            }
        }
        Ok(())
    }

    /// A tier's day files, oldest first.
    fn files(&self, tier: Tier) -> io::Result<Vec<String>> {
        let folder = self.dir.join(tier.folder());
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut names = Vec::new();
        for entry in entries {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.ends_with(".jsonl") {
                names.push(name);
            }
        }
        names.sort_unstable();
        Ok(names)
    }

    /// How many days are kept, the oldest point and the bytes used.
    pub fn store(&self) -> HistoryStore {
        let mut bytes = 0;
        let mut first = [None, None];
        for (index, tier) in Tier::ALL.into_iter().enumerate() {
            let names = self.files(tier).unwrap_or_default();
            for name in &names {
                let path = self.dir.join(tier.folder()).join(name);
                bytes += fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            }
            first[index] = names
                .iter()
                .find_map(|name| first_point(&self.dir.join(tier.folder()).join(name)));
        }
        let [mut minutes, quarters] = first;
        for (tier, point) in self.pending() {
            if tier == Tier::Minute {
                minutes = Some(minutes.map_or(point.t, |t| t.min(point.t)));
            }
        }
        // A quarter hour's point is stamped with its start, before its first
        // reading; while the minutes reach back into it, they say when.
        let oldest = match (minutes, quarters) {
            (Some(minute), Some(quarter)) if quarter + QUARTER_MS <= minute => Some(quarter),
            (Some(minute), _) => Some(minute),
            (None, quarter) => quarter,
        };
        HistoryStore {
            keep_days: self.state().keep_days,
            oldest,
            bytes,
        }
    }

    /// A span cut into columns: minutes when it starts within 48 hours of
    /// `now`, quarter hours otherwise, with the points in progress.
    pub fn read(&self, params: &HistoryParams, now: u64) -> Result<Chart, CallError> {
        let HistoryParams { from, to, columns } = *params;
        if !(1..=COLUMNS_MAX).contains(&columns) {
            return Err(bad(format!(
                "columns must be from 1 to {COLUMNS_MAX}, not {columns}"
            )));
        }
        if from >= to {
            return Err(bad("the span must end after it starts"));
        }
        if to - from > SPAN_MAX_MS {
            return Err(bad(format!(
                "a span can reach {} days at most",
                KEEP_DAYS_MAX + 1
            )));
        }
        let tier = if from.saturating_add(MINUTES_REACH_MS) >= now {
            Tier::Minute
        } else {
            Tier::Quarter
        };
        let width = u128::from(to - from);
        let count = columns as usize;
        let mut merged: BTreeMap<HistoryMetric, Vec<Option<Merge>>> = BTreeMap::new();
        let mut place = |point: &Point| {
            if point.t < from || point.t >= to {
                return;
            }
            let column = u128::from(point.t - from) * u128::from(columns) / width;
            let Ok(column) = usize::try_from(column) else {
                return;
            };
            for (&metric, &[min, max, avg]) in &point.metrics {
                if metric == HistoryMetric::Unknown {
                    continue;
                }
                let spans = merged.entry(metric).or_insert_with(|| vec![None; count]);
                if let Some(slot) = spans.get_mut(column) {
                    let merge = slot.get_or_insert(Merge {
                        min,
                        max,
                        sum: 0.0,
                        count: 0,
                    });
                    merge.min = merge.min.min(min);
                    merge.max = merge.max.max(max);
                    merge.sum += avg;
                    merge.count += 1;
                }
            }
        };
        for day in from / DAY_MS..=(to - 1) / DAY_MS {
            let path = self.file(tier, day);
            let file = match fs::File::open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(CallError::internal(format!("can't read the history: {e}"))),
            };
            for line in BufReader::new(file).lines() {
                let Ok(line) = line else { break };
                // A line a crash tore, or one from a newer agent, is skipped.
                if let Ok(point) = serde_json::from_str::<Point>(&line) {
                    place(&point);
                }
            }
        }
        for (pending, point) in self.pending() {
            if pending == tier {
                place(&point);
            }
        }
        let series = merged
            .into_iter()
            .map(|(metric, spans)| HistorySeries {
                metric,
                spans: spans
                    .into_iter()
                    .map(|merge| {
                        merge.map(|merge| Span {
                            min: merge.min,
                            max: merge.max,
                            avg: round(merge.sum / f64::from(merge.count), places(metric)),
                        })
                    })
                    .collect(),
            })
            .collect();
        Ok(Chart {
            from,
            to,
            columns,
            series,
            store: self.store(),
        })
    }

    /// Keeps `days` days, deleting older ones at once.
    pub fn keep(&self, days: u32, now: u64) -> Result<HistoryStore, CallError> {
        if !(1..=KEEP_DAYS_MAX).contains(&days) {
            return Err(bad(format!(
                "history can be kept from 1 to {KEEP_DAYS_MAX} days, not {days}"
            )));
        }
        self.save_settings(days)
            .map_err(|e| CallError::internal(format!("can't save the setting: {e}")))?;
        self.state().keep_days = days;
        self.prune(now)
            .map_err(|e| CallError::internal(format!("can't delete the older days: {e}")))?;
        Ok(self.store())
    }

    /// Deletes all of it; the next reading starts again.
    pub fn clear(&self) -> Result<HistoryStore, CallError> {
        {
            let mut state = self.state();
            state.minute = None;
            state.quarter = None;
        }
        self.delete_all()
            .map_err(|e| CallError::internal(format!("can't delete the history: {e}")))?;
        Ok(self.store())
    }

    /// Saves how many days are kept: a temporary file renamed into place.
    fn save_settings(&self, days: u32) -> io::Result<()> {
        let bytes = serde_json::to_vec(&Settings { keep_days: days }).map_err(io::Error::other)?;
        let path = self.dir.join(SETTINGS_FILE);
        let temporary = path.with_extension("json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)
    }

    fn delete_all(&self) -> io::Result<()> {
        for tier in Tier::ALL {
            for name in self.files(tier)? {
                remove(&self.dir.join(tier.folder()).join(name))?;
            }
        }
        Ok(())
    }
}

/// A column's points so far.
#[derive(Debug, Clone, Copy)]
struct Merge {
    min: f64,
    max: f64,
    sum: f64,
    count: u32,
}

fn bad(msg: impl Into<String>) -> CallError {
    CallError::new(ErrorCode::BadRequest, msg)
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The first readable point's time in a day's file.
fn first_point(path: &Path) -> Option<u64> {
    let file = fs::File::open(path).ok()?;
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .find_map(|line| serde_json::from_str::<Point>(&line).ok())
        .map(|point| point.t)
}

/// Takes readings every 10 seconds until shutdown, writing each finished
/// point and pruning once a day. Read errors are logged, and sampling carries
/// on.
pub async fn run(
    history: Arc<History>,
    host: Arc<dyn Stats>,
    token: CancellationToken,
) -> Result<(), String> {
    let mut ticker = tokio::time::interval(EVERY);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut previous: Option<(StatsReading, Instant)> = None;
    let mut last_error: Option<String> = None;
    let mut pruned: Option<u64> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = token.cancelled() => {
                let history = Arc::clone(&history);
                let _ = tokio::task::spawn_blocking(move || history.save_pending()).await;
                return Ok(());
            }
        }
        let reading = match read(&host).await {
            Ok(reading) => {
                last_error = None;
                reading
            }
            Err(e) => {
                if last_error.as_deref() != Some(e.as_str()) {
                    warn!("can't read host stats for the history: {e}");
                    last_error = Some(e);
                }
                continue;
            }
        };
        let now = Instant::now();
        let earlier = previous.replace((reading.clone(), now));
        let Some((earlier, at)) = earlier.filter(|(_, at)| now.duration_since(*at) <= STALE) else {
            continue;
        };
        let wall = now_ms();
        let sample = reading.since(&earlier, now.duration_since(at), wall);
        let finished = history.add(&sample);
        let today = wall / DAY_MS;
        if finished.is_empty() && pruned == Some(today) {
            continue;
        }
        let prune = pruned != Some(today);
        pruned = Some(today);
        let history = Arc::clone(&history);
        let written = tokio::task::spawn_blocking(move || {
            history.write(&finished)?;
            if prune {
                history.prune(wall)?;
            }
            Ok::<(), io::Error>(())
        })
        .await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("can't write the history: {e}"),
            Err(e) => warn!("the history writer stopped: {e}"),
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

/// What a sample says for each metric, in the charts' units; a metric the
/// machine doesn't report is left out.
#[allow(clippy::cast_precision_loss)]
fn values(sample: &StatsSample) -> Vec<(HistoryMetric, f64)> {
    let ratio = |part: u64, whole: u64| (whole > 0).then(|| part as f64 / whole as f64);
    let most = |values: &mut dyn Iterator<Item = f64>| {
        values.fold(None, |most: Option<f64>, value| {
            Some(most.map_or(value, |most| most.max(value)))
        })
    };
    let memory = &sample.memory;
    let mut values = vec![
        (HistoryMetric::Cpu, Some(sample.cpu.busy)),
        (
            HistoryMetric::Memory,
            ratio(memory.total.saturating_sub(memory.available), memory.total),
        ),
        (
            HistoryMetric::Swap,
            sample
                .swap
                .as_ref()
                .and_then(|swap| ratio(swap.used, swap.total)),
        ),
        (HistoryMetric::Load, Some(sample.cpu.load.one)),
        (
            HistoryMetric::Network,
            sample
                .network
                .as_ref()
                .map(|network| network.received.saturating_add(network.sent) as f64),
        ),
        (
            HistoryMetric::Disk,
            sample
                .disk_io
                .as_ref()
                .map(|disk| disk.read.saturating_add(disk.write) as f64),
        ),
        (
            HistoryMetric::DiskUsed,
            most(
                &mut sample
                    .filesystems
                    .iter()
                    .filter_map(|fs| ratio(fs.used, fs.total)),
            ),
        ),
        (
            HistoryMetric::Temperature,
            most(
                &mut sample
                    .temperatures
                    .iter()
                    .filter(|sensor| sensor.sensor == SensorKind::Cpu)
                    .map(|sensor| sensor.celsius),
            ),
        ),
        (
            HistoryMetric::Gpu,
            most(&mut sample.gpus.iter().filter_map(|gpu| gpu.busy)),
        ),
    ];
    values.retain(|(_, value)| value.is_some_and(f64::is_finite));
    values
        .into_iter()
        .filter_map(|(metric, value)| Some((metric, value?)))
        .collect()
}

/// Decimals kept for a metric: ratios to a hundredth of a percent, rates to
/// the byte.
fn places(metric: HistoryMetric) -> i32 {
    match metric {
        HistoryMetric::Network | HistoryMetric::Disk => 0,
        HistoryMetric::Temperature => 1,
        HistoryMetric::Load => 2,
        _ => 4,
    }
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// A moment as `2026-10-06 14:02 UTC`.
pub fn utc(ms: u64) -> String {
    let minutes = ms / MINUTE_MS;
    format!(
        "{} {:02}:{:02} UTC",
        day_name(ms / DAY_MS),
        minutes / 60 % 24,
        minutes % 60
    )
}

/// A day's name, `2026-10-06`, from days since the Unix epoch in UTC
/// (Howard Hinnant's `civil_from_days`).
fn day_name(day: u64) -> String {
    let z = i64::try_from(day).unwrap_or(i64::MAX / 2) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use cntrl_host::stats::CpuTicks;
    use cntrl_host::stats::fake::FakeStats;
    use cntrl_protocol::stats::{
        CpuStats, DiskIo, Filesystem, GpuStats, LoadAverage, MemoryStats, NetworkIo, SwapStats,
        Temperature,
    };

    use super::*;

    /// 2026-10-06 00:00 UTC.
    const DAY: u64 = 20_732 * DAY_MS;

    fn sample(ts: u64, busy: f64) -> StatsSample {
        StatsSample {
            ts,
            cpu: CpuStats {
                busy,
                load: LoadAverage {
                    one: 0.5,
                    five: 0.4,
                    fifteen: 0.3,
                },
            },
            memory: MemoryStats {
                total: 8_000,
                available: 6_000,
            },
            swap: None,
            disk_io: None,
            network: None,
            filesystems: Vec::new(),
            temperatures: Vec::new(),
            gpus: Vec::new(),
        }
    }

    fn opened() -> (tempfile::TempDir, History) {
        let dir = tempfile::tempdir().expect("temp dir");
        let history = History::open(dir.path());
        (dir, history)
    }

    fn series(chart: &Chart, metric: HistoryMetric) -> &[Option<Span>] {
        &chart
            .series
            .iter()
            .find(|series| series.metric == metric)
            .expect("the metric is there")
            .spans
    }

    #[test]
    fn names_days_in_utc() {
        assert_eq!(day_name(0), "1970-01-01");
        assert_eq!(day_name(11_016), "2000-02-29");
        assert_eq!(day_name(20_088), "2024-12-31");
        assert_eq!(day_name(20_732), "2026-10-06");
        assert_eq!(day_name(47_541), "2100-03-01");
        assert_eq!(
            utc(DAY + 14 * 3_600_000 + 2 * MINUTE_MS + 59_999),
            "2026-10-06 14:02 UTC"
        );
    }

    #[test]
    fn reads_each_metric_in_the_charts_units() {
        let mut full = sample(DAY, 0.25);
        full.swap = Some(SwapStats {
            total: 1_000,
            used: 100,
        });
        full.network = Some(NetworkIo {
            received: 1_000,
            sent: 500,
        });
        full.disk_io = Some(DiskIo {
            read: 4_096,
            write: 0,
        });
        full.filesystems = vec![
            Filesystem {
                mount: "/".to_owned(),
                name: None,
                kind: "ext4".to_owned(),
                total: 100,
                used: 40,
                available: 60,
            },
            Filesystem {
                mount: "/data".to_owned(),
                name: None,
                kind: "xfs".to_owned(),
                total: 100,
                used: 90,
                available: 10,
            },
        ];
        full.temperatures = vec![
            Temperature {
                sensor: SensorKind::Cpu,
                label: "Package".to_owned(),
                celsius: 61.0,
            },
            Temperature {
                sensor: SensorKind::Disk,
                label: "nvme0".to_owned(),
                celsius: 70.0,
            },
        ];
        full.gpus = vec![GpuStats {
            name: "GPU".to_owned(),
            busy: Some(0.3),
            ..GpuStats::default()
        }];
        let read: BTreeMap<_, _> = values(&full).into_iter().collect();
        assert_eq!(read[&HistoryMetric::Cpu], 0.25);
        assert_eq!(read[&HistoryMetric::Memory], 0.25);
        assert_eq!(read[&HistoryMetric::Swap], 0.1);
        assert_eq!(read[&HistoryMetric::Load], 0.5);
        assert_eq!(read[&HistoryMetric::Network], 1_500.0);
        assert_eq!(read[&HistoryMetric::Disk], 4_096.0);
        assert_eq!(read[&HistoryMetric::DiskUsed], 0.9);
        // The CPU's, not the drive's.
        assert_eq!(read[&HistoryMetric::Temperature], 61.0);
        assert_eq!(read[&HistoryMetric::Gpu], 0.3);
        // What a machine doesn't report is left out.
        let bare: BTreeMap<_, _> = values(&sample(DAY, 0.1)).into_iter().collect();
        assert!(!bare.contains_key(&HistoryMetric::Swap));
        assert!(!bare.contains_key(&HistoryMetric::Network));
        assert!(!bare.contains_key(&HistoryMetric::Gpu));
    }

    #[test]
    fn a_minute_becomes_a_point_when_the_next_begins() {
        let (_dir, history) = opened();
        assert!(history.add(&sample(DAY + 5_000, 0.2)).is_empty());
        assert!(history.add(&sample(DAY + 15_000, 0.6)).is_empty());
        let finished = history.add(&sample(DAY + MINUTE_MS + 5_000, 0.4));
        assert_eq!(finished.len(), 1);
        let (tier, point) = &finished[0];
        assert_eq!(*tier, Tier::Minute);
        assert_eq!(point.t, DAY);
        assert_eq!(point.metrics[&HistoryMetric::Cpu], [0.2, 0.6, 0.4]);
        // The quarter hour gathers on.
        let pending = history.pending();
        assert!(pending.iter().any(|(tier, point)| *tier == Tier::Quarter
            && point.metrics[&HistoryMetric::Cpu] == [0.2, 0.6, 0.4]));
        let finished = history.add(&sample(DAY + QUARTER_MS, 0.1));
        assert!(finished.iter().any(|(tier, _)| *tier == Tier::Quarter));
    }

    #[test]
    fn reads_a_span_into_columns_with_the_minute_in_progress() {
        let (_dir, history) = opened();
        let mut finished = Vec::new();
        for minute in 0..10 {
            let busy = f64::from(minute) / 10.0;
            finished.extend(history.add(&sample(DAY + minute as u64 * MINUTE_MS + 5_000, busy)));
        }
        history.write(&finished).expect("written");
        // Ten minutes in two columns of five; minute 9 is still in progress.
        let params = HistoryParams {
            from: DAY,
            to: DAY + 10 * MINUTE_MS,
            columns: 2,
        };
        let chart = history.read(&params, DAY + 10 * MINUTE_MS).expect("read");
        let cpu = series(&chart, HistoryMetric::Cpu);
        assert_eq!(
            cpu[0],
            Some(Span {
                min: 0.0,
                max: 0.4,
                avg: 0.2
            })
        );
        assert_eq!(
            cpu[1],
            Some(Span {
                min: 0.5,
                max: 0.9,
                avg: 0.7
            })
        );
        assert_eq!(chart.store.keep_days, KEEP_DAYS_DEFAULT);
        assert_eq!(chart.store.oldest, Some(DAY));
        assert!(chart.store.bytes > 0);
        // A column with nothing in it is null.
        let wide = HistoryParams {
            from: DAY - 10 * MINUTE_MS,
            to: DAY + 10 * MINUTE_MS,
            columns: 2,
        };
        let chart = history.read(&wide, DAY + 10 * MINUTE_MS).expect("read");
        assert_eq!(series(&chart, HistoryMetric::Cpu)[0], None);
    }

    #[test]
    fn older_spans_come_from_quarter_hours_and_torn_lines_are_skipped() {
        let (dir, history) = opened();
        let point = |t: u64, busy: f64| Point {
            t,
            metrics: BTreeMap::from([(HistoryMetric::Cpu, [busy, busy, busy])]),
        };
        history
            .write(&[
                (Tier::Quarter, point(DAY, 0.1)),
                (Tier::Quarter, point(DAY + QUARTER_MS, 0.3)),
                (Tier::Minute, point(DAY, 0.9)),
            ])
            .expect("written");
        let quarters = dir.path().join("history/15m/2026-10-06.jsonl");
        let mut file = OpenOptions::new()
            .append(true)
            .open(&quarters)
            .expect("open");
        file.write_all(b"{\"t\":17598").expect("a torn line");
        let params = HistoryParams {
            from: DAY,
            to: DAY + DAY_MS,
            columns: 1,
        };
        let chart = history.read(&params, DAY + 30 * DAY_MS).expect("read");
        assert_eq!(
            series(&chart, HistoryMetric::Cpu)[0],
            Some(Span {
                min: 0.1,
                max: 0.3,
                avg: 0.2
            })
        );
        // The same span read the day it happened comes from the minutes.
        let chart = history.read(&params, DAY + 3_600_000).expect("read");
        assert_eq!(
            series(&chart, HistoryMetric::Cpu)[0].map(|span| span.avg),
            Some(0.9)
        );
    }

    #[test]
    fn the_oldest_point_is_the_first_minute_recorded() {
        let (_dir, history) = opened();
        let _ = history.add(&sample(DAY + 3 * MINUTE_MS + 5_000, 0.5));
        assert_eq!(history.store().oldest, Some(DAY + 3 * MINUTE_MS));
        // Saved on a restart, the quarter hour's point is stamped DAY.
        history.save_pending();
        assert_eq!(history.store().oldest, Some(DAY + 3 * MINUTE_MS));
        // Once the minutes are pruned, the quarter hours reach back further.
        let older = Point {
            t: DAY - DAY_MS,
            metrics: BTreeMap::from([(HistoryMetric::Cpu, [0.1, 0.1, 0.1])]),
        };
        history.write(&[(Tier::Quarter, older)]).expect("written");
        assert_eq!(history.store().oldest, Some(DAY - DAY_MS));
    }

    #[test]
    fn refuses_spans_it_cant_answer() {
        let (_dir, history) = opened();
        let read = |from: u64, to: u64, columns: u32| {
            history
                .read(&HistoryParams { from, to, columns }, DAY)
                .map(|_| ())
                .map_err(|e| e.code)
        };
        assert_eq!(read(DAY, DAY, 10), Err(ErrorCode::BadRequest));
        assert_eq!(read(DAY, DAY + 1, 0), Err(ErrorCode::BadRequest));
        assert_eq!(
            read(DAY, DAY + 1, COLUMNS_MAX + 1),
            Err(ErrorCode::BadRequest)
        );
        assert_eq!(read(0, DAY, 10), Err(ErrorCode::BadRequest));
        assert_eq!(read(DAY - DAY_MS, DAY, COLUMNS_MAX), Ok(()));
    }

    #[test]
    fn keeping_fewer_days_deletes_the_older_ones() {
        let (dir, history) = opened();
        let point = |t: u64| Point {
            t,
            metrics: BTreeMap::from([(HistoryMetric::Cpu, [0.1, 0.1, 0.1])]),
        };
        let today = DAY + 40 * DAY_MS;
        history
            .write(&[
                (Tier::Quarter, point(DAY)),
                (Tier::Quarter, point(today - 5 * DAY_MS)),
                (Tier::Minute, point(today - 5 * DAY_MS)),
                (Tier::Minute, point(today - DAY_MS)),
            ])
            .expect("written");
        let store = history.keep(30, today).expect("kept");
        assert_eq!(store.keep_days, 30);
        assert_eq!(store.oldest, Some(today - 5 * DAY_MS));
        assert!(!dir.path().join("history/15m/2026-10-06.jsonl").exists());
        // Minutes keep two days before today's.
        assert!(
            !dir.path()
                .join("history/1m")
                .join(format!("{}.jsonl", day_name(today / DAY_MS - 5)))
                .exists()
        );
        assert!(
            dir.path()
                .join("history/1m")
                .join(format!("{}.jsonl", day_name(today / DAY_MS - 1)))
                .exists()
        );
        // The setting outlives a restart.
        assert_eq!(History::open(dir.path()).store().keep_days, 30);
        let refused = history.keep(0, today).map(|_| ()).map_err(|e| e.code);
        assert_eq!(refused, Err(ErrorCode::BadRequest));
        let refused = history
            .keep(KEEP_DAYS_MAX + 1, today)
            .map(|_| ())
            .map_err(|e| e.code);
        assert_eq!(refused, Err(ErrorCode::BadRequest));
    }

    #[test]
    fn clearing_deletes_everything_and_the_minute_in_progress() {
        let (_dir, history) = opened();
        let finished = [
            history.add(&sample(DAY, 0.5)),
            history.add(&sample(DAY + MINUTE_MS, 0.5)),
        ]
        .concat();
        history.write(&finished).expect("written");
        assert!(history.store().bytes > 0);
        let store = history.clear().expect("cleared");
        assert_eq!(store.bytes, 0);
        assert_eq!(store.oldest, None);
        assert!(history.pending().is_empty());
        assert_eq!(store.keep_days, KEEP_DAYS_DEFAULT);
    }

    #[test]
    fn saving_on_shutdown_keeps_the_minute_so_far() {
        let (dir, history) = opened();
        let _ = history.add(&sample(DAY + 5_000, 0.5));
        history.save_pending();
        assert!(history.pending().is_empty());
        let reopened = History::open(dir.path());
        let params = HistoryParams {
            from: DAY,
            to: DAY + MINUTE_MS,
            columns: 1,
        };
        let chart = reopened.read(&params, DAY + MINUTE_MS).expect("read");
        assert_eq!(
            series(&chart, HistoryMetric::Cpu)[0].map(|span| span.avg),
            Some(0.5)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn samples_every_ten_seconds_watched_or_not() {
        let dir = tempfile::tempdir().expect("temp dir");
        let history = Arc::new(History::open(dir.path()));
        let reading = |busy: u64, idle: u64| StatsReading {
            cpu: CpuTicks { busy, idle },
            memory: MemoryStats {
                total: 8_000,
                available: 6_000,
            },
            ..StatsReading::default()
        };
        let host = Arc::new(FakeStats::new([
            reading(0, 0),
            reading(5, 5),
            reading(10, 10),
            reading(15, 15),
        ]));
        let token = CancellationToken::new();
        let sampler = tokio::spawn(run(
            Arc::clone(&history),
            Arc::clone(&host) as Arc<dyn Stats>,
            token.clone(),
        ));
        tokio::time::sleep(Duration::from_millis(25_000)).await;
        // At 0, 10 and 20 s.
        assert_eq!(host.reads(), 3);
        assert!(
            history
                .pending()
                .iter()
                .any(|(_, point)| point.metrics[&HistoryMetric::Cpu][2] == 0.5)
        );
        token.cancel();
        assert_eq!(sampler.await.expect("the sampler ran"), Ok(()));
        // What was in progress is on disk.
        assert!(History::open(dir.path()).store().bytes > 0);
    }
}
