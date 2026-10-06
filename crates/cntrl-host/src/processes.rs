//! The process table, and stopping a process (D24, research angle 06). One row
//! per process with its threads folded in, read with sysinfo on Linux, macOS
//! and Windows. On macOS and Windows only root, or SYSTEM, sees other users'
//! processes, so privd reads the table there; on Linux the unprivileged agent
//! can. A stop names the process by PID and start time, so a PID that has been
//! reused is left alone.

use std::ops::RangeInclusive;

use cntrl_protocol::process::{
    ProcessInfo, ProcessOwner, ProcessSort, ProcessesParams, ProcessesSample,
};

/// Rows a subscription gets when it doesn't say.
pub const DEFAULT_LIMIT: u32 = 50;
/// The most rows one subscription can ask for.
pub const MAX_LIMIT: u32 = 500;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use os::{Sampler, Target, stop, target};
#[cfg(windows)]
pub use windows::{Sampler, Target, stop, target};

#[cfg(windows)]
mod windows;

/// A subscription's view of the table: the processes matching its query and
/// its owner, sorted its way, at most `limit` of them.
pub fn view(table: &[ProcessInfo], params: &ProcessesParams, ts: u64) -> ProcessesSample {
    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(str::to_lowercase);
    let mut matched: Vec<&ProcessInfo> = table
        .iter()
        .filter(|process| query.as_deref().is_none_or(|query| matches(process, query)))
        .filter(|process| {
            params
                .owner
                .is_none_or(|owner| process.system == (owner == ProcessOwner::System))
        })
        .collect();
    match params.sort {
        ProcessSort::Cpu => matched.sort_by(|a, b| {
            b.cpu
                .total_cmp(&a.cpu)
                .then(b.memory.cmp(&a.memory))
                .then(a.pid.cmp(&b.pid))
        }),
        ProcessSort::Memory => matched.sort_by(|a, b| {
            b.memory
                .cmp(&a.memory)
                .then(b.cpu.total_cmp(&a.cpu))
                .then(a.pid.cmp(&b.pid))
        }),
    }
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    ProcessesSample {
        ts,
        total: u32::try_from(table.len()).unwrap_or(u32::MAX),
        matched: u32::try_from(matched.len()).unwrap_or(u32::MAX),
        processes: matched
            .into_iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .cloned()
            .collect(),
    }
}

fn matches(process: &ProcessInfo, query: &str) -> bool {
    let contains =
        |text: Option<&str>| text.is_some_and(|text| text.to_lowercase().contains(query));
    process.pid.to_string() == query
        || contains(Some(&process.name))
        || contains(process.user.as_deref())
        || contains(process.unit.as_deref())
}

/// The user IDs people's accounts take on Linux (D60): `UID_MIN` to `UID_MAX`
/// from `/etc/login.defs`, 1000 and 60000 where it doesn't say, as systemd's
/// regular users. The rest are the system's.
pub fn linux_people(login_defs: &str) -> RangeInclusive<u32> {
    let value = |key: &str| {
        login_defs.lines().find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == key)
                .then(|| words.next()?.parse::<u32>().ok())
                .flatten()
        })
    };
    value("UID_MIN").unwrap_or(1000)..=value("UID_MAX").unwrap_or(60000)
}

/// A Mac's: 501 up, as the first person's account is, short of `nobody`, -2,
/// and -1.
pub const MAC_PEOPLE: RangeInclusive<u32> = 501..=u32::MAX - 2;

/// The service a process runs in, from its `/proc/<pid>/cgroup`: on cgroup v2,
/// or v1's systemd hierarchy, the innermost `.service` in its path.
pub fn parse_unit(cgroup: &str) -> Option<String> {
    let path = cgroup.lines().find_map(|line| {
        let mut parts = line.splitn(3, ':');
        let (_, controllers, path) = (parts.next()?, parts.next()?, parts.next()?);
        (controllers.is_empty() || controllers == "name=systemd").then_some(path)
    })?;
    path.rsplit('/')
        .find(|part| part.ends_with(".service"))
        .map(str::to_owned)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod os {
    use std::collections::HashMap;
    use std::ops::RangeInclusive;
    use std::time::{Duration, Instant};

    use cntrl_protocol::process::{ProcessInfo, StopResult};
    use rustix::io::Errno;
    use rustix::process::{Pid as OsPid, Signal};
    use sysinfo::{
        Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind, Users,
    };

    use crate::HostError;

    /// A table read after a longer gap than this is read twice, a moment apart,
    /// so its CPU figures cover a known span.
    const STALE: Duration = Duration::from_secs(10);
    /// How often user names are read again.
    const USERS_EVERY: Duration = Duration::from_secs(60);
    /// How long a process gets to exit after SIGTERM, and after SIGKILL.
    const TERM_WAIT: Duration = Duration::from_secs(5);
    const KILL_WAIT: Duration = Duration::from_secs(2);

    /// Reads the process table, keeping what CPU figures and names need
    /// between reads.
    pub struct Sampler {
        system: System,
        users: Users,
        users_read: Instant,
        /// The user IDs people's accounts take; the rest are the system's.
        people: RangeInclusive<u32>,
        read: Option<Instant>,
        /// Each process's unit, by PID and start time, since it doesn't change.
        units: HashMap<(u32, u64), Option<String>>,
    }

    impl Default for Sampler {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Sampler {
        pub fn new() -> Self {
            Self {
                system: System::new(),
                users: Users::new_with_refreshed_list(),
                users_read: Instant::now(),
                #[cfg(target_os = "linux")]
                people: super::linux_people(
                    &std::fs::read_to_string("/etc/login.defs").unwrap_or_default(),
                ),
                #[cfg(target_os = "macos")]
                people: super::MAC_PEOPLE,
                read: None,
                units: HashMap::new(),
            }
        }

        /// Every process, with CPU since the previous read. It blocks for a
        /// moment when the last read is stale.
        pub fn read(&mut self) -> Vec<ProcessInfo> {
            let kind = ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_user(UpdateKind::OnlyIfNotSet)
                .without_tasks();
            if self.read.is_none_or(|read| read.elapsed() > STALE) {
                self.system
                    .refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
                std::thread::sleep(
                    sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(250)),
                );
            }
            self.system
                .refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
            self.read = Some(Instant::now());
            if self.users_read.elapsed() > USERS_EVERY {
                self.users.refresh();
                self.users_read = Instant::now();
            }
            let mut units = HashMap::with_capacity(self.units.len());
            let table = self
                .system
                .processes()
                .values()
                .map(|process| {
                    let pid = process.pid().as_u32();
                    let started = process.start_time();
                    let unit = self
                        .units
                        .get(&(pid, started))
                        .cloned()
                        .unwrap_or_else(|| unit(pid));
                    units.insert((pid, started), unit.clone());
                    let parent = process.parent().map(Pid::as_u32);
                    let kernel = is_kernel(pid, parent);
                    let owner = process.user_id().map(|uid| **uid);
                    ProcessInfo {
                        pid,
                        parent,
                        name: process.name().to_string_lossy().into_owned(),
                        user: process
                            .user_id()
                            .and_then(|uid| self.users.get_user_by_id(uid))
                            .map(|user| user.name().to_owned()),
                        cpu: f64::from(process.cpu_usage()),
                        memory: process.memory(),
                        started,
                        unit,
                        kernel,
                        system: kernel || owner.is_none_or(|uid| !self.people.contains(&uid)),
                        protected: false,
                    }
                })
                .collect();
            self.units = units;
            table
        }
    }

    /// What privd checks before it stops a process.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Target {
        /// Its user's ID.
        pub owner: Option<u32>,
        pub started: u64,
        pub kernel: bool,
        pub unit: Option<String>,
    }

    /// The process `pid` as privd needs to judge it, if it's running.
    pub fn target(pid: u32) -> Option<Target> {
        let mut system = System::new();
        let os_pid = Pid::from_u32(pid);
        let kind = ProcessRefreshKind::nothing().with_user(UpdateKind::Always);
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[os_pid]), true, kind);
        let process = system.process(os_pid)?;
        if process.status() == ProcessStatus::Zombie {
            return None;
        }
        Some(Target {
            owner: process.user_id().map(|uid| **uid),
            started: process.start_time(),
            kernel: is_kernel(pid, process.parent().map(Pid::as_u32)),
            unit: unit(pid),
        })
    }

    /// Stops `pid` if it's still the process that started at `started`:
    /// SIGTERM and up to five seconds to exit, or with `force`, SIGKILL.
    pub fn stop(pid: u32, started: u64, force: bool) -> Result<StopResult, HostError> {
        let Some(os_pid) = i32::try_from(pid).ok().and_then(OsPid::from_raw) else {
            return Err(HostError::Invalid(format!("{pid} isn't a process ID")));
        };
        let (signal, wait) = if force {
            (Signal::KILL, KILL_WAIT)
        } else {
            (Signal::TERM, TERM_WAIT)
        };
        signal_and_wait(pid, os_pid, started, signal, wait)
    }

    /// Through a pidfd, which pins the process: once its start time checks out,
    /// the signal can only reach it, and the descriptor turns readable when it
    /// exits. Kernels before 5.3 have no pidfd and get the plain path.
    #[cfg(target_os = "linux")]
    fn signal_and_wait(
        pid: u32,
        os_pid: OsPid,
        started: u64,
        signal: Signal,
        wait: Duration,
    ) -> Result<StopResult, HostError> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        use rustix::process::{PidfdFlags, pidfd_open, pidfd_send_signal};

        let pidfd = match pidfd_open(os_pid, PidfdFlags::empty()) {
            Ok(pidfd) => pidfd,
            Err(Errno::SRCH) => return Ok(StopResult::NotRunning),
            Err(Errno::NOSYS) => return plain_signal_and_wait(pid, os_pid, started, signal, wait),
            Err(e) => return Err(HostError::Failed(format!("can't open process {pid}: {e}"))),
        };
        if start_time(pid) != Some(started) {
            return Ok(StopResult::NotRunning);
        }
        match pidfd_send_signal(&pidfd, signal) {
            Ok(()) => {}
            Err(Errno::SRCH) => return Ok(StopResult::NotRunning),
            Err(e) => {
                return Err(HostError::Failed(format!(
                    "can't signal process {pid}: {e}"
                )));
            }
        }
        let mut fds = [PollFd::new(&pidfd, PollFlags::IN)];
        let timeout = Timespec {
            tv_sec: i64::try_from(wait.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: 0,
        };
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => Ok(StopResult::StillRunning),
            Ok(_) => Ok(StopResult::Stopped),
            Err(e) => Err(HostError::Failed(format!(
                "can't wait for process {pid}: {e}"
            ))),
        }
    }

    #[cfg(target_os = "macos")]
    fn signal_and_wait(
        pid: u32,
        os_pid: OsPid,
        started: u64,
        signal: Signal,
        wait: Duration,
    ) -> Result<StopResult, HostError> {
        plain_signal_and_wait(pid, os_pid, started, signal, wait)
    }

    /// Checks the start time, signals, and watches for the process to go. A
    /// PID could be reused between the check and the signal, a window the
    /// pidfd path closes.
    fn plain_signal_and_wait(
        pid: u32,
        os_pid: OsPid,
        started: u64,
        signal: Signal,
        wait: Duration,
    ) -> Result<StopResult, HostError> {
        if start_time(pid) != Some(started) {
            return Ok(StopResult::NotRunning);
        }
        match rustix::process::kill_process(os_pid, signal) {
            Ok(()) => {}
            Err(Errno::SRCH) => return Ok(StopResult::NotRunning),
            Err(e) => {
                return Err(HostError::Failed(format!(
                    "can't signal process {pid}: {e}"
                )));
            }
        }
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            if start_time(pid) != Some(started) {
                return Ok(StopResult::Stopped);
            }
        }
        Ok(StopResult::StillRunning)
    }

    /// When `pid` started, if it's running and not a zombie.
    fn start_time(pid: u32) -> Option<u64> {
        target(pid).map(|target| target.started)
    }

    /// Linux's kernel threads all descend from kthreadd, PID 2; macOS has
    /// `kernel_task`, PID 0.
    fn is_kernel(pid: u32, parent: Option<u32>) -> bool {
        if cfg!(target_os = "linux") {
            pid == 2 || parent == Some(2)
        } else {
            pid == 0
        }
    }

    #[cfg(target_os = "linux")]
    fn unit(pid: u32) -> Option<String> {
        let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
        super::parse_unit(&cgroup)
    }

    #[cfg(target_os = "macos")]
    fn unit(_pid: u32) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, cpu: f64, memory: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent: Some(1),
            name: name.to_owned(),
            user: Some("www-data".to_owned()),
            cpu,
            memory,
            started: 1_791_000_000,
            unit: (name == "nginx").then(|| "nginx.service".to_owned()),
            kernel: false,
            // node runs as a person; the rest as system accounts.
            system: name != "node",
            protected: false,
        }
    }

    fn table() -> Vec<ProcessInfo> {
        vec![
            process(10, "nginx", 2.0, 300),
            process(11, "postgres", 40.0, 900),
            process(12, "sshd", 0.0, 100),
            process(13, "node", 40.0, 500),
        ]
    }

    fn pids(sample: &ProcessesSample) -> Vec<u32> {
        sample.processes.iter().map(|process| process.pid).collect()
    }

    #[test]
    fn sorts_by_cpu_then_memory_and_limits() {
        let params = ProcessesParams {
            limit: Some(3),
            ..ProcessesParams::default()
        };
        let sample = view(&table(), &params, 5);
        assert_eq!(pids(&sample), [11, 13, 10]);
        assert_eq!((sample.total, sample.matched, sample.ts), (4, 4, 5));
    }

    #[test]
    fn sorts_by_memory() {
        let params = ProcessesParams {
            sort: ProcessSort::Memory,
            ..ProcessesParams::default()
        };
        assert_eq!(pids(&view(&table(), &params, 0)), [11, 13, 10, 12]);
    }

    #[test]
    fn searches_names_units_users_and_pids() {
        let search = |query: &str| {
            let params = ProcessesParams {
                query: Some(query.to_owned()),
                ..ProcessesParams::default()
            };
            pids(&view(&table(), &params, 0))
        };
        assert_eq!(search("POST"), [11]);
        assert_eq!(search("nginx.serv"), [10]);
        assert_eq!(search("12"), [12]);
        assert_eq!(search("www-data").len(), 4);
        assert_eq!(search("  ").len(), 4);
        assert!(search("nothing").is_empty());
    }

    #[test]
    fn keeps_the_systems_or_peoples_processes_before_the_limit() {
        let owned = |owner: ProcessOwner, query: Option<&str>| {
            let params = ProcessesParams {
                owner: Some(owner),
                query: query.map(str::to_owned),
                limit: Some(1),
                ..ProcessesParams::default()
            };
            let sample = view(&table(), &params, 0);
            (pids(&sample), sample.matched)
        };
        // node ties postgres on CPU, but only node is a person's.
        assert_eq!(owned(ProcessOwner::User, None), (vec![13], 1));
        assert_eq!(owned(ProcessOwner::System, None), (vec![11], 3));
        assert_eq!(owned(ProcessOwner::System, Some("node")), (vec![], 0));
    }

    #[test]
    fn people_are_login_defs_range_on_linux() {
        assert_eq!(linux_people(""), 1000..=60000);
        let defs = "# comment\nUID_MIN\t\t 500\nUID_MAX   59999\nSYS_UID_MIN 101\n";
        assert_eq!(linux_people(defs), 500..=59999);
        assert_eq!(linux_people("UID_MIN nonsense\n"), 1000..=60000);
        assert!(!MAC_PEOPLE.contains(&0) && !MAC_PEOPLE.contains(&500));
        assert!(MAC_PEOPLE.contains(&501) && !MAC_PEOPLE.contains(&(u32::MAX - 1)));
    }

    #[test]
    fn a_limit_is_clamped() {
        let params = ProcessesParams {
            limit: Some(0),
            ..ProcessesParams::default()
        };
        assert_eq!(view(&table(), &params, 0).processes.len(), 1);
    }

    #[test]
    fn finds_the_service_in_a_cgroup() {
        assert_eq!(
            parse_unit("0::/system.slice/nginx.service\n").as_deref(),
            Some("nginx.service")
        );
        let user = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/sync.service";
        assert_eq!(parse_unit(user).as_deref(), Some("sync.service"));
        let v1 = "12:pids:/system.slice/cron.service\n1:name=systemd:/system.slice/cron.service\n";
        assert_eq!(parse_unit(v1).as_deref(), Some("cron.service"));
        assert_eq!(
            parse_unit("0::/user.slice/user-1000.slice/session-3.scope"),
            None
        );
        assert_eq!(parse_unit("0::/"), None);
    }
}
