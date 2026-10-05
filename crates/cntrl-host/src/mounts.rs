//! Filesystems on Linux: which mounts to show, from `/proc/self/mountinfo`,
//! and their space, from statvfs(3). statvfs on a network mount whose server
//! has gone can block for minutes, so space is read on a thread of its own, and
//! a mount still stuck from an earlier look is left out until it answers, as
//! node_exporter does (angle 06 §1.3, angle 09).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use cntrl_protocol::stats::Filesystem;
use procfs_core::FromBufRead;
use procfs_core::process::MountInfos;

use crate::stats::Background;

/// Filesystems on the machine's own disks, whose statvfs answers at once.
const LOCAL: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "btrfs", "bcachefs", "zfs", "f2fs", "jfs", "reiserfs", "nilfs2",
    "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "hfsplus",
];

/// Network filesystems, which can block when their server goes away. FUSE
/// filesystems (`fuse.*`) are treated the same, since their daemon can hang.
const REMOTE: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "9p",
    "virtiofs",
    "ceph",
    "glusterfs",
];

/// Where the kernel and container runtimes mount things that aren't the
/// machine's storage.
const HIDDEN: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run/credentials",
    "/var/lib/docker",
    "/var/lib/containers",
    "/var/lib/kubelet",
    "/snap",
];

/// How long a network mount's statvfs may take before it counts as stuck.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(2);

/// A mount worth showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mount {
    pub mount: String,
    pub kind: String,
    /// A network or FUSE mount, whose statvfs can block.
    pub remote: bool,
    /// What's mounted, such as `/dev/nvme0n1p2` or `nas:/export`.
    pub source: Option<String>,
    /// The device's `major:minor`, which leads to its disk in sysfs.
    pub device: String,
}

/// The mounts worth showing, from mountinfo's text: `/` always (in a container
/// it's an overlay, but its space is the disk's), then disk, network and FUSE
/// filesystems mounted from their root, away from the kernel's and container
/// runtimes' places. A filesystem mounted twice shows once, at its shortest
/// path; btrfs subvolumes are mounted from paths inside it, so they count as
/// the same filesystem.
pub(crate) fn select(mountinfo: &str) -> Vec<Mount> {
    let Ok(infos) = MountInfos::from_buf_read(mountinfo.as_bytes()) else {
        return Vec::new();
    };
    let mut chosen: HashMap<(String, String), Mount> = HashMap::new();
    for info in infos {
        let mount = info.mount_point.to_string_lossy().into_owned();
        let kind = info.fs_type;
        let remote = REMOTE.contains(&kind.as_str()) || kind.starts_with("fuse.");
        let wanted = mount == "/"
            || ((LOCAL.contains(&kind.as_str()) || remote)
                && (info.root == "/" || matches!(kind.as_str(), "btrfs" | "bcachefs"))
                && !HIDDEN
                    .iter()
                    .any(|hidden| Path::new(&mount).starts_with(hidden)));
        if !wanted {
            continue;
        }
        let source = info.mount_source.filter(|source| !source.is_empty());
        let key = (source.clone().unwrap_or_default(), kind.clone());
        if chosen
            .get(&key)
            .is_none_or(|existing| mount.len() < existing.mount.len())
        {
            chosen.insert(
                key,
                Mount {
                    mount,
                    kind,
                    remote,
                    source,
                    device: info.majmin,
                },
            );
        }
    }
    let mut mounts: Vec<Mount> = chosen.into_values().collect();
    mounts.sort_by(|a, b| a.mount.cmp(&b.mount));
    mounts
}

/// The mount points mounted read-only, from a mountinfo. The agent's own
/// (`/proc/self/mountinfo`) won't do: its unit's `ProtectSystem=strict` mounts
/// everything read-only for it alone, so the machine's are PID 1's.
pub(crate) fn read_only(mountinfo: &Path) -> Option<HashSet<String>> {
    let text = fs::read_to_string(mountinfo).ok()?;
    let infos = MountInfos::from_buf_read(text.as_bytes()).ok()?;
    Some(
        infos
            .into_iter()
            .filter(|info| info.mount_options.contains_key("ro"))
            .map(|info| info.mount_point.to_string_lossy().into_owned())
            .collect(),
    )
}

/// A filesystem's space, and its files where it has a fixed number of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Space {
    pub total: u64,
    /// Used counts the blocks kept for root, as df(1) does, so used and
    /// available don't add up to the total.
    pub used: u64,
    pub available: u64,
    pub inodes: Option<u64>,
    pub inodes_used: Option<u64>,
}

/// A filesystem's space, from statvfs(3). A filesystem that makes inodes as it
/// needs them, as btrfs does, says it has none.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn space(path: &str) -> Option<Space> {
    let stat = rustix::fs::statvfs(path).ok()?;
    let unit = if stat.f_frsize > 0 {
        stat.f_frsize
    } else {
        stat.f_bsize
    };
    let total = stat.f_blocks.saturating_mul(unit);
    let free = stat.f_bfree.saturating_mul(unit);
    let inodes = (stat.f_files > 0).then_some(stat.f_files);
    Some(Space {
        total,
        used: total.saturating_sub(free),
        available: stat.f_bavail.saturating_mul(unit),
        inodes,
        inodes_used: inodes.map(|files| files.saturating_sub(stat.f_ffree)),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn space(_path: &str) -> Option<Space> {
    None
}

/// Network mounts whose statvfs hasn't returned yet, shared by the reads.
pub(crate) type Stuck = Arc<Mutex<HashSet<String>>>;

/// Linux's filesystems with their space, looked at again on a thread of its
/// own once the last look is `every` old.
#[derive(Debug)]
pub(crate) struct LinuxFilesystems {
    mountinfo: PathBuf,
    stuck: Stuck,
    watch: Background<Vec<Filesystem>>,
}

impl LinuxFilesystems {
    pub(crate) fn new(mountinfo: PathBuf, every: Duration) -> Self {
        Self {
            mountinfo,
            stuck: Stuck::default(),
            watch: Background::new(every),
        }
    }

    /// The filesystems as of the last look, starting another when one is due.
    pub(crate) fn get(&self) -> Vec<Filesystem> {
        let (mountinfo, stuck) = (self.mountinfo.clone(), Arc::clone(&self.stuck));
        self.watch.get(move || look(&mountinfo, &stuck))
    }
}

/// Selects the mounts and measures them: disks at once, network and FUSE
/// mounts each on a thread of its own, skipping any still stuck.
fn look(mountinfo: &Path, stuck: &Stuck) -> Vec<Filesystem> {
    measure(mountinfo, stuck)
        .into_iter()
        .map(|(mount, space)| Filesystem {
            mount: mount.mount,
            name: None,
            kind: mount.kind,
            total: space.total,
            used: space.used,
            available: space.available,
        })
        .collect()
}

/// The mounts worth showing with their space: disks at once, network and FUSE
/// mounts each on a thread of its own, skipping any still stuck, and any that
/// says it has no space.
pub(crate) fn measure(mountinfo: &Path, stuck: &Stuck) -> Vec<(Mount, Space)> {
    let mounts = fs::read_to_string(mountinfo)
        .map(|text| select(&text))
        .unwrap_or_default();
    mounts
        .into_iter()
        .filter_map(|mount| {
            let space = if mount.remote {
                remote_space(stuck, &mount.mount)?
            } else {
                space(&mount.mount)?
            };
            (space.total > 0).then_some((mount, space))
        })
        .collect()
}

/// statvfs on a thread of its own, given up on after `REMOTE_TIMEOUT`. The
/// mount counts as stuck, and isn't tried again, until that thread returns.
pub(crate) fn remote_space(stuck: &Stuck, mount: &str) -> Option<Space> {
    if !lock(stuck).insert(mount.to_owned()) {
        return None;
    }
    let (sender, receiver) = mpsc::channel();
    let (shared, path) = (Arc::clone(stuck), mount.to_owned());
    let started = thread::Builder::new()
        .name("statvfs".to_owned())
        .spawn(move || {
            let measured = space(&path);
            lock(&shared).remove(&path);
            let _ = sender.send(measured);
        });
    if started.is_err() {
        lock(stuck).remove(mount);
        return None;
    }
    receiver.recv_timeout(REMOTE_TIMEOUT).ok().flatten()
}

fn lock(stuck: &Mutex<HashSet<String>>) -> MutexGuard<'_, HashSet<String>> {
    stuck.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a Debian 13 container in Docker Desktop's Linux VM.
    const CONTAINER: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/host/proc/self/mountinfo"
    );

    fn line(id: u32, root: &str, mount: &str, kind: &str, source: &str) -> String {
        format!("{id} 1 8:{id} {root} {mount} rw,relatime shared:{id} - {kind} {source} rw\n")
    }

    #[test]
    fn a_container_shows_its_root_only() {
        let text = fs::read_to_string(CONTAINER).expect("the fixture");
        let mounts = select(&text);
        // /etc/hostname and friends are ext4, but bind-mounted files.
        assert_eq!(
            mounts,
            vec![Mount {
                mount: "/".to_owned(),
                kind: "overlay".to_owned(),
                remote: false,
                source: Some("overlay".to_owned()),
                device: "0:57".to_owned(),
            }]
        );
    }

    #[test]
    fn picks_the_machines_storage() {
        let text = [
            line(1, "/@", "/", "btrfs", "/dev/nvme0n1p2"),
            line(2, "/@home", "/home", "btrfs", "/dev/nvme0n1p2"),
            line(3, "/", "/boot/efi", "vfat", "/dev/nvme0n1p1"),
            line(4, "/", "/tank/media", "zfs", "tank/media"),
            line(5, "/", "/mnt/nas", "nfs4", "nas:/export"),
            line(6, "/", "/mnt/pool", "fuse.mergerfs", "pool"),
            line(7, "/", "/run", "tmpfs", "tmpfs"),
            line(8, "/", "/snap/core/1", "squashfs", "/dev/loop0"),
            line(
                9,
                "/",
                "/var/lib/docker/overlay2/x/merged",
                "overlay",
                "overlay",
            ),
            line(10, "/", "/var/lib/docker", "ext4", "/dev/sdb1"),
            line(11, "/", "/sys/fs/cgroup", "cgroup2", "cgroup2"),
            line(12, "/", "/mnt/backup", "ext4", "/dev/sdb1"),
        ]
        .concat();
        let mounts: Vec<(String, bool)> = select(&text)
            .into_iter()
            .map(|m| (m.mount, m.remote))
            .collect();
        assert_eq!(
            mounts,
            [
                ("/", false),
                ("/boot/efi", false),
                ("/mnt/backup", false),
                ("/mnt/nas", true),
                ("/mnt/pool", true),
                ("/tank/media", false),
            ]
            .map(|(mount, remote)| (mount.to_owned(), remote))
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_stuck_mount_is_skipped_until_it_answers() {
        let stuck = Stuck::default();
        lock(&stuck).insert("/".to_owned());
        assert_eq!(remote_space(&stuck, "/"), None);
        lock(&stuck).clear();
        assert!(remote_space(&stuck, "/").is_some());
        assert!(lock(&stuck).is_empty());
    }

    #[test]
    fn nonsense_selects_nothing() {
        assert_eq!(select("not mountinfo"), Vec::new());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn measures_the_root_filesystem() {
        let measured = space("/").expect("statvfs on /");
        assert!(
            measured.total > 0
                && measured.used <= measured.total
                && measured.available <= measured.total
        );
        assert!(measured.inodes_used <= measured.inodes);
        assert_eq!(space("/nonexistent/path"), None);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_look_measures_what_it_selects() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mountinfo = dir.path().join("mountinfo");
        fs::write(
            &mountinfo,
            [
                line(1, "/", "/", "ext4", "/dev/sda1"),
                line(2, "/", "/nonexistent/disk", "ext4", "/dev/sdc1"),
            ]
            .concat(),
        )
        .expect("write");
        let found = look(&mountinfo, &Stuck::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            (found[0].mount.as_str(), found[0].kind.as_str()),
            ("/", "ext4")
        );
        assert!(found[0].total > 0);
    }
}
