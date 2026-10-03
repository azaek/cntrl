//! Host stats behind the `stats` topic: CPU, load, memory and swap, disk and
//! network throughput, filesystems and temperatures.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use cntrl_protocol::stats::{
    CpuStats, DiskIo, Filesystem, LoadAverage, MemoryStats, NetworkIo, StatsSample, SwapStats,
    Temperature,
};

use crate::HostError;

/// Reads the counters behind host stats.
pub trait Stats: Send + Sync {
    /// Takes one reading. It blocks on the OS, so call it on a blocking thread.
    fn read(&self) -> Result<StatsReading, HostError>;
}

/// One reading. CPU time and the byte counters only mean something as the
/// difference between two readings, so a sample takes two.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatsReading {
    pub cpu: CpuTicks,
    pub load: LoadAverage,
    pub memory: MemoryStats,
    pub swap: Option<SwapStats>,
    pub disk: Option<DiskCounters>,
    pub network: Option<NetworkCounters>,
    /// The latest filesystems and temperatures, which are read on slower clocks
    /// than the counters.
    pub filesystems: Vec<Filesystem>,
    pub temperatures: Vec<Temperature>,
}

/// CPU time across all cores since boot, in the OS's ticks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTicks {
    pub busy: u64,
    pub idle: u64,
}

/// Bytes read from and written to the physical disks since boot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskCounters {
    pub read: u64,
    pub written: u64,
}

/// Bytes received and sent on the physical network interfaces since boot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkCounters {
    pub received: u64,
    pub sent: u64,
}

impl StatsReading {
    /// The sample for the `elapsed` time between `earlier` and this reading,
    /// stamped `ts` (milliseconds since the Unix epoch).
    pub fn since(&self, earlier: &StatsReading, elapsed: Duration, ts: u64) -> StatsSample {
        StatsSample {
            ts,
            cpu: CpuStats {
                busy: round(busy_share(earlier.cpu, self.cpu), 4),
                load: self.load.clone(),
            },
            memory: self.memory.clone(),
            swap: self.swap.clone(),
            disk_io: earlier.disk.zip(self.disk).map(|(before, after)| DiskIo {
                read: rate(before.read, after.read, elapsed),
                write: rate(before.written, after.written, elapsed),
            }),
            network: earlier
                .network
                .zip(self.network)
                .map(|(before, after)| NetworkIo {
                    received: rate(before.received, after.received, elapsed),
                    sent: rate(before.sent, after.sent, elapsed),
                }),
            filesystems: self.filesystems.clone(),
            temperatures: self.temperatures.clone(),
        }
    }
}

/// Bytes per second between two readings of a counter. One that went back, as
/// when a disk or an interface goes away, counts as nothing moved.
fn rate(before: u64, after: u64, elapsed: Duration) -> u64 {
    let millis = elapsed.as_millis();
    if millis == 0 {
        return 0;
    }
    u64::try_from(u128::from(after.saturating_sub(before)) * 1000 / millis).unwrap_or(u64::MAX)
}

/// A value read on a thread of its own once the last read is `every` old, for
/// reads that can block, such as a filesystem's space on a network mount. A
/// read still running after `STUCK_AFTER` doesn't hold up the next, and its
/// result, should it ever come, is dropped as stale.
#[derive(Debug)]
pub(crate) struct Background<T> {
    every: Duration,
    shared: Arc<Mutex<BackgroundState<T>>>,
}

#[derive(Debug, Default)]
struct BackgroundState<T> {
    value: T,
    read: Option<Instant>,
    /// When the read under way started, and its number.
    running: Option<(Instant, u64)>,
    started: u64,
    stored: u64,
}

const STUCK_AFTER: Duration = Duration::from_secs(60);

impl<T: Clone + Default + Send + 'static> Background<T> {
    pub(crate) fn new(every: Duration) -> Self {
        Self {
            every,
            shared: Arc::default(),
        }
    }

    /// The value as of the last read, starting another when one is due. Until
    /// the first read ends it's the default.
    pub(crate) fn get(&self, read: impl FnOnce() -> T + Send + 'static) -> T {
        let mut state = lock(&self.shared);
        let due = state.read.is_none_or(|at| at.elapsed() >= self.every);
        let free = state
            .running
            .is_none_or(|(since, _)| since.elapsed() >= STUCK_AFTER);
        if due && free {
            let number = state.started + 1;
            let shared = Arc::clone(&self.shared);
            let spawned = thread::Builder::new()
                .name("stats-read".to_owned())
                .spawn(move || {
                    let value = read();
                    let mut state = lock(&shared);
                    if number > state.stored {
                        state.value = value;
                        state.stored = number;
                        state.read = Some(Instant::now());
                    }
                    if state.running.is_some_and(|(_, running)| running == number) {
                        state.running = None;
                    }
                });
            if spawned.is_ok() {
                state.started = number;
                state.running = Some((Instant::now(), number));
            }
        }
        state.value.clone()
    }
}

fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A value read on its own, slower clock: kept until it's `every` old.
#[derive(Debug)]
pub(crate) struct Every<T> {
    every: Duration,
    value: T,
    at: Option<Instant>,
}

impl<T: Clone + Default> Every<T> {
    pub(crate) fn new(every: Duration) -> Self {
        Self {
            every,
            value: T::default(),
            at: None,
        }
    }

    /// The value, read again first when it's due.
    pub(crate) fn get(&mut self, read: impl FnOnce() -> T) -> T {
        if self.at.is_none_or(|at| at.elapsed() >= self.every) {
            self.value = read();
            self.at = Some(Instant::now());
        }
        self.value.clone()
    }
}

/// The busy share of the CPU time between two readings. Counters can step
/// backwards (Linux's iowait does on tickless kernels), so deltas saturate.
fn busy_share(earlier: CpuTicks, now: CpuTicks) -> f64 {
    let busy = now.busy.saturating_sub(earlier.busy);
    let elapsed = busy + now.idle.saturating_sub(earlier.idle);
    if elapsed == 0 {
        return 0.0;
    }
    busy as f64 / elapsed as f64
}

/// Rounds to `places` decimals, so samples don't carry float noise.
pub(crate) fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// The backend for this OS.
pub fn backend() -> Arc<dyn Stats> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(crate::linux::LinuxStats::default())
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(crate::macos::MacStats::default())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Arc::new(crate::Unsupported)
    }
}

impl Stats for crate::Unsupported {
    fn read(&self) -> Result<StatsReading, HostError> {
        Err(HostError::Unsupported)
    }
}

/// A scripted backend for tests.
#[cfg(feature = "test-util")]
pub mod fake {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, PoisonError};

    use super::{HostError, Stats, StatsReading};

    /// Returns its readings in order, then repeats the last one, and counts
    /// how often it was read.
    #[derive(Debug, Default)]
    pub struct FakeStats {
        readings: Mutex<VecDeque<StatsReading>>,
        reads: AtomicUsize,
    }

    impl FakeStats {
        pub fn new(readings: impl IntoIterator<Item = StatsReading>) -> Self {
            Self {
                readings: Mutex::new(readings.into_iter().collect()),
                reads: AtomicUsize::new(0),
            }
        }

        /// How many readings were taken.
        pub fn reads(&self) -> usize {
            self.reads.load(Ordering::SeqCst)
        }
    }

    impl Stats for FakeStats {
        fn read(&self) -> Result<StatsReading, HostError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let mut readings = self.readings.lock().unwrap_or_else(PoisonError::into_inner);
            let reading = match readings.len() {
                0 => return Err(HostError::Failed("no readings scripted".to_owned())),
                1 => readings.front().cloned(),
                _ => readings.pop_front(),
            };
            reading.ok_or_else(|| HostError::Failed("no readings scripted".to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    fn reading(busy: u64, idle: u64) -> StatsReading {
        StatsReading {
            cpu: CpuTicks { busy, idle },
            load: LoadAverage {
                one: 0.53,
                five: 0.27,
                fifteen: 0.1,
            },
            memory: MemoryStats {
                total: 8_000,
                available: 6_000,
            },
            ..StatsReading::default()
        }
    }

    #[test]
    fn busy_is_the_share_of_elapsed_ticks() {
        let sample = reading(2_100, 8_800).since(&reading(1_700, 8_200), SECOND, 42);
        assert_eq!(sample.ts, 42);
        assert_eq!(sample.cpu.busy, 0.4);
        assert_eq!(sample.cpu.load, reading(0, 0).load);
        assert_eq!(sample.memory, reading(0, 0).memory);
    }

    #[test]
    fn busy_is_rounded() {
        let sample = reading(1, 2).since(&reading(0, 0), SECOND, 0);
        assert_eq!(sample.cpu.busy, 0.3333);
    }

    #[test]
    fn busy_is_zero_without_elapsed_ticks() {
        let same = reading(1_700, 8_200);
        assert_eq!(same.since(&same, SECOND, 0).cpu.busy, 0.0);
    }

    #[test]
    fn busy_survives_counters_going_backwards() {
        // Idle stepped back while busy moved on: everything elapsed was busy.
        let sample = reading(1_800, 8_100).since(&reading(1_700, 8_200), SECOND, 0);
        assert_eq!(sample.cpu.busy, 1.0);
    }

    #[test]
    fn throughput_is_bytes_per_second() {
        let earlier = StatsReading {
            disk: Some(DiskCounters {
                read: 1_000,
                written: 5_000,
            }),
            network: Some(NetworkCounters {
                received: 10_000,
                sent: 2_000,
            }),
            ..reading(0, 0)
        };
        let later = StatsReading {
            disk: Some(DiskCounters {
                read: 3_000,
                written: 5_000,
            }),
            // An interface went away, so the total went back.
            network: Some(NetworkCounters {
                received: 13_000,
                sent: 1_000,
            }),
            ..reading(0, 0)
        };
        let sample = later.since(&earlier, Duration::from_millis(500), 0);
        assert_eq!(
            sample.disk_io,
            Some(DiskIo {
                read: 4_000,
                write: 0
            })
        );
        assert_eq!(
            sample.network,
            Some(NetworkIo {
                received: 6_000,
                sent: 0
            })
        );
        // Without an earlier count there's no rate yet.
        assert_eq!(later.since(&reading(0, 0), SECOND, 0).disk_io, None);
        assert_eq!(
            later.since(&earlier, Duration::ZERO, 0).network,
            Some(NetworkIo::default())
        );
    }

    #[test]
    fn a_background_value_arrives_later_and_keeps() {
        let background = Background::<u32>::new(Duration::from_secs(3600));
        // The first call starts the read and has nothing yet.
        assert_eq!(background.get(|| 5), 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while background.get(|| 9) != 5 {
            assert!(Instant::now() < deadline, "the read never landed");
            thread::sleep(Duration::from_millis(5));
        }
        // Not due again for an hour.
        assert_eq!(background.get(|| 9), 5);
    }

    #[test]
    fn a_slow_value_is_kept_until_due() {
        let mut reads = 0;
        let mut every = Every::<u32>::new(Duration::from_secs(3600));
        assert_eq!(
            every.get(|| {
                reads += 1;
                7
            }),
            7
        );
        assert_eq!(
            every.get(|| {
                reads += 1;
                9
            }),
            7
        );
        assert_eq!(reads, 1);
        let mut always = Every::<u32>::new(Duration::ZERO);
        assert_eq!(always.get(|| 1), 1);
        assert_eq!(always.get(|| 2), 2);
    }

    #[test]
    fn unsupported_says_so() {
        assert_eq!(crate::Unsupported.read(), Err(HostError::Unsupported));
    }
}
