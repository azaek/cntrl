//! The macOS backend: host stats and system info through sysinfo, which needs
//! no root for any of it (angle 09), and the machine's identity from `ioreg`,
//! `sysctl` and `system_profiler`.

use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use cntrl_protocol::stats::{
    Filesystem, LoadAverage, MemoryStats, SensorKind, SwapStats, Temperature,
};
use cntrl_protocol::system::{CpuInfo, OsInfo, SystemInfo};
use sysinfo::{
    Components, CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind,
};

use crate::HostError;
use crate::mounts::{Stuck, remote_space};
use crate::stats::{
    Background, CpuTicks, DiskCounters, Every, NetworkCounters, Stats, StatsReading, round,
};
use crate::system::System;

/// How often filesystems and temperatures are looked at while someone watches
/// (angle 09).
const FILESYSTEMS_EVERY: Duration = Duration::from_secs(10);
const TEMPERATURES_EVERY: Duration = Duration::from_secs(5);

/// Network volumes, whose space comes from statvfs with a timeout rather than
/// from sysinfo, since their server can stop answering.
const REMOTE: &[&str] = &[
    "smbfs", "nfs", "afpfs", "webdav", "ftp", "macfuse", "osxfuse",
];

/// A volume group's data volume and the system's helper volumes are mounted
/// here; they share their container's space with `/`.
fn hidden(mount: &str) -> bool {
    mount.starts_with("/System/Volumes/") || mount == "/private/var/vm"
}

/// Host stats from sysinfo. It gives CPU use as a share of the time since its
/// last refresh, not as tick counts, so this keeps running totals in
/// milliseconds instead: each reading adds the time since the one before,
/// split by that share. Differences between readings then work as on Linux.
pub struct MacStats {
    state: Mutex<CpuState>,
    filesystems: Background<Vec<Filesystem>>,
    stuck: Stuck,
}

struct CpuState {
    system: sysinfo::System,
    last: Instant,
    ticks: CpuTicks,
    disks: Disks,
    networks: Networks,
    components: Components,
    temperatures: Every<Vec<Temperature>>,
}

impl Default for MacStats {
    fn default() -> Self {
        let refresh = RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
            .with_memory(MemoryRefreshKind::nothing().with_ram().with_swap());
        Self {
            state: Mutex::new(CpuState {
                system: sysinfo::System::new_with_specifics(refresh),
                last: Instant::now(),
                ticks: CpuTicks { busy: 0, idle: 0 },
                disks: Disks::new(),
                networks: Networks::new(),
                components: Components::new(),
                temperatures: Every::new(TEMPERATURES_EVERY),
            }),
            filesystems: Background::new(FILESYSTEMS_EVERY),
            stuck: Stuck::default(),
        }
    }
}

/// Bytes moved on the disks behind the volumes shown: `/` stands for its
/// whole container, which the hidden volumes share, so their counts would be
/// the same disk's again. getfsstat lists them without waiting, and the counts
/// come from IOKit, so this doesn't block on a network volume.
fn disk_counters(disks: &mut Disks) -> Option<DiskCounters> {
    disks.refresh_specifics(true, DiskRefreshKind::nothing().with_io_usage());
    disks
        .iter()
        .filter(|disk| !hidden(&disk.mount_point().to_string_lossy()))
        .map(|disk| disk.usage())
        .map(|usage| DiskCounters {
            read: usage.total_read_bytes,
            written: usage.total_written_bytes,
        })
        .reduce(|a, b| DiskCounters {
            read: a.read.saturating_add(b.read),
            written: a.written.saturating_add(b.written),
        })
}

/// Bytes moved on the `en` interfaces, the Mac's Ethernet, Wi-Fi and
/// Thunderbolt ports. The rest are tunnels (`utun`), whose traffic crosses an
/// `en` interface too, AirDrop's links (`awdl`, `llw`), bridges and loopback.
fn network_counters(networks: &mut Networks) -> Option<NetworkCounters> {
    networks.refresh(true);
    networks
        .iter()
        .filter(|(name, _)| name.starts_with("en"))
        .map(|(_, data)| NetworkCounters {
            received: data.total_received(),
            sent: data.total_transmitted(),
        })
        .reduce(|a, b| NetworkCounters {
            received: a.received.saturating_add(b.received),
            sent: a.sent.saturating_add(b.sent),
        })
}

/// The CPU's temperature, as the hottest of Apple silicon's die sensors (`PMU
/// tdie…`) or an Intel Mac's CPU sensors, then the SSD's.
fn temperatures(components: &mut Components) -> Vec<Temperature> {
    if components.is_empty() {
        components.refresh(true);
    } else {
        components.refresh(false);
    }
    let readings: Vec<(String, f64)> = components
        .iter()
        .filter_map(|c| Some((c.label().to_owned(), f64::from(c.temperature()?))))
        .filter(|(_, celsius)| (-40.0..125.0).contains(celsius) && *celsius != 0.0)
        .collect();
    let hottest = |wanted: &dyn Fn(&str) -> bool| {
        readings
            .iter()
            .filter(|(label, _)| wanted(label))
            .map(|(_, celsius)| *celsius)
            .reduce(f64::max)
    };
    let cpu = hottest(&|label| {
        label.starts_with("PMU tdie") || label.starts_with("PMU2 tdie") || label.contains("CPU")
    });
    let ssd = hottest(&|label| label.contains("NAND"));
    let reading = |sensor, label: &str, celsius: f64| Temperature {
        sensor,
        label: label.to_owned(),
        celsius: round(celsius, 1),
    };
    cpu.map(|c| reading(SensorKind::Cpu, "CPU", c))
        .into_iter()
        .chain(ssd.map(|c| reading(SensorKind::Disk, "SSD", c)))
        .collect()
}

/// The volumes worth showing and their space: `/` for the startup disk's
/// container and whatever's mounted under `/Volumes`. Local space comes from
/// sysinfo, which counts what macOS can purge as available, as Finder does.
fn filesystems(stuck: &Stuck) -> Vec<Filesystem> {
    let mut disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing());
    disks
        .iter_mut()
        .filter_map(|disk| {
            let mount = disk.mount_point().to_string_lossy().into_owned();
            if hidden(&mount) {
                return None;
            }
            let kind = disk.file_system().to_string_lossy().into_owned();
            let (total, used, available) = if REMOTE.contains(&kind.as_str()) {
                remote_space(stuck, &mount)?
            } else {
                disk.refresh_specifics(DiskRefreshKind::nothing().with_storage());
                let (total, available) = (disk.total_space(), disk.available_space());
                (total, total.saturating_sub(available), available)
            };
            let name = disk.name().to_string_lossy().into_owned();
            (total > 0).then(|| Filesystem {
                mount,
                name: (!name.is_empty()).then_some(name),
                kind,
                total,
                used,
                available,
            })
        })
        .collect()
}

impl Stats for MacStats {
    fn read(&self) -> Result<StatsReading, HostError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let CpuState {
            system,
            last,
            ticks,
            disks,
            networks,
            components,
            temperatures: slow_temperatures,
        } = &mut *state;
        system.refresh_cpu_usage();
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());
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
        let stuck = Arc::clone(&self.stuck);
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
            // macOS grows swap files as it needs them, so there may be none yet.
            swap: (system.total_swap() > 0).then(|| SwapStats {
                total: system.total_swap(),
                used: system.used_swap(),
            }),
            disk: disk_counters(disks),
            network: network_counters(networks),
            filesystems: self.filesystems.get(move || filesystems(&stuck)),
            temperatures: slow_temperatures.get(|| temperatures(components)),
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
            machine: hardware().0.clone(),
            cpu: hardware().1.clone(),
        })
    }
}

/// The model and the CPU, read once: they don't change while the agent runs,
/// and `system_profiler` takes a moment.
fn hardware() -> &'static (Option<String>, Option<CpuInfo>) {
    static HARDWARE: OnceLock<(Option<String>, Option<CpuInfo>)> = OnceLock::new();
    HARDWARE.get_or_init(|| (model(), cpu()))
}

/// The model's name with its identifier, such as `Mac mini (Mac16,10)`, else
/// just the identifier.
fn model() -> Option<String> {
    let identifier = sysctl("hw.model");
    let name = output(
        "/usr/sbin/system_profiler",
        &["SPHardwareDataType", "-json"],
    )
    .and_then(|json| json_string(&json, "machine_name"));
    match (name, identifier) {
        (Some(name), Some(identifier)) => Some(format!("{name} ({identifier})")),
        (name, identifier) => name.or(identifier),
    }
}

/// The CPU from sysctl. Apple silicon has two kinds of core, as perflevel0
/// (performance) and perflevel1 (efficiency); Intel Macs have neither key.
fn cpu() -> Option<CpuInfo> {
    let count = |key: &str| sysctl(key)?.parse::<u32>().ok();
    let threads = count("hw.logicalcpu")?;
    let (performance, efficiency) = match count("hw.nperflevels") {
        Some(2) => (
            count("hw.perflevel0.physicalcpu"),
            count("hw.perflevel1.physicalcpu"),
        ),
        _ => (None, None),
    };
    Some(CpuInfo {
        name: sysctl("machdep.cpu.brand_string"),
        cores: count("hw.physicalcpu").unwrap_or(threads),
        threads,
        performance_cores: performance,
        efficiency_cores: efficiency,
    })
}

fn sysctl(key: &str) -> Option<String> {
    output("/usr/sbin/sysctl", &["-n", key])
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// The first `"key" : "value"` in pretty-printed JSON, as `system_profiler`
/// writes it.
fn json_string(json: &str, key: &str) -> Option<String> {
    let after = &json[json.find(&format!("\"{key}\""))? + key.len() + 2..];
    let value = after
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    Some(value[..value.find('"')?].to_owned()).filter(|v| !v.is_empty())
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
        let stats = MacStats::default();
        let reading = stats.read().expect("a reading");
        assert!(reading.memory.total > 0);
        assert!(reading.memory.available <= reading.memory.total);
        assert!(reading.disk.is_some(), "disk counters");
        let cpu = reading.temperatures.first().expect("a CPU temperature");
        assert_eq!((cpu.sensor, cpu.label.as_str()), (SensorKind::Cpu, "CPU"));
        // Filesystems arrive from the background read.
        let deadline = Instant::now() + Duration::from_secs(10);
        let filesystems = loop {
            let reading = stats.read().expect("a reading");
            if !reading.filesystems.is_empty() {
                break reading.filesystems;
            }
            assert!(Instant::now() < deadline, "no filesystems");
            std::thread::sleep(Duration::from_millis(50));
        };
        let root = filesystems
            .iter()
            .find(|f| f.mount == "/")
            .expect("the startup disk");
        assert_eq!(root.kind, "apfs");
        assert!(root.used + root.available <= root.total + root.total / 100);
        assert!(
            !filesystems
                .iter()
                .any(|f| f.mount.starts_with("/System/Volumes/"))
        );
        let info = MacSystem.info("0.0.0").expect("system info");
        assert_eq!(info.os.id, "macos");
        assert!(info.boot_time > 0);
        let cpu = info.cpu.expect("cpu info");
        assert!(cpu.cores > 0 && cpu.threads >= cpu.cores);
        assert!(info.machine.is_some());
        assert!(machine_id().is_some());
        assert!(boot_id().is_some());
    }

    #[test]
    fn finds_a_string_in_system_profilers_json() {
        let json = r#"{ "SPHardwareDataType" : [ { "chip_type" : "Apple M4", "machine_model" : "Mac16,10", "machine_name" : "Mac mini" } ] }"#;
        assert_eq!(
            json_string(json, "machine_name").as_deref(),
            Some("Mac mini")
        );
        assert_eq!(
            json_string(json, "machine_model").as_deref(),
            Some("Mac16,10")
        );
        assert_eq!(json_string(json, "serial_number"), None);
    }
}
