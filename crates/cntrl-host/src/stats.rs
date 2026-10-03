//! Host stats behind the `stats` topic: CPU, load and memory.

use std::sync::Arc;

use cntrl_protocol::stats::{CpuStats, LoadAverage, MemoryStats, StatsSample};

use crate::HostError;

/// Reads the counters behind host stats.
pub trait Stats: Send + Sync {
    /// Takes one reading. It blocks on the OS, so call it on a blocking thread.
    fn read(&self) -> Result<StatsReading, HostError>;
}

/// One reading. CPU time only means something as the difference between two
/// readings, so a sample takes two.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsReading {
    pub cpu: CpuTicks,
    pub load: LoadAverage,
    pub memory: MemoryStats,
}

/// CPU time across all cores since boot, in the OS's ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTicks {
    pub busy: u64,
    pub idle: u64,
}

impl StatsReading {
    /// The sample for the time between `earlier` and this reading, stamped
    /// `ts` (milliseconds since the Unix epoch).
    pub fn since(&self, earlier: &StatsReading, ts: u64) -> StatsSample {
        StatsSample {
            ts,
            cpu: CpuStats {
                busy: round(busy_share(earlier.cpu, self.cpu), 4),
                load: self.load.clone(),
            },
            memory: self.memory.clone(),
        }
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
    #[cfg(not(target_os = "linux"))]
    {
        Arc::new(Unsupported)
    }
}

/// The backend for an OS that has none yet.
#[derive(Debug, Default)]
pub struct Unsupported;

impl Stats for Unsupported {
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
        }
    }

    #[test]
    fn busy_is_the_share_of_elapsed_ticks() {
        let sample = reading(2_100, 8_800).since(&reading(1_700, 8_200), 42);
        assert_eq!(sample.ts, 42);
        assert_eq!(sample.cpu.busy, 0.4);
        assert_eq!(sample.cpu.load, reading(0, 0).load);
        assert_eq!(sample.memory, reading(0, 0).memory);
    }

    #[test]
    fn busy_is_rounded() {
        let sample = reading(1, 2).since(&reading(0, 0), 0);
        assert_eq!(sample.cpu.busy, 0.3333);
    }

    #[test]
    fn busy_is_zero_without_elapsed_ticks() {
        let same = reading(1_700, 8_200);
        assert_eq!(same.since(&same, 0).cpu.busy, 0.0);
    }

    #[test]
    fn busy_survives_counters_going_backwards() {
        // Idle stepped back while busy moved on: everything elapsed was busy.
        let sample = reading(1_800, 8_100).since(&reading(1_700, 8_200), 0);
        assert_eq!(sample.cpu.busy, 1.0);
    }

    #[test]
    fn unsupported_says_so() {
        assert_eq!(Unsupported.read(), Err(HostError::Unsupported));
    }
}
