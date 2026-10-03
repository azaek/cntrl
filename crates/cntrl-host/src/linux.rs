//! The Linux backend, reading procfs and os-release. Parsing is plain Rust, so
//! this builds and its tests run on any OS; the `backend()` functions pick it
//! only on Linux.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use cntrl_protocol::stats::{LoadAverage, MemoryStats};
use cntrl_protocol::system::{OsInfo, SystemInfo};
use procfs_core::{CpuTime, ExplicitSystemInfo, FromRead, FromReadSI, KernelStats, Meminfo};

use crate::HostError;
use crate::stats::{CpuTicks, Stats, StatsReading, round};
use crate::system::System;

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

/// The machine from procfs and os-release(5), under a root directory.
#[derive(Debug, Clone)]
pub struct LinuxSystem {
    root: PathBuf,
}

impl Default for LinuxSystem {
    fn default() -> Self {
        Self::at("/")
    }
}

impl LinuxSystem {
    /// Reads from another root, such as a test fixture.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn read(&self, path: &str) -> Result<String, HostError> {
        fs::read_to_string(self.root.join(path))
            .map(|text| text.trim().to_owned())
            .map_err(|e| HostError::Failed(format!("can't read /{path}: {e}")))
    }
}

impl System for LinuxSystem {
    fn info(&self, agent_version: &str) -> Result<SystemInfo, HostError> {
        let stat = KernelStats::from_file(self.root.join("proc/stat"), &SYSTEM_INFO)
            .map_err(|e| HostError::Failed(e.to_string()))?;
        // os-release(5): /etc first, then the vendor's copy.
        let os_release = self
            .read("etc/os-release")
            .or_else(|_| self.read("usr/lib/os-release"))?;
        Ok(SystemInfo {
            hostname: self.read("proc/sys/kernel/hostname")?,
            os: os_info(&os_release),
            arch: std::env::consts::ARCH.to_owned(),
            kernel: self.read("proc/sys/kernel/osrelease")?,
            boot_time: stat.btime,
            agent_version: agent_version.to_owned(),
        })
    }
}

/// Reads os-release(5): `KEY=value` lines, with values optionally quoted shell
/// style. Missing keys take the defaults the man page gives.
fn os_info(text: &str) -> OsInfo {
    let mut fields: HashMap<&str, String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), unquote(value.trim())))
        .collect();
    OsInfo {
        id: fields.remove("ID").unwrap_or_else(|| "linux".to_owned()),
        name: fields
            .remove("PRETTY_NAME")
            .unwrap_or_else(|| "Linux".to_owned()),
        version: fields.remove("VERSION_ID"),
    }
}

/// Strips matching quotes; inside double quotes, a backslash escapes the next
/// character.
fn unquote(value: &str) -> String {
    let quoted = |q: char| value.len() >= 2 && value.starts_with(q) && value.ends_with(q);
    if quoted('\'') {
        return value[1..value.len() - 1].to_owned();
    }
    if !quoted('"') {
        return value.to_owned();
    }
    let mut out = String::new();
    let mut chars = value[1..value.len() - 1].chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' {
            chars.next().unwrap_or(c)
        } else {
            c
        });
    }
    out
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

    /// Captured from a Debian 13 container in Docker Desktop's Linux VM,
    /// kernel 7.0.
    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/host");
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
    fn describes_a_real_machine() {
        let info = LinuxSystem::at(ROOT)
            .info("1.2.3")
            .expect("the fixture reads");
        assert_eq!(info.hostname, "cntrl-mac-docker");
        assert_eq!(info.kernel, "7.0.14-linuxkit");
        assert_eq!(info.boot_time, 1_791_017_471);
        assert_eq!(info.os.id, "debian");
        assert_eq!(info.os.name, "Debian GNU/Linux 13 (trixie)");
        assert_eq!(info.os.version.as_deref(), Some("13"));
        assert_eq!(info.agent_version, "1.2.3");
    }

    #[test]
    fn os_release_quoting_and_defaults() {
        let arch = os_info("NAME=\"Arch Linux\"\nPRETTY_NAME='Arch Linux'\nID=arch\n# comment\n");
        assert_eq!(arch.id, "arch");
        assert_eq!(arch.name, "Arch Linux");
        assert_eq!(arch.version, None);
        let escaped = os_info(r#"PRETTY_NAME="Say \"hi\" \\o/""#);
        assert_eq!(escaped.name, r#"Say "hi" \o/"#);
        let empty = os_info("");
        assert_eq!((empty.id.as_str(), empty.name.as_str()), ("linux", "Linux"));
    }

    #[test]
    fn a_missing_procfs_fails() {
        let error = LinuxStats::at("/nonexistent").read().unwrap_err();
        assert!(matches!(error, HostError::Failed(_)));
    }
}
