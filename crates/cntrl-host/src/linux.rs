//! The Linux backend, reading procfs, sysfs and os-release. Parsing is plain
//! Rust, so this builds and its tests run on any OS; the `backend()` functions
//! pick it only on Linux.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use cntrl_protocol::stats::{LoadAverage, MemoryStats, SwapStats, Temperature};
use cntrl_protocol::system::{CpuInfo, OsInfo, SystemInfo};
use procfs_core::net::InterfaceDeviceStatus;
use procfs_core::{
    CpuTime, DiskStat, ExplicitSystemInfo, FromBufRead, FromRead, FromReadSI, KernelStats, Meminfo,
};

use crate::HostError;
use crate::hwmon;
use crate::mounts::LinuxFilesystems;
use crate::stats::{CpuTicks, DiskCounters, Every, NetworkCounters, Stats, StatsReading, round};
use crate::system::System;

/// How often filesystems and temperatures are looked at while someone watches
/// (angle 09).
const FILESYSTEMS_EVERY: Duration = Duration::from_secs(10);
const TEMPERATURES_EVERY: Duration = Duration::from_secs(5);

/// `/proc/diskstats` counts in 512-byte sectors whatever the device's sector
/// size (kernel `admin-guide/iostats`).
const SECTOR: u64 = 512;

/// Only raw tick counts are read from `/proc/stat`, so the conversions these
/// values drive never run.
const SYSTEM_INFO: ExplicitSystemInfo = ExplicitSystemInfo {
    boot_time_secs: 0,
    ticks_per_second: 100,
    page_size: 4096,
    is_little_endian: cfg!(target_endian = "little"),
};

/// Host stats from procfs (`stat`, `loadavg`, `meminfo`, `diskstats`,
/// `net/dev` and the mounts) and sysfs (which disks and interfaces are
/// physical, and hwmon).
#[derive(Debug)]
pub struct LinuxStats {
    root: PathBuf,
    filesystems: LinuxFilesystems,
    temperatures: Mutex<Every<Vec<Temperature>>>,
}

impl Default for LinuxStats {
    fn default() -> Self {
        Self::at("/")
    }
}

impl LinuxStats {
    /// Reads procfs and sysfs under another root, such as a test fixture.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            filesystems: LinuxFilesystems::new(root.join("proc/self/mountinfo"), FILESYSTEMS_EVERY),
            temperatures: Mutex::new(Every::new(TEMPERATURES_EVERY)),
            root,
        }
    }

    /// Bytes moved on physical disks: whole disks with a device behind them,
    /// so partitions, device-mapper, RAID, loop, RAM and zram devices, whose
    /// traffic is some physical disk's too, aren't added twice.
    fn disk_counters(&self) -> Option<DiskCounters> {
        let text = fs::read_to_string(self.root.join("proc/diskstats")).ok()?;
        let block = self.root.join("sys/block");
        let mut counters: Option<DiskCounters> = None;
        for stat in text
            .lines()
            .filter_map(|line| DiskStat::from_line(line).ok())
        {
            // sysfs spells a `/` in a device name as `!`, as in `cciss!c0d0`.
            if !block
                .join(stat.name.replace('/', "!"))
                .join("device")
                .exists()
            {
                continue;
            }
            let total = counters.get_or_insert_default();
            total.read = total
                .read
                .saturating_add(stat.sectors_read.saturating_mul(SECTOR));
            total.written = total
                .written
                .saturating_add(stat.sectors_written.saturating_mul(SECTOR));
        }
        counters
    }

    /// Bytes moved on physical interfaces: those with a device behind them, not
    /// loopback, bridges, VLANs, tunnels or container links, whose traffic
    /// crosses a physical interface too. A container sees only its link, so
    /// without a physical interface every one but `lo` counts.
    fn network_counters(&self) -> Option<NetworkCounters> {
        let file = File::open(self.root.join("proc/net/dev")).ok()?;
        let interfaces = InterfaceDeviceStatus::from_buf_read(BufReader::new(file)).ok()?;
        let net = self.root.join("sys/class/net");
        let physical: Vec<_> = interfaces
            .0
            .values()
            .filter(|interface| net.join(&interface.name).join("device").exists())
            .collect();
        let counted = if physical.is_empty() {
            interfaces
                .0
                .values()
                .filter(|interface| interface.name != "lo")
                .collect()
        } else {
            physical
        };
        (!counted.is_empty()).then(|| NetworkCounters {
            received: counted
                .iter()
                .map(|i| i.recv_bytes)
                .fold(0, u64::saturating_add),
            sent: counted
                .iter()
                .map(|i| i.sent_bytes)
                .fold(0, u64::saturating_add),
        })
    }
}

impl Stats for LinuxStats {
    fn read(&self) -> Result<StatsReading, HostError> {
        let failed = |e: procfs_core::ProcError| HostError::Failed(e.to_string());
        let proc = self.root.join("proc");
        let stat = KernelStats::from_file(proc.join("stat"), &SYSTEM_INFO).map_err(failed)?;
        let load = procfs_core::LoadAverage::from_file(proc.join("loadavg")).map_err(failed)?;
        let memory = Meminfo::from_file(proc.join("meminfo")).map_err(failed)?;
        let temperatures = self
            .temperatures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(|| hwmon::temperatures(&self.root.join("sys")));
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
            swap: (memory.swap_total > 0).then(|| SwapStats {
                total: memory.swap_total,
                used: memory.swap_total.saturating_sub(memory.swap_free),
            }),
            disk: self.disk_counters(),
            network: self.network_counters(),
            filesystems: self.filesystems.get(),
            temperatures,
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
            machine: machine(&self.root),
            cpu: cpu_info(&self.root.join("proc/cpuinfo")),
        })
    }
}

/// What firmware puts in DMI when the maker didn't fill it in.
const PLACEHOLDERS: &[&str] = &[
    "To Be Filled By O.E.M.",
    "System Product Name",
    "System manufacturer",
    "Default string",
    "Not Applicable",
    "Not Specified",
    "None",
    "O.E.M.",
    "OEM",
];

/// The machine: an ARM board's device-tree model, else the vendor and product
/// names firmware gives through DMI, which any user can read.
fn machine(root: &Path) -> Option<String> {
    let model = fs::read_to_string(root.join("proc/device-tree/model"))
        .ok()
        .map(|model| model.trim_end_matches('\0').trim().to_owned())
        .filter(|model| !model.is_empty());
    if model.is_some() {
        return model;
    }
    let field = |name: &str| {
        fs::read_to_string(root.join("sys/class/dmi/id").join(name))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| {
                !value.is_empty() && !PLACEHOLDERS.iter().any(|p| value.eq_ignore_ascii_case(p))
            })
    };
    match (field("sys_vendor"), field("product_name")) {
        (Some(vendor), Some(product)) if product.starts_with(&vendor) => Some(product),
        (Some(vendor), Some(product)) => Some(format!("{vendor} {product}")),
        (None, product) => product,
        (Some(_), None) => None,
    }
}

/// The CPU from `/proc/cpuinfo`: x86 names its model; cores are the distinct
/// physical and core ID pairs, and where those are missing, as on ARM, each
/// CPU counts as a core.
fn cpu_info(path: &Path) -> Option<CpuInfo> {
    let file = File::open(path).ok()?;
    let info = procfs_core::CpuInfo::from_buf_read(BufReader::new(file)).ok()?;
    let threads = u32::try_from(info.num_cores()).ok().filter(|n| *n > 0)?;
    let cores: HashSet<(Option<&str>, &str)> = (0..info.num_cores())
        .filter_map(|cpu| {
            Some((
                info.get_field(cpu, "physical id"),
                info.get_field(cpu, "core id")?,
            ))
        })
        .collect();
    Some(CpuInfo {
        name: info
            .model_name(0)
            .map(|name| name.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|name| !name.is_empty()),
        cores: u32::try_from(cores.len())
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(threads),
        threads,
        performance_cores: None,
        efficiency_cores: None,
    })
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
    /// kernel 7.0, on an M4.
    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/host");

    fn total(stat: &str) -> CpuTicks {
        cpu_ticks(
            &KernelStats::from_read(stat.as_bytes(), &SYSTEM_INFO)
                .expect("a valid /proc/stat")
                .total,
        )
    }

    #[test]
    fn reads_a_real_procfs() {
        let reading = LinuxStats::at(ROOT).read().expect("the fixture reads");
        assert_eq!(reading.memory.total, 8_124_516 * 1024);
        assert_eq!(reading.memory.available, 7_425_296 * 1024);
        assert!(reading.cpu.busy > 0 && reading.cpu.idle > 0);
        // Only vda and vdb have a device behind them; loop, ram and nbd don't.
        let disk = reading.disk.expect("disk counters");
        assert!(disk.read > 0 && disk.written > 0);
        // The container's eth0 is a veth, so everything but lo counts.
        assert_eq!(
            reading.network,
            Some(NetworkCounters {
                received: 1_716_630,
                sent: 7_159_818
            })
        );
        // No hwmon or thermal zones in Docker Desktop's VM.
        assert_eq!(reading.temperatures, Vec::new());
    }

    #[test]
    fn counts_physical_disks_and_interfaces() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            fs::write(path, text).expect("write");
        };
        write(
            "proc/diskstats",
            concat!(
                " 259 0 nvme0n1 100 0 2000 50 300 0 4000 60 0 0 0 0 0 0 0 0 0\n",
                " 259 1 nvme0n1p1 90 0 1800 40 280 0 3800 50 0 0 0 0 0 0 0 0 0\n",
                " 253 0 dm-0 90 0 1800 40 280 0 3800 50 0 0 0 0 0 0 0 0 0\n",
                "   8 0 sda 10 0 100 5 20 0 200 6 0 0 0 0 0 0 0 0 0\n",
            ),
        );
        write("sys/block/nvme0n1/device/uevent", "");
        write("sys/block/sda/device/uevent", "");
        write(
            "proc/net/dev",
            concat!(
                "Inter-|   Receive                                                |  Transmit\n",
                " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
                "    lo:  500 5 0 0 0 0 0 0  500 5 0 0 0 0 0 0\n",
                "  eno1: 9000 9 0 0 0 0 0 0 4000 4 0 0 0 0 0 0\n",
                "docker0: 7000 7 0 0 0 0 0 0 3000 3 0 0 0 0 0 0\n",
                "   wg0: 6000 6 0 0 0 0 0 0 2000 2 0 0 0 0 0 0\n",
            ),
        );
        write("sys/class/net/eno1/device/uevent", "");
        let stats = LinuxStats::at(root);
        assert_eq!(
            stats.disk_counters(),
            Some(DiskCounters {
                read: (2000 + 100) * 512,
                written: (4000 + 200) * 512
            })
        );
        assert_eq!(
            stats.network_counters(),
            Some(NetworkCounters {
                received: 9000,
                sent: 4000
            })
        );
    }

    #[test]
    fn describes_a_machine_and_its_cpu() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        fs::create_dir_all(root.join("sys/class/dmi/id")).expect("mkdir");
        fs::write(root.join("sys/class/dmi/id/sys_vendor"), "Dell Inc.\n").expect("write");
        fs::write(
            root.join("sys/class/dmi/id/product_name"),
            "OptiPlex 7090\n",
        )
        .expect("write");
        assert_eq!(machine(root).as_deref(), Some("Dell Inc. OptiPlex 7090"));
        fs::write(
            root.join("sys/class/dmi/id/product_name"),
            "To be filled by O.E.M.\n",
        )
        .expect("write");
        assert_eq!(machine(root), None);
        fs::create_dir_all(root.join("proc/device-tree")).expect("mkdir");
        fs::write(
            root.join("proc/device-tree/model"),
            "Raspberry Pi 4 Model B Rev 1.4\0",
        )
        .expect("write");
        assert_eq!(
            machine(root).as_deref(),
            Some("Raspberry Pi 4 Model B Rev 1.4")
        );

        // Two cores with two threads each.
        let cpuinfo = root.join("cpuinfo");
        let cpu = |n: u32, core: u32| {
            format!(
                "processor\t: {n}\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Core(TM)  i5-7200U CPU @ 2.50GHz\nphysical id\t: 0\ncore id\t\t: {core}\n\n"
            )
        };
        fs::write(
            &cpuinfo,
            [cpu(0, 0), cpu(1, 1), cpu(2, 0), cpu(3, 1)].concat(),
        )
        .expect("write");
        let info = cpu_info(&cpuinfo).expect("cpu info");
        assert_eq!(
            info.name.as_deref(),
            Some("Intel(R) Core(TM) i5-7200U CPU @ 2.50GHz")
        );
        assert_eq!((info.cores, info.threads), (2, 4));
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
        // An M4 under Linux: ARM's cpuinfo names no model, and the VM no machine.
        let cpu = info.cpu.as_ref().expect("cpu info");
        assert_eq!(
            (cpu.name.as_deref(), cpu.cores, cpu.threads),
            (None, 10, 10)
        );
        assert_eq!(info.machine, None);
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
