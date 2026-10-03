//! The macOS backend: host stats and system info through sysinfo, and the
//! machine's identity from `ioreg` and `sysctl`.

use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use cntrl_protocol::stats::{LoadAverage, MemoryStats};
use cntrl_protocol::system::{OsInfo, SystemInfo};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind};

use crate::HostError;
use crate::stats::{CpuTicks, Stats, StatsReading, round};
use crate::system::System;

/// Host stats from sysinfo. It gives CPU use as a share of the time since its
/// last refresh, not as tick counts, so this keeps running totals in
/// milliseconds instead: each reading adds the time since the one before,
/// split by that share. Differences between readings then work as on Linux.
pub struct MacStats {
    state: Mutex<CpuState>,
}

struct CpuState {
    system: sysinfo::System,
    last: Instant,
    ticks: CpuTicks,
}

impl Default for MacStats {
    fn default() -> Self {
        let refresh = RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
            .with_memory(MemoryRefreshKind::nothing().with_ram());
        Self {
            state: Mutex::new(CpuState {
                system: sysinfo::System::new_with_specifics(refresh),
                last: Instant::now(),
                ticks: CpuTicks { busy: 0, idle: 0 },
            }),
        }
    }
}

impl Stats for MacStats {
    fn read(&self) -> Result<StatsReading, HostError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let CpuState {
            system,
            last,
            ticks,
        } = &mut *state;
        system.refresh_cpu_usage();
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let now = Instant::now();
        let elapsed = u64::try_from(now.duration_since(*last).as_millis()).unwrap_or(u64::MAX);
        *last = now;
        let share = (f64::from(system.global_cpu_usage()) / 100.0).clamp(0.0, 1.0);
        let busy = (elapsed as f64 * share).round() as u64;
        ticks.busy = ticks.busy.saturating_add(busy);
        ticks.idle = ticks.idle.saturating_add(elapsed.saturating_sub(busy));
        let total = system.total_memory();
        if total == 0 {
            return Err(HostError::Failed("macOS reported no memory".to_owned()));
        }
        let load = sysinfo::System::load_average();
        Ok(StatsReading {
            cpu: *ticks,
            load: LoadAverage {
                one: round(load.one, 2),
                five: round(load.five, 2),
                fifteen: round(load.fifteen, 2),
            },
            memory: MemoryStats {
                total,
                // What Activity Monitor calls Memory Used (app memory, wired
                // and compressed) is in use; the rest, file cache included,
                // is available. sysinfo's own figure counts active pages as
                // available, which would show a busy Mac as nearly empty.
                available: total.saturating_sub(system.used_memory()),
            },
        })
    }
}

/// The machine from sysinfo.
#[derive(Debug, Default)]
pub struct MacSystem;

impl System for MacSystem {
    fn info(&self, agent_version: &str) -> Result<SystemInfo, HostError> {
        Ok(SystemInfo {
            hostname: hostname().unwrap_or_else(|| "unknown".to_owned()),
            os: OsInfo {
                id: "macos".to_owned(),
                // Such as "macOS 26.0 Tahoe".
                name: sysinfo::System::long_os_version().unwrap_or_else(|| "macOS".to_owned()),
                version: sysinfo::System::os_version(),
            },
            arch: std::env::consts::ARCH.to_owned(),
            // The Darwin release, as `uname -r` prints it.
            kernel: sysinfo::System::kernel_version().unwrap_or_default(),
            boot_time: sysinfo::System::boot_time(),
            agent_version: agent_version.to_owned(),
        })
    }
}

/// The hostname without the `.local` that macOS adds for Bonjour.
pub fn hostname() -> Option<String> {
    let name = sysinfo::System::host_name()?;
    let name = name.strip_suffix(".local").unwrap_or(&name).to_owned();
    (!name.is_empty()).then_some(name)
}

/// The hardware UUID, which survives reinstalls of macOS.
pub fn machine_id() -> Option<String> {
    let listing = output("/usr/sbin/ioreg", &["-rd1", "-c", "IOPlatformExpertDevice"])?;
    platform_uuid(&listing)
}

/// macOS's ID for the current boot.
pub fn boot_id() -> Option<String> {
    output("/usr/sbin/sysctl", &["-n", "kern.bootsessionuuid"])
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// Finds `"IOPlatformUUID" = "…"` in `ioreg`'s listing.
fn platform_uuid(listing: &str) -> Option<String> {
    listing
        .lines()
        .find(|line| line.contains("\"IOPlatformUUID\""))
        .and_then(|line| line.split('"').nth(3))
        .map(str::to_owned)
        .filter(|uuid| !uuid.is_empty())
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_platform_uuid() {
        let listing = r#"+-o Mac14,3  <class IOPlatformExpertDevice, id 0x100000110>
    {
      "IOPlatformSerialNumber" = "XYZ123"
      "IOPlatformUUID" = "4C1A2B3D-5E6F-7081-92A3-B4C5D6E7F809"
    }"#;
        assert_eq!(
            platform_uuid(listing).as_deref(),
            Some("4C1A2B3D-5E6F-7081-92A3-B4C5D6E7F809")
        );
        assert_eq!(platform_uuid("nothing here"), None);
    }

    #[test]
    fn reads_this_mac() {
        let reading = MacStats::default().read().expect("a reading");
        assert!(reading.memory.total > 0);
        assert!(reading.memory.available <= reading.memory.total);
        let info = MacSystem.info("0.0.0").expect("system info");
        assert_eq!(info.os.id, "macos");
        assert!(info.boot_time > 0);
        assert!(machine_id().is_some());
        assert!(boot_id().is_some());
    }
}
