//! Types for the Storage tab (angle 13): the `storage` topic, the machine's
//! disks with what's moving on each and its volumes, read only while someone
//! watches; and `storage.health`, what each disk says of its own health.

use serde::{Deserialize, Serialize};

/// What a `storage` subscription asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StorageParams {
    /// How often to send, in milliseconds: 2000 when absent, 1000 at least.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u32>,
}

/// One `storage` event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StorageSample {
    /// When it was read, in Unix milliseconds.
    pub ts: u64,
    /// The physical disks, not partitions, RAID or device-mapper devices.
    pub disks: Vec<Disk>,
    pub volumes: Vec<Volume>,
}

/// A physical disk, with its traffic since the previous reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Disk {
    /// As the OS names it, such as `sda`, `nvme0n1` or `disk0`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// In bytes.
    pub size: u64,
    pub kind: DiskKind,
    /// Plugged in over USB or Thunderbolt, or removable media.
    pub external: bool,
    /// Bytes per second.
    pub read: u64,
    pub written: u64,
    /// Requests completed per second, where the OS counts them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reads: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writes: Option<f64>,
    /// How long a request took on average, queueing included, in
    /// milliseconds; absent when none completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<f64>,
    /// The share of the time it had requests in progress, 0 to 1. A disk
    /// that serves requests in parallel, as an SSD does, can take more while
    /// at 1 (iostat(1), %util).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DiskKind {
    Nvme,
    /// A solid-state disk on SATA, SAS or USB.
    Ssd,
    /// A spinning disk.
    Hdd,
    /// A disk a hypervisor provides.
    Virtual,
    #[serde(other)]
    Unknown,
}

/// A mounted filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Volume {
    /// Where it's mounted, such as `/` or `/mnt/media`.
    pub mount: String,
    /// The volume's name where the OS gives one, such as `Macintosh HD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its type, such as `ext4`, `zfs`, `nfs4` or `apfs`.
    pub kind: String,
    /// What's mounted: a device such as `/dev/nvme0n1p2`, or for a network
    /// volume its server and share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The physical disk it's on, by name, when there's one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<String>,
    pub total: u64,
    /// In use. On a Mac it's the whole APFS container's use, since its volumes
    /// share the space.
    pub used: u64,
    /// What a user other than root can still write.
    pub available: u64,
    /// Files it can hold, where the filesystem has a fixed number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inodes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inodes_used: Option<u64>,
    pub read_only: bool,
    /// Mounted from another machine: NFS, SMB and the like.
    pub network: bool,
}

/// The result of `storage.health`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DisksHealth {
    pub disks: Vec<DiskHealth>,
    /// Why there's nothing to say, when there isn't: on Linux, that
    /// smartmontools isn't installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What one disk says of its health.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DiskHealth {
    /// The disk's name, as `storage` gives it.
    pub disk: String,
    pub status: HealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_on_hours: Option<u64>,
    /// How much of an SSD's rated life is used, as a percentage; it can pass
    /// 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wear: Option<u32>,
    /// What's wrong, in a few words, when something is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// The disk passes its own check.
    Ok,
    /// It passes, but something is past a limit or was once: worn out, too
    /// hot, errors logged, attributes that crossed their threshold.
    Warning,
    /// It says it's failing.
    Failing,
    /// It can't say, or wasn't asked, as a disk asleep isn't woken for this.
    #[serde(other)]
    Unknown,
}
