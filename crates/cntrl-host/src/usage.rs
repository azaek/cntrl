//! What a process has used since it started: its CPU time, that of the
//! children it waited for, and the bytes it read and wrote (D121). Follow-ups
//! read these each 15 s for a job's processes and judge a minute from the
//! difference, so a short task that ran and ended between two looks at the
//! process table still counts: its parent takes in its time when it waits for
//! it. Linux keeps the children's CPU time and storage bytes; macOS their CPU
//! time; Windows neither. Reading another user's process takes root, so privd
//! reads these.

/// A process's running totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// CPU time, user and system, in milliseconds.
    pub cpu_ms: u64,
    /// The CPU time of the children it waited for, theirs and their own
    /// children's, in milliseconds; none where the OS doesn't keep it.
    pub children_cpu_ms: Option<u64>,
    /// Bytes read and written: from storage on Linux, with the waited-for
    /// children's in them ([`CHILDREN_DISK`]); from storage on macOS; every
    /// read and write on Windows. None where it can't be read.
    pub disk_bytes: Option<u64>,
}

/// Whether a parent's disk bytes take in those of the children it waits for,
/// as Linux's do.
pub const CHILDREN_DISK: bool = cfg!(target_os = "linux");

/// The totals of `pid`, if it's still the process that started at `started`
/// (Unix seconds, as the process table gives it).
#[cfg(target_os = "linux")]
pub fn read(pid: u32, started: u64) -> Option<Usage> {
    linux::read(std::path::Path::new("/proc"), pid, started)
}

#[cfg(target_os = "macos")]
pub use mac::read;

#[cfg(windows)]
pub use crate::processes::usage as read;

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::sync::OnceLock;

    use procfs_core::process::{Io, Stat};
    use procfs_core::{FromBufRead, FromRead};

    use super::Usage;

    pub fn read(proc: &Path, pid: u32, started: u64) -> Option<Usage> {
        let stat = std::fs::read(proc.join(pid.to_string()).join("stat")).ok()?;
        let stat = Stat::from_read(stat.as_slice()).ok()?;
        let ticks = rustix::param::clock_ticks_per_second().max(1);
        // As sysinfo counts it: the boot time and the ticks since, in seconds.
        let began = boot_time(proc)? + stat.starttime / ticks;
        if began.abs_diff(started) > 1 {
            return None;
        }
        let io = std::fs::read(proc.join(pid.to_string()).join("io"))
            .ok()
            .and_then(|io| Io::from_buf_read(io.as_slice()).ok());
        Some(totals(&stat, io.as_ref(), ticks))
    }

    /// The totals from a process's `stat` and `io`, at `ticks` a second.
    pub fn totals(stat: &Stat, io: Option<&Io>, ticks: u64) -> Usage {
        let ms = |t: u64| t.saturating_mul(1000) / ticks.max(1);
        let children = u64::try_from(stat.cutime.max(0)).unwrap_or(0)
            + u64::try_from(stat.cstime.max(0)).unwrap_or(0);
        Usage {
            cpu_ms: ms(stat.utime + stat.stime),
            children_cpu_ms: Some(ms(children)),
            disk_bytes: io.map(|io| io.read_bytes + io.write_bytes),
        }
    }

    /// When the machine booted, in Unix seconds, from `/proc/stat`; it
    /// doesn't change, so it's read once.
    fn boot_time(proc: &Path) -> Option<u64> {
        static BOOT: OnceLock<Option<u64>> = OnceLock::new();
        *BOOT.get_or_init(|| {
            let stat = std::fs::read_to_string(proc.join("stat")).ok()?;
            stat.lines()
                .find_map(|line| line.strip_prefix("btime "))
                .and_then(|value| value.trim().parse().ok())
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_this_process() {
            let started = crate::processes::target(std::process::id())
                .expect("this process")
                .started;
            let usage = read(Path::new("/proc"), std::process::id(), started).expect("its usage");
            assert!(usage.children_cpu_ms.is_some());
            // Another start time is another process.
            assert!(read(Path::new("/proc"), std::process::id(), started + 100).is_none());
        }
    }
}

/// macOS keeps a process's totals in `proc_pid_rusage`, its CPU in Mach
/// time units, which on Apple silicon aren't nanoseconds.
#[cfg(target_os = "macos")]
mod mac {
    #![allow(unsafe_code)]

    use std::sync::OnceLock;

    use super::Usage;

    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }

    unsafe extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }

    pub fn read(pid: u32, started: u64) -> Option<Usage> {
        let pid = i32::try_from(pid).ok()?;
        // SAFETY: zeroed is a valid proc_bsdinfo; proc_pidinfo writes at most
        // `size` bytes into it.
        let mut bsd: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
        let filled = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut bsd).cast(), size)
        };
        if filled != size || bsd.pbi_start_tvsec != started {
            return None;
        }
        // SAFETY: zeroed is a valid rusage_info_v4, the flavour asked for,
        // which proc_pid_rusage fills.
        let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        let ok =
            unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, (&raw mut info).cast()) };
        if ok != 0 {
            return None;
        }
        Some(Usage {
            cpu_ms: millis(info.ri_user_time + info.ri_system_time),
            children_cpu_ms: Some(millis(info.ri_child_user_time + info.ri_child_system_time)),
            disk_bytes: Some(info.ri_diskio_bytesread + info.ri_diskio_byteswritten),
        })
    }

    /// Mach time units in milliseconds.
    fn millis(units: u64) -> u64 {
        static TIMEBASE: OnceLock<(u64, u64)> = OnceLock::new();
        let (numer, denom) = *TIMEBASE.get_or_init(|| {
            let mut timebase = Timebase { numer: 1, denom: 1 };
            // SAFETY: a plain call that fills the struct.
            if unsafe { mach_timebase_info(&raw mut timebase) } != 0 || timebase.denom == 0 {
                return (1, 1);
            }
            (u64::from(timebase.numer), u64::from(timebase.denom))
        });
        let nanos = u128::from(units) * u128::from(numer) / u128::from(denom);
        u64::try_from(nanos / 1_000_000).unwrap_or(u64::MAX)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_this_process_and_counts_its_cpu() {
            let pid = std::process::id();
            let started = crate::processes::target(pid).expect("this process").started;
            let before = read(pid, started).expect("its usage");
            // Half a second of work shows as CPU time in milliseconds, not Mach units.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
            let mut spin = 0u64;
            while std::time::Instant::now() < until {
                spin = std::hint::black_box(spin.wrapping_add(1));
            }
            let after = read(pid, started).expect("its usage");
            let spent = after.cpu_ms - before.cpu_ms;
            assert!((300..=1500).contains(&spent), "{spent} ms");
            assert!(read(pid, started + 100).is_none());
        }
    }
}
