//! What the Storage tab shows (angle 13): the physical disks with their I/O
//! counters, the volumes with their space and inodes, and what each disk says
//! of its health. On Linux the disks come from sysfs and `/proc/diskstats`
//! (kernel `admin-guide/iostats`), and health from smartctl, which privd runs
//! since it needs the raw device. On a Mac they come from `diskutil -plist`
//! and ioreg's block storage statistics, with health from diskutil, none of
//! which needs root. On Windows, storage queries that any account may make
//! (`crate::windows::storage`).

use std::time::Duration;

#[cfg(target_os = "linux")]
use cntrl_protocol::storage::DisksHealth;
use cntrl_protocol::storage::{Disk, DiskHealth, DiskKind, HealthStatus, Volume};
use serde_json::Value;

/// One physical disk and its counters since boot.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskReading {
    pub name: String,
    pub model: Option<String>,
    pub size: u64,
    pub kind: DiskKind,
    pub external: bool,
    pub counters: IoCounters,
}

/// A disk's I/O since boot. What the OS doesn't count is absent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoCounters {
    /// Bytes.
    pub read: u64,
    pub written: u64,
    /// Requests completed.
    pub reads: Option<u64>,
    pub writes: Option<u64>,
    /// Milliseconds the completed requests took, queueing included, reads and
    /// writes together.
    pub time_ms: Option<u64>,
    /// Milliseconds with requests in progress.
    pub busy_ms: Option<u64>,
}

/// A disk as the `storage` topic sends it: its counters turned into rates over
/// `elapsed` since `before`, or zero rates when there's no earlier reading.
pub fn disk_sample(reading: &DiskReading, before: Option<&IoCounters>, elapsed: Duration) -> Disk {
    let now = &reading.counters;
    let seconds = elapsed.as_secs_f64();
    let usable = before.filter(|_| seconds > 0.0);
    let per_second = |after: u64, before: u64| {
        // A counter that went back, as after a disk is replaced, says nothing.
        let delta = after.saturating_sub(before);
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let rate = (delta as f64 / seconds).round() as u64;
        rate
    };
    let delta = |after: Option<u64>, before: Option<u64>| Some(after?.saturating_sub(before?));
    let (read, written, reads, writes, wait_ms, busy) = match usable {
        None => (0, 0, None, None, None, None),
        Some(before) => {
            let reads = delta(now.reads, before.reads);
            let writes = delta(now.writes, before.writes);
            let done = reads.zip(writes).map(|(r, w)| r.saturating_add(w));
            let time = delta(now.time_ms, before.time_ms);
            #[allow(clippy::cast_precision_loss)]
            let wait_ms = done
                .zip(time)
                .filter(|(done, _)| *done > 0)
                .map(|(done, time)| time as f64 / done as f64);
            #[allow(clippy::cast_precision_loss)]
            let busy = delta(now.busy_ms, before.busy_ms)
                .map(|ms| (ms as f64 / (seconds * 1000.0)).clamp(0.0, 1.0));
            #[allow(clippy::cast_precision_loss)]
            let rate = |count: Option<u64>| count.map(|count| count as f64 / seconds);
            (
                per_second(now.read, before.read),
                per_second(now.written, before.written),
                rate(reads),
                rate(writes),
                wait_ms,
                busy,
            )
        }
    };
    Disk {
        name: reading.name.clone(),
        model: reading.model.clone(),
        size: reading.size,
        kind: reading.kind,
        external: reading.external,
        read,
        written,
        reads,
        writes,
        wait_ms,
        busy,
    }
}

/// Reads disks and volumes; each call blocks on the OS, so call it on a
/// blocking thread.
pub trait Storage: Send + Sync {
    fn disks(&self) -> Vec<DiskReading>;
    /// The volumes as of the last look, which runs on a slower clock.
    fn volumes(&self) -> Vec<Volume>;
}

/// This OS's storage reader.
pub fn backend() -> std::sync::Arc<dyn Storage> {
    #[cfg(target_os = "linux")]
    {
        std::sync::Arc::new(linux::LinuxStorage::default())
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(mac::MacStorage::default())
    }
    #[cfg(windows)]
    {
        std::sync::Arc::new(crate::windows::storage::WinStorage)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        std::sync::Arc::new(crate::Unsupported)
    }
}

impl Storage for crate::Unsupported {
    fn disks(&self) -> Vec<DiskReading> {
        Vec::new()
    }

    fn volumes(&self) -> Vec<Volume> {
        Vec::new()
    }
}

/// How a hypervisor's disks name their maker or model.
pub(crate) const VIRTUAL_MAKERS: &[&str] = &[
    "QEMU",
    "VMware",
    "VBOX",
    "Msft",
    "Virtual",
    "Xen",
    "Google",
    "Amazon EC2",
];

pub mod linux {
    //! Disks from sysfs and `/proc/diskstats`, volumes from mountinfo.

    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use cntrl_protocol::storage::{DiskKind, Volume};
    use procfs_core::DiskStats;
    use procfs_core::FromBufRead;

    use super::{DiskReading, IoCounters, Storage, VIRTUAL_MAKERS};
    use crate::mounts::{self, Stuck};
    use crate::stats::Background;

    /// `/proc/diskstats` counts in 512-byte sectors whatever the device's
    /// sector size, and sysfs gives sizes in the same unit.
    const SECTOR: u64 = 512;
    /// How often the volumes are looked at again.
    const VOLUMES_EVERY: Duration = Duration::from_secs(10);

    #[derive(Debug)]
    pub struct LinuxStorage {
        root: PathBuf,
        stuck: Stuck,
        volumes: Background<Vec<Volume>>,
    }

    impl Default for LinuxStorage {
        fn default() -> Self {
            Self::at("/")
        }
    }

    impl LinuxStorage {
        /// Reads procfs and sysfs under another root, such as a test fixture.
        pub fn at(root: impl Into<PathBuf>) -> Self {
            Self {
                root: root.into(),
                stuck: Stuck::default(),
                volumes: Background::new(VOLUMES_EVERY),
            }
        }
    }

    impl Storage for LinuxStorage {
        fn disks(&self) -> Vec<DiskReading> {
            disks(&self.root)
        }

        fn volumes(&self) -> Vec<Volume> {
            let (root, stuck) = (self.root.clone(), std::sync::Arc::clone(&self.stuck));
            self.volumes.get(move || volumes(&root, &stuck))
        }
    }

    /// The physical disks, the whole disks with a device behind them, not
    /// partitions, device-mapper, RAID, loop, RAM or zram devices.
    pub fn disks(root: &Path) -> Vec<DiskReading> {
        let Ok(text) = fs::read_to_string(root.join("proc/diskstats")) else {
            return Vec::new();
        };
        let Ok(stats) = DiskStats::from_buf_read(text.as_bytes()) else {
            return Vec::new();
        };
        let block = root.join("sys/block");
        let mut disks: Vec<DiskReading> = stats
            .0
            .into_iter()
            .filter_map(|stat| {
                // sysfs spells a `/` in a device name as `!`, as in `cciss!c0d0`.
                let dir = block.join(stat.name.replace('/', "!"));
                if !dir.join("device").exists() {
                    return None;
                }
                let model = read_trimmed(&dir.join("device/model"));
                let vendor = read_trimmed(&dir.join("device/vendor"));
                let size = read_number(&dir.join("size"))
                    .unwrap_or(0)
                    .saturating_mul(SECTOR);
                let (kind, external) =
                    kind(root, &dir, &stat.name, model.as_deref(), vendor.as_deref());
                Some(DiskReading {
                    name: stat.name,
                    model,
                    size,
                    kind,
                    external,
                    counters: IoCounters {
                        read: stat.sectors_read.saturating_mul(SECTOR),
                        written: stat.sectors_written.saturating_mul(SECTOR),
                        reads: Some(stat.reads),
                        writes: Some(stat.writes),
                        time_ms: Some(stat.time_reading.saturating_add(stat.time_writing)),
                        busy_ms: Some(stat.time_in_progress),
                    },
                })
            })
            .collect();
        disks.sort_by(|a, b| a.name.cmp(&b.name));
        disks
    }

    /// What kind of disk it is, and whether it's plugged in: a hypervisor's
    /// first, since virtio disks call themselves rotational; then NVMe; then
    /// by rotation. USB or removable media count as external.
    fn kind(
        root: &Path,
        dir: &Path,
        name: &str,
        model: Option<&str>,
        vendor: Option<&str>,
    ) -> (DiskKind, bool) {
        let device = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let usb = device
            .strip_prefix(root)
            .unwrap_or(&device)
            .components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with("usb"));
        let external = usb || read_number(&dir.join("removable")) == Some(1);
        let made_virtual = [model, vendor]
            .into_iter()
            .flatten()
            .any(|text| VIRTUAL_MAKERS.iter().any(|maker| text.contains(maker)));
        let kind = if name.starts_with("vd") || name.starts_with("xvd") || made_virtual {
            DiskKind::Virtual
        } else if name.starts_with("nvme") {
            DiskKind::Nvme
        } else {
            match read_number(&dir.join("queue/rotational")) {
                Some(1) => DiskKind::Hdd,
                Some(0) => DiskKind::Ssd,
                _ => DiskKind::Unknown,
            }
        };
        (kind, external)
    }

    /// The volumes worth showing, as the stats topic picks them, with their
    /// inodes, whether they're read-only, and the disk each is on.
    pub fn volumes(root: &Path, stuck: &Stuck) -> Vec<Volume> {
        // Without the machine's view, nothing counts as read-only rather than
        // everything.
        let read_only = mounts::read_only(&root.join("proc/1/mountinfo")).unwrap_or_default();
        mounts::measure(&root.join("proc/self/mountinfo"), stuck)
            .into_iter()
            .map(|(mount, space)| Volume {
                read_only: read_only.contains(&mount.mount),
                disk: disk_of(root, &mount.device),
                network: mount.remote && !mount.kind.starts_with("fuse."),
                mount: mount.mount,
                name: None,
                kind: mount.kind,
                source: mount.source,
                total: space.total,
                used: space.used,
                available: space.available,
                inodes: space.inodes,
                inodes_used: space.inodes_used,
            })
            .collect()
    }

    /// The physical disk under a device, by `major:minor`: a partition's disk,
    /// or through device-mapper and RAID, their one disk, if there's one.
    pub fn disk_of(root: &Path, device: &str) -> Option<String> {
        let found = disks_under(root, &root.join("sys/dev/block").join(device), 0);
        (found.len() == 1)
            .then(|| found.into_iter().next())
            .flatten()
    }

    fn disks_under(root: &Path, link: &Path, depth: u8) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let Ok(dir) = fs::canonicalize(link) else {
            return found;
        };
        if depth > 4 {
            return found;
        }
        if dir.join("partition").exists() {
            if let Some(parent) = dir.parent()
                && parent.join("device").exists()
                && let Some(name) = parent.file_name()
            {
                found.insert(name.to_string_lossy().into_owned());
            }
        } else if dir.join("device").exists() {
            if let Some(name) = dir.file_name() {
                found.insert(name.to_string_lossy().into_owned());
            }
        } else if let Ok(slaves) = fs::read_dir(dir.join("slaves")) {
            for slave in slaves.flatten() {
                let name = slave.file_name();
                found.extend(disks_under(
                    root,
                    &root.join("sys/class/block").join(name),
                    depth + 1,
                ));
            }
        }
        found
    }

    fn read_trimmed(path: &Path) -> Option<String> {
        let text = fs::read_to_string(path).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }

    fn read_number(path: &Path) -> Option<u64> {
        fs::read_to_string(path).ok()?.trim().parse().ok()
    }
}

/// Where smartctl is installed, if it is.
#[cfg(target_os = "linux")]
fn smartctl() -> Option<&'static str> {
    [
        "/usr/sbin/smartctl",
        "/usr/bin/smartctl",
        "/usr/local/sbin/smartctl",
        "/sbin/smartctl",
    ]
    .into_iter()
    .find(|path| std::path::Path::new(path).exists())
}

/// How long one disk's smartctl may take.
#[cfg(target_os = "linux")]
const SMARTCTL_LIMIT: Duration = Duration::from_secs(15);

/// Each disk's health from smartctl, which reads the raw device, so privd runs
/// it. A disk asleep isn't woken (`-n standby`); its health is unknown until it
/// next spins up.
#[cfg(target_os = "linux")]
pub async fn smart_health(disks: &[String]) -> DisksHealth {
    let Some(program) = smartctl() else {
        return DisksHealth {
            disks: Vec::new(),
            note: Some(
                "smartmontools isn't installed; install it for drive health, as with apt install smartmontools"
                    .to_owned(),
            ),
        };
    };
    let mut found = Vec::new();
    for disk in disks {
        if !disk
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            continue;
        }
        let run = tokio::process::Command::new(program)
            .args([
                "-j",
                "-n",
                "standby",
                "-i",
                "-H",
                "-A",
                &format!("/dev/{disk}"),
            ])
            .kill_on_drop(true)
            .output();
        let health = match tokio::time::timeout(SMARTCTL_LIMIT, run).await {
            Ok(Ok(output)) => {
                let json: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
                smart_from_json(disk, &json, output.status.code().unwrap_or(2))
            }
            Ok(Err(e)) => unknown(disk, format!("smartctl didn't run: {e}")),
            Err(_) => unknown(disk, "smartctl didn't answer in time".to_owned()),
        };
        found.push(health);
    }
    DisksHealth {
        disks: found,
        note: None,
    }
}

#[cfg(target_os = "linux")]
fn unknown(disk: &str, detail: String) -> DiskHealth {
    DiskHealth {
        disk: disk.to_owned(),
        status: HealthStatus::Unknown,
        temperature: None,
        power_on_hours: None,
        wear: None,
        detail: Some(detail),
    }
}

/// A disk's health from smartctl's JSON and its exit status, a bitmask
/// (smartctl(8), EXIT STATUS): bit 1 the device didn't open or is asleep,
/// bit 3 "DISK FAILING", bits 4 and 5 attributes at or past their threshold,
/// now or before, bits 6 and 7 errors in the device's logs.
pub fn smart_from_json(disk: &str, json: &Value, exit: i32) -> DiskHealth {
    let number = |path: &[&str]| {
        path.iter()
            .try_fold(json, |at, key| at.get(key))
            .and_then(Value::as_u64)
    };
    let temperature = json.pointer("/temperature/current").and_then(Value::as_f64);
    let power_on_hours = number(&["power_on_time", "hours"]);
    let nvme = json.get("nvme_smart_health_information_log");
    let wear = nvme
        .and_then(|log| log.get("percentage_used"))
        .and_then(Value::as_u64)
        .and_then(|used| u32::try_from(used).ok());
    let mut problems: Vec<String> = Vec::new();
    // ATA: sectors reallocated, or waiting to be.
    if let Some(table) = json
        .pointer("/ata_smart_attributes/table")
        .and_then(Value::as_array)
    {
        let raw = |id: u64| {
            table
                .iter()
                .find(|attribute| attribute.get("id").and_then(Value::as_u64) == Some(id))
                .and_then(|attribute| attribute.pointer("/raw/value"))
                .and_then(Value::as_u64)
        };
        if let Some(count) = raw(5).filter(|count| *count > 0) {
            problems.push(format!("{count} reallocated sectors"));
        }
        if let Some(count) = raw(197).filter(|count| *count > 0) {
            problems.push(format!("{count} sectors waiting to be reallocated"));
        }
    }
    // NVMe: the controller's critical warning bits.
    if let Some(warning) = nvme
        .and_then(|log| log.get("critical_warning"))
        .and_then(Value::as_u64)
        .filter(|bits| *bits > 0)
    {
        let words = [
            (0x01, "spare capacity below its threshold"),
            (0x02, "temperature outside its limits"),
            (0x04, "reliability degraded"),
            (0x08, "media read-only"),
            (0x10, "backup memory failed"),
        ];
        problems.extend(
            words
                .iter()
                .filter(|(bit, _)| warning & bit != 0)
                .map(|(_, words)| (*words).to_owned()),
        );
    }
    if wear.is_some_and(|wear| wear >= 100) {
        problems.push("past its rated life".to_owned());
    }
    let passed = json
        .pointer("/smart_status/passed")
        .and_then(Value::as_bool);
    let asleep = exit & 0b10 != 0 && passed.is_none();
    let status = if exit & 0b1000 != 0 || passed == Some(false) {
        HealthStatus::Failing
    } else if asleep || passed.is_none() {
        HealthStatus::Unknown
    } else if exit & 0b1111_0000 != 0 || !problems.is_empty() {
        if exit & 0b1100_0000 != 0 && problems.is_empty() {
            problems.push("errors in its log".to_owned());
        }
        if exit & 0b0011_0000 != 0 && problems.is_empty() {
            problems.push("attributes past their threshold".to_owned());
        }
        HealthStatus::Warning
    } else {
        HealthStatus::Ok
    };
    let detail = if asleep {
        Some("asleep, so it wasn't woken to ask".to_owned())
    } else if status == HealthStatus::Unknown {
        json.pointer("/smartctl/messages/0/string")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| Some("it doesn't report SMART".to_owned()))
    } else {
        (!problems.is_empty()).then(|| problems.join(", "))
    };
    DiskHealth {
        disk: disk.to_owned(),
        status,
        temperature,
        power_on_hours,
        wear,
        detail,
    }
}

#[cfg(target_os = "macos")]
pub mod mac {
    //! Disks from `diskutil list -plist` and `diskutil info -plist`, kept for a
    //! minute, with ioreg's block storage statistics for their I/O; volumes as
    //! the stats topic picks them, mapped to their disk through diskutil's
    //! layout: a volume in an APFS container sits on the partition its
    //! container lives on.

    use std::collections::HashMap;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::Duration;

    use cntrl_protocol::storage::{DiskHealth, DiskKind, DisksHealth, HealthStatus, Volume};
    use plist::Value;
    use sysinfo::{DiskRefreshKind, Disks};

    use super::{DiskReading, IoCounters, Storage, VIRTUAL_MAKERS};
    use crate::mounts::{Stuck, remote_space, space};
    use crate::stats::{Background, Every};

    /// How long diskutil's view is kept.
    const LAYOUT_EVERY: Duration = Duration::from_secs(60);
    const VOLUMES_EVERY: Duration = Duration::from_secs(10);
    /// Network volumes, as the stats topic names them.
    const REMOTE: &[&str] = &[
        "smbfs", "nfs", "afpfs", "webdav", "ftp", "macfuse", "osxfuse",
    ];

    /// diskutil's view: the physical disks, and where each mounted volume is.
    #[derive(Debug, Clone, Default)]
    pub struct Layout {
        pub disks: Vec<MacDisk>,
        /// A mount point's device and physical disk.
        pub mounts: HashMap<String, (String, Option<String>)>,
    }

    #[derive(Debug, Clone, PartialEq)]
    pub struct MacDisk {
        pub name: String,
        pub model: Option<String>,
        pub size: u64,
        pub kind: DiskKind,
        pub external: bool,
    }

    pub struct MacStorage {
        layout: Mutex<Every<Layout>>,
        volumes: Background<Vec<Volume>>,
        stuck: Stuck,
    }

    impl Default for MacStorage {
        fn default() -> Self {
            Self {
                layout: Mutex::new(Every::new(LAYOUT_EVERY)),
                volumes: Background::new(VOLUMES_EVERY),
                stuck: Stuck::default(),
            }
        }
    }

    impl MacStorage {
        fn layout(&self) -> Layout {
            let mut every = self
                .layout
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            every.get(layout)
        }
    }

    impl Storage for MacStorage {
        fn disks(&self) -> Vec<DiskReading> {
            let layout = self.layout();
            let counters = output(
                "ioreg",
                &["-r", "-c", "IOBlockStorageDriver", "-l", "-d", "2", "-a"],
            )
            .map(|bytes| io_counters(&bytes))
            .unwrap_or_default();
            layout
                .disks
                .into_iter()
                .map(|disk| DiskReading {
                    counters: counters.get(&disk.name).copied().unwrap_or_default(),
                    name: disk.name,
                    model: disk.model,
                    size: disk.size,
                    kind: disk.kind,
                    external: disk.external,
                })
                .collect()
        }

        fn volumes(&self) -> Vec<Volume> {
            let layout = self.layout();
            let stuck = std::sync::Arc::clone(&self.stuck);
            self.volumes.get(move || volumes(&layout, &stuck))
        }
    }

    /// The volumes worth showing, as the stats topic picks them: `/` for the
    /// startup disk's container and whatever's under `/Volumes`.
    fn volumes(layout: &Layout, stuck: &Stuck) -> Vec<Volume> {
        let disks =
            Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage());
        disks
            .iter()
            .filter_map(|disk| {
                let mount = disk.mount_point().to_string_lossy().into_owned();
                if mount.starts_with("/System/Volumes/") || mount == "/private/var/vm" {
                    return None;
                }
                let kind = disk.file_system().to_string_lossy().into_owned();
                let network = REMOTE.contains(&kind.as_str());
                let measured = if network {
                    remote_space(stuck, &mount)
                } else {
                    space(&mount)
                };
                let (total, available) = if network {
                    let measured = measured?;
                    (measured.total, measured.available)
                } else {
                    (disk.total_space(), disk.available_space())
                };
                if total == 0 {
                    return None;
                }
                let name = disk.name().to_string_lossy().into_owned();
                let (source, on) = layout.mounts.get(&mount).cloned().unzip();
                Some(Volume {
                    name: (!name.is_empty()).then_some(name),
                    kind,
                    source: source.map(|device| format!("/dev/{device}")),
                    disk: on.flatten(),
                    total,
                    used: total.saturating_sub(available),
                    available,
                    inodes: measured.and_then(|space| space.inodes),
                    inodes_used: measured.and_then(|space| space.inodes_used),
                    // The sealed system volume is read-only by design; what's
                    // written goes to its data volume in the same container.
                    read_only: disk.is_read_only() && mount != "/",
                    network,
                    mount,
                })
            })
            .collect()
    }

    /// diskutil's layout and each physical disk's facts.
    fn layout() -> Layout {
        let Some(list) = output("diskutil", &["list", "-plist"])
            .and_then(|bytes| Value::from_reader(std::io::Cursor::new(bytes)).ok())
        else {
            return Layout::default();
        };
        let mut layout = parse_list(&list);
        for disk in &mut layout.disks {
            if let Some(info) = output("diskutil", &["info", "-plist", &disk.name])
                .and_then(|bytes| Value::from_reader(std::io::Cursor::new(bytes)).ok())
            {
                apply_info(disk, &info);
            }
        }
        layout
            .disks
            .retain(|disk| disk.kind != DiskKind::Unknown || disk.size > 0);
        layout
    }

    /// The physical disks and where each volume is, from `diskutil list
    /// -plist`. An APFS container (`Apple_APFS_Container`) is a synthesized
    /// disk; its volumes are on the partitions its physical stores name.
    pub fn parse_list(list: &Value) -> Layout {
        let mut layout = Layout::default();
        let Some(entries) = list
            .as_dictionary()
            .and_then(|list| list.get("AllDisksAndPartitions"))
            .and_then(Value::as_array)
        else {
            return layout;
        };
        let text = |entry: &Value, key: &str| {
            entry
                .as_dictionary()
                .and_then(|entry| entry.get(key))
                .and_then(Value::as_string)
                .map(str::to_owned)
        };
        let array = |entry: &Value, key: &str| {
            entry
                .as_dictionary()
                .and_then(|entry| entry.get(key))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        // A partition's whole disk: `disk0s2` is on `disk0`.
        let whole = |partition: &str| {
            partition
                .strip_prefix("disk")
                .and_then(|rest| rest.split('s').next())
                .map(|number| format!("disk{number}"))
        };
        for entry in entries {
            let Some(id) = text(entry, "DeviceIdentifier") else {
                continue;
            };
            let container = text(entry, "Content").as_deref() == Some("Apple_APFS_Container");
            if container {
                let physical = array(entry, "APFSPhysicalStores")
                    .iter()
                    .filter_map(|store| text(store, "DeviceIdentifier"))
                    .filter_map(|store| whole(&store))
                    .next();
                for volume in array(entry, "APFSVolumes") {
                    if let (Some(mount), Some(device)) = (
                        text(&volume, "MountPoint"),
                        text(&volume, "DeviceIdentifier"),
                    ) {
                        layout.mounts.insert(mount, (device, physical.clone()));
                    }
                }
                continue;
            }
            layout.disks.push(MacDisk {
                name: id.clone(),
                model: None,
                size: entry
                    .as_dictionary()
                    .and_then(|entry| entry.get("Size"))
                    .and_then(Value::as_unsigned_integer)
                    .unwrap_or(0),
                kind: DiskKind::Unknown,
                external: false,
            });
            if let Some(mount) = text(entry, "MountPoint") {
                layout.mounts.insert(mount, (id.clone(), Some(id.clone())));
            }
            for partition in array(entry, "Partitions") {
                if let (Some(mount), Some(device)) = (
                    text(&partition, "MountPoint"),
                    text(&partition, "DeviceIdentifier"),
                ) {
                    layout.mounts.insert(mount, (device, Some(id.clone())));
                }
            }
        }
        layout
    }

    /// A disk's model, size, kind and whether it's external, from `diskutil info
    /// -plist`. Disk images are virtual.
    pub fn apply_info(disk: &mut MacDisk, info: &Value) {
        let Some(info) = info.as_dictionary() else {
            return;
        };
        let text = |key: &str| {
            info.get(key)
                .and_then(Value::as_string)
                .unwrap_or_default()
                .to_owned()
        };
        let flag = |key: &str| info.get(key).and_then(Value::as_boolean);
        let model = text("MediaName");
        disk.model = (!model.is_empty()).then_some(model.clone());
        if let Some(size) = info.get("TotalSize").and_then(Value::as_unsigned_integer) {
            disk.size = size;
        }
        let protocol = text("BusProtocol");
        disk.external = flag("Internal") == Some(false)
            || flag("Removable") == Some(true)
            || flag("RemovableMedia") == Some(true);
        disk.kind = if protocol == "Disk Image"
            || text("VirtualOrPhysical") == "Virtual"
            || VIRTUAL_MAKERS.iter().any(|maker| model.contains(maker))
        {
            DiskKind::Virtual
        } else if protocol.contains("PCI")
            || protocol.contains("NVMe")
            || protocol == "Apple Fabric"
        {
            DiskKind::Nvme
        } else if flag("SolidState") == Some(true) {
            DiskKind::Ssd
        } else if flag("SolidState") == Some(false) {
            DiskKind::Hdd
        } else {
            DiskKind::Unknown
        };
    }

    /// Each disk's counters from ioreg's block storage drivers: bytes,
    /// operations and the time they took, in nanoseconds, by the BSD name of
    /// the whole disk under each driver.
    pub fn io_counters(bytes: &[u8]) -> HashMap<String, IoCounters> {
        let mut counters = HashMap::new();
        let Ok(Value::Array(drivers)) = Value::from_reader(std::io::Cursor::new(bytes)) else {
            return counters;
        };
        for driver in &drivers {
            let Some(driver) = driver.as_dictionary() else {
                continue;
            };
            let Some(stats) = driver.get("Statistics").and_then(Value::as_dictionary) else {
                continue;
            };
            let count = |key: &str| stats.get(key).and_then(Value::as_unsigned_integer);
            let name = driver
                .get("IORegistryEntryChildren")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_dictionary)
                .find(|media| media.get("Whole").and_then(Value::as_boolean) == Some(true))
                .and_then(|media| media.get("BSD Name"))
                .and_then(Value::as_string);
            let Some(name) = name else { continue };
            let nanos = count("Total Time (Read)")
                .zip(count("Total Time (Write)"))
                .map(|(r, w)| r.saturating_add(w));
            counters.insert(
                name.to_owned(),
                IoCounters {
                    read: count("Bytes (Read)").unwrap_or(0),
                    written: count("Bytes (Write)").unwrap_or(0),
                    reads: count("Operations (Read)"),
                    writes: count("Operations (Write)"),
                    time_ms: nanos.map(|nanos| nanos / 1_000_000),
                    busy_ms: None,
                },
            );
        }
        counters
    }

    /// What each disk says of its health, as diskutil reports SMART.
    pub fn health(disks: &[String]) -> DisksHealth {
        let disks = disks
            .iter()
            .filter(|disk| {
                disk.strip_prefix("disk").is_some_and(|number| {
                    !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                })
            })
            .map(|disk| {
                let status = output("diskutil", &["info", "-plist", disk])
                    .and_then(|bytes| Value::from_reader(std::io::Cursor::new(bytes)).ok())
                    .and_then(|info| {
                        info.as_dictionary()
                            .and_then(|info| info.get("SMARTStatus"))
                            .and_then(Value::as_string)
                            .map(str::to_owned)
                    });
                let (status, detail) = match status.as_deref() {
                    Some("Verified") => (HealthStatus::Ok, None),
                    Some("Failing") => (
                        HealthStatus::Failing,
                        Some("SMART says it's failing".to_owned()),
                    ),
                    Some(other) => (HealthStatus::Unknown, Some(format!("SMART: {other}"))),
                    None => (
                        HealthStatus::Unknown,
                        Some("diskutil didn't say".to_owned()),
                    ),
                };
                DiskHealth {
                    disk: disk.clone(),
                    status,
                    temperature: None,
                    power_on_hours: None,
                    wear: None,
                    detail,
                }
            })
            .collect();
        DisksHealth { disks, note: None }
    }

    fn output(program: &str, args: &[&str]) -> Option<Vec<u8>> {
        let output = Command::new(program).args(args).output().ok()?;
        output.status.success().then_some(output.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(counters: IoCounters) -> DiskReading {
        DiskReading {
            name: "sda".to_owned(),
            model: Some("WDC WD40EFZX".to_owned()),
            size: 4_000_787_030_016,
            kind: DiskKind::Hdd,
            external: false,
            counters,
        }
    }

    #[test]
    fn turns_counters_into_rates_as_iostat_does() {
        let before = IoCounters {
            read: 1_000_000,
            written: 0,
            reads: Some(100),
            writes: Some(50),
            time_ms: Some(1_000),
            busy_ms: Some(10_000),
        };
        let after = IoCounters {
            read: 3_000_000,
            written: 1_000_000,
            reads: Some(300),
            writes: Some(70),
            time_ms: Some(3_200),
            busy_ms: Some(10_500),
        };
        let disk = disk_sample(&reading(after), Some(&before), Duration::from_secs(2));
        assert_eq!((disk.read, disk.written), (1_000_000, 500_000));
        assert_eq!((disk.reads, disk.writes), (Some(100.0), Some(10.0)));
        // 2200 ms over 220 requests.
        assert_eq!(disk.wait_ms, Some(10.0));
        assert_eq!(disk.busy, Some(0.25));
        // The first reading has nothing to compare with.
        let first = disk_sample(&reading(after), None, Duration::ZERO);
        assert_eq!((first.read, first.reads, first.busy), (0, None, None));
    }

    #[test]
    fn reads_smartctls_verdicts() {
        let ok: Value = serde_json::json!({
            "smart_status": {"passed": true},
            "temperature": {"current": 41},
            "power_on_time": {"hours": 9812},
            "nvme_smart_health_information_log": {"critical_warning": 0, "percentage_used": 2}
        });
        let health = smart_from_json("nvme0n1", &ok, 0);
        assert_eq!(health.status, HealthStatus::Ok);
        assert_eq!(
            (health.temperature, health.power_on_hours, health.wear),
            (Some(41.0), Some(9812), Some(2))
        );
        assert_eq!(health.detail, None);

        let worn: Value = serde_json::json!({
            "smart_status": {"passed": true},
            "ata_smart_attributes": {"table": [
                {"id": 5, "name": "Reallocated_Sector_Ct", "raw": {"value": 8}},
                {"id": 197, "name": "Current_Pending_Sector", "raw": {"value": 0}}
            ]}
        });
        let health = smart_from_json("sda", &worn, 0);
        assert_eq!(health.status, HealthStatus::Warning);
        assert_eq!(health.detail.as_deref(), Some("8 reallocated sectors"));

        let failing: Value = serde_json::json!({"smart_status": {"passed": false}});
        assert_eq!(
            smart_from_json("sdb", &failing, 8).status,
            HealthStatus::Failing
        );

        let asleep: Value = serde_json::json!({"smartctl": {"messages": [{"string": "Device is in STANDBY mode, exit(2)"}]}});
        let health = smart_from_json("sdc", &asleep, 2);
        assert_eq!(health.status, HealthStatus::Unknown);
        assert_eq!(
            health.detail.as_deref(),
            Some("asleep, so it wasn't woken to ask")
        );

        let logged: Value = serde_json::json!({"smart_status": {"passed": true}});
        let health = smart_from_json("sdd", &logged, 64);
        assert_eq!(
            (health.status, health.detail.as_deref()),
            (HealthStatus::Warning, Some("errors in its log"))
        );
    }

    #[test]
    fn finds_linux_disks_and_their_kinds() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        let disk = |name: &str, rotational: &str, model: &str| {
            let base = root.join("sys/block").join(name);
            std::fs::create_dir_all(base.join("device")).expect("mkdir");
            std::fs::create_dir_all(base.join("queue")).expect("mkdir");
            std::fs::write(base.join("queue/rotational"), rotational).expect("write");
            std::fs::write(base.join("device/model"), model).expect("write");
            std::fs::write(base.join("size"), "7814037168\n").expect("write");
            std::fs::write(base.join("removable"), "0\n").expect("write");
        };
        disk("nvme0n1", "0\n", "Samsung SSD 980 PRO 1TB          \n");
        disk("sda", "1\n", "WDC WD40EFZX-68A\n");
        disk("vda", "1\n", "\n");
        std::fs::create_dir_all(root.join("sys/block/loop0")).expect("mkdir");
        std::fs::create_dir_all(root.join("proc")).expect("mkdir");
        std::fs::write(
            root.join("proc/diskstats"),
            concat!(
                " 259       0 nvme0n1 120 0 9600 40 50 0 4000 60 0 90 100 0 0 0 0 0 0\n",
                " 259       1 nvme0n1p1 10 0 80 4 0 0 0 0 0 4 4 0 0 0 0\n",
                "   8       0 sda 7 0 56 70 0 0 0 0 0 70 70 0 0 0 0 0 0\n",
                " 253       0 vda 1 0 8 1 1 0 8 1 0 2 2 0 0 0 0 0 0\n",
                "   7       0 loop0 1 0 8 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
            ),
        )
        .expect("write");
        let disks = linux::disks(root);
        let summary: Vec<(&str, DiskKind, Option<&str>)> = disks
            .iter()
            .map(|d| (d.name.as_str(), d.kind, d.model.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                ("nvme0n1", DiskKind::Nvme, Some("Samsung SSD 980 PRO 1TB")),
                ("sda", DiskKind::Hdd, Some("WDC WD40EFZX-68A")),
                ("vda", DiskKind::Virtual, None),
            ]
        );
        assert_eq!(disks[0].size, 7_814_037_168 * 512);
        assert_eq!(
            disks[0].counters,
            IoCounters {
                read: 9600 * 512,
                written: 4000 * 512,
                reads: Some(120),
                writes: Some(50),
                time_ms: Some(100),
                busy_ms: Some(90),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn traces_a_partition_and_an_lvm_volume_to_their_disk() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        let devices = root.join("sys/devices/pci0/nvme/nvme0/nvme0n1");
        std::fs::create_dir_all(devices.join("device")).expect("mkdir");
        std::fs::create_dir_all(devices.join("nvme0n1p3")).expect("mkdir");
        std::fs::write(devices.join("nvme0n1p3/partition"), "3\n").expect("write");
        let dm = root.join("sys/devices/virtual/block/dm-0");
        std::fs::create_dir_all(dm.join("slaves")).expect("mkdir");
        for dir in ["sys/dev/block", "sys/class/block"] {
            std::fs::create_dir_all(root.join(dir)).expect("mkdir");
        }
        symlink(devices.join("nvme0n1p3"), root.join("sys/dev/block/259:3")).expect("link");
        symlink(
            devices.join("nvme0n1p3"),
            root.join("sys/class/block/nvme0n1p3"),
        )
        .expect("link");
        symlink(&dm, root.join("sys/dev/block/253:0")).expect("link");
        std::fs::create_dir_all(dm.join("slaves/nvme0n1p3")).expect("mkdir");
        assert_eq!(linux::disk_of(root, "259:3").as_deref(), Some("nvme0n1"));
        assert_eq!(linux::disk_of(root, "253:0").as_deref(), Some("nvme0n1"));
        assert_eq!(linux::disk_of(root, "0:57"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn maps_this_macs_volumes_to_its_disks() {
        let storage = mac::MacStorage::default();
        let disks = storage.disks();
        assert!(!disks.is_empty(), "no disks");
        let startup = disks
            .iter()
            .find(|disk| disk.name == "disk0")
            .expect("disk0");
        assert!(startup.size > 0 && startup.counters.read > 0, "{startup:?}");
        // The first call starts the look and returns what's known: nothing.
        let mut volumes = storage.volumes();
        for _ in 0..50 {
            if !volumes.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            volumes = storage.volumes();
        }
        let root = volumes
            .iter()
            .find(|volume| volume.mount == "/")
            .expect("/");
        assert_eq!(root.disk.as_deref(), Some("disk0"), "{root:?}");
        assert!(!root.read_only);
        let health = mac::health(&["disk0".to_owned()]);
        assert_eq!(health.disks.len(), 1);
    }
}
