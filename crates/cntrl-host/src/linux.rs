//! The Linux backend, reading procfs. Parsing is plain Rust, so this builds and
//! its tests run on any OS; [`crate::stats::backend`] picks it only on Linux.

use std::path::PathBuf;

use cntrl_protocol::stats::{LoadAverage, MemoryStats};
use procfs_core::{CpuTime, ExplicitSystemInfo, FromRead, FromReadSI, KernelStats, Meminfo};

use crate::HostError;
use crate::stats::{CpuTicks, Stats, StatsReading, round};

/// Only raw tick counts are read from `/proc/stat`, so the conversions these
/// values drive never run.
const SYSTEM_INFO: ExplicitSystemInfo = ExplicitSystemInfo {
    boot_time_secs: 0,
    ticks_per_second: 100,
    page_size: 4096,
    is_little_endian: cfg!(target_endian = "little"),
};

/// Host stats from `/proc/stat`, `/proc/loadavg` and `/proc/meminfo`.
#[derive(Debug, Clone)]
pub struct LinuxStats {
    proc: PathBuf,
}

impl Default for LinuxStats {
    fn default() -> Self {
        Self::at("/proc")
    }
}

impl LinuxStats {
    /// Reads from a procfs mounted elsewhere, such as a test fixture.
    pub fn at(proc: impl Into<PathBuf>) -> Self {
        Self { proc: proc.into() }
    }
}

impl Stats for LinuxStats {
    fn read(&self) -> Result<StatsReading, HostError> {
        let failed = |e: procfs_core::ProcError| HostError::Failed(e.to_string());
        let stat = KernelStats::from_file(self.proc.join("stat"), &SYSTEM_INFO).map_err(failed)?;
        let load =
            procfs_core::LoadAverage::from_file(self.proc.join("loadavg")).map_err(failed)?;
        let memory = Meminfo::from_file(self.proc.join("meminfo")).map_err(failed)?;
        Ok(StatsReading {
            cpu: cpu_ticks(&stat.total),
            load: LoadAverage {
                one: round(f64::from(load.one), 2),
                five: round(f64::from(load.five), 2),
                fifteen: round(f64::from(load.fifteen), 2),
            },
            memory: MemoryStats {
                total: memory.mem_total,
                // MemAvailable arrived in Linux 3.14. MemFree undercounts, but
                // it's the closest older kernels have.
                available: memory.mem_available.unwrap_or(memory.mem_free),
            },
        })
    }
}

/// Splits `/proc/stat`'s `cpu` line into busy and idle ticks. Idle and iowait
/// count as idle. Steal counts as busy: the guest wanted the CPU and the host
/// gave it elsewhere. guest and guest_nice are already inside user and nice, so
/// they aren't added again.
fn cpu_ticks(t: &CpuTime) -> CpuTicks {
    CpuTicks {
        busy: t.user
            + t.nice
            + t.system
            + t.irq.unwrap_or(0)
            + t.softirq.unwrap_or(0)
            + t.steal.unwrap_or(0),
        idle: t.idle + t.iowait.unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from Docker Desktop's Linux VM, kernel 7.0.
    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/host/proc");

    fn total(stat: &str) -> CpuTicks {
        cpu_ticks(
            &KernelStats::from_read(stat.as_bytes(), &SYSTEM_INFO)
                .expect("a valid /proc/stat")
                .total,
        )
    }

    #[test]
    fn reads_a_real_procfs() {
        let reading = LinuxStats::at(FIXTURE).read().expect("the fixture reads");
        assert_eq!(reading.memory.total, 8_124_516 * 1024);
        assert_eq!(reading.memory.available, 7_425_296 * 1024);
        assert!(reading.cpu.busy > 0 && reading.cpu.idle > 0);
    }

    #[test]
    fn steal_is_busy_and_iowait_is_idle() {
        // user nice system idle iowait irq softirq steal guest guest_nice
        let ticks =
            total("cpu  1000 50 500 8000 200 10 40 100 30 5\nctxt 1\nbtime 1\nprocesses 1\n");
        assert_eq!(ticks.busy, 1000 + 50 + 500 + 10 + 40 + 100);
        assert_eq!(ticks.idle, 8000 + 200);
    }

    #[test]
    fn old_kernels_without_iowait_or_steal_parse() {
        let ticks = total("cpu  1000 50 500 8000\nctxt 1\nbtime 1\nprocesses 1\n");
        assert_eq!(
            ticks,
            CpuTicks {
                busy: 1550,
                idle: 8000
            }
        );
    }

    #[test]
    fn a_missing_procfs_fails() {
        let error = LinuxStats::at("/nonexistent").read().unwrap_err();
        assert!(matches!(error, HostError::Failed(_)));
    }
}
