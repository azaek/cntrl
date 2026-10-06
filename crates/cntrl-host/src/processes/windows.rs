//! Windows' process table (D58). sysinfo reads it, as on macOS; this module
//! names each process's account, which sysinfo finds only for local users,
//! and stops processes itself, since sysinfo would run `taskkill`. A stop
//! opens the process, which keeps its PID from being reused, checks its start
//! time, and ends it with TerminateProcess: from session 0 there's no asking
//! a program to close (angle 06). Windows' critical processes, whose end
//! stops the machine, are refused.
//!
//! Windows' calls are C, so this module may use `unsafe`; each block says why
//! it's sound.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use cntrl_protocol::process::{ProcessInfo, StopResult};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, LocalFree, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::{LookupAccountSidW, PSID, SID_NAME_USE};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, IsProcessCritical, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};

use crate::HostError;

/// A table read after a longer gap than this is read twice, a moment apart,
/// so its CPU figures cover a known span.
const STALE: Duration = Duration::from_secs(10);
/// How often account names are looked up again.
const NAMES_EVERY: Duration = Duration::from_secs(60);
/// How long a process gets to go once ended, which happens asynchronously.
const END_WAIT: Duration = Duration::from_secs(5);
/// Seconds from Windows' epoch, 1601, to Unix's.
const EPOCH_GAP: u64 = 11_644_473_600;

/// Reads the process table, keeping what CPU figures and names need between
/// reads.
pub struct Sampler {
    system: System,
    read: Option<Instant>,
    /// Account names by SID.
    names: HashMap<String, Option<String>>,
    names_read: Instant,
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
            read: None,
            names: HashMap::new(),
            names_read: Instant::now(),
        }
    }

    /// Every process, with CPU since the previous read. It blocks for a
    /// moment when the last read is stale.
    pub fn read(&mut self) -> Vec<ProcessInfo> {
        let kind = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_user(UpdateKind::OnlyIfNotSet);
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
        if self.names_read.elapsed() > NAMES_EVERY {
            self.names.clear();
            self.names_read = Instant::now();
        }
        let names = &mut self.names;
        self.system
            .processes()
            .values()
            .map(|process| {
                let pid = process.pid().as_u32();
                let parent = process.parent().map(Pid::as_u32);
                let user = process.user_id().and_then(|sid| {
                    names
                        .entry(sid.to_string())
                        .or_insert_with_key(|sid| account_name(sid))
                        .clone()
                });
                ProcessInfo {
                    pid,
                    parent,
                    name: process.name().to_string_lossy().into_owned(),
                    user,
                    cpu: f64::from(process.cpu_usage()),
                    memory: process.memory(),
                    started: process.start_time(),
                    unit: None,
                    kernel: is_kernel(pid, parent),
                    protected: false,
                }
            })
            .collect()
    }
}

/// What privd checks before it stops a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Its account's SID.
    pub owner: Option<String>,
    pub started: u64,
    /// Part of Windows: the kernel's own, or one whose end stops the machine.
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
    let parent = process.parent().map(Pid::as_u32);
    Some(Target {
        owner: process.user_id().map(|sid| sid.to_string()),
        started: process.start_time(),
        kernel: is_kernel(pid, parent)
            || Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)
                .is_ok_and(|process| process.critical()),
        unit: None,
    })
}

/// Ends `pid` if it's still the process that started at `started`, at once:
/// Windows has no asking a process to exit from a service, so only `force`
/// stops one.
pub fn stop(pid: u32, started: u64, force: bool) -> Result<StopResult, HostError> {
    if !force {
        return Err(HostError::Invalid(
            "Windows can't ask a process to exit from a service; force stopping ends it at once, losing anything unsaved".to_owned(),
        ));
    }
    let access = PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
    let process = match Process::open(pid, access) {
        Ok(process) => process,
        // No such process.
        Err(e) if e.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) => {
            return Ok(StopResult::NotRunning);
        }
        Err(e) => return Err(HostError::Failed(format!("can't open process {pid}: {e}"))),
    };
    if process.start_time() != Some(started) || process.exited_within(Duration::ZERO) {
        return Ok(StopResult::NotRunning);
    }
    if process.critical() {
        return Err(HostError::Invalid(format!(
            "process {pid} is part of Windows, which stops with it"
        )));
    }
    // SAFETY: an open handle with PROCESS_TERMINATE.
    if unsafe { TerminateProcess(process.0, 1) } == 0 {
        let e = std::io::Error::last_os_error();
        if process.exited_within(Duration::ZERO) {
            return Ok(StopResult::NotRunning);
        }
        return Err(HostError::Failed(format!("can't end process {pid}: {e}")));
    }
    if process.exited_within(END_WAIT) {
        Ok(StopResult::Stopped)
    } else {
        Ok(StopResult::StillRunning)
    }
}

/// The System Idle Process, PID 0, System, PID 4, and what System starts:
/// Registry, Memory Compression, smss.
fn is_kernel(pid: u32, parent: Option<u32>) -> bool {
    pid == 0 || pid == 4 || parent == Some(4)
}

/// An open process, closed when dropped. While it's open, its PID names it
/// and no other.
struct Process(HANDLE);

impl Process {
    fn open(pid: u32, access: u32) -> std::io::Result<Self> {
        // SAFETY: a plain call; the handle it gives is closed on drop.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    /// When it started, in Unix seconds, as sysinfo counts.
    fn start_time(&self) -> Option<u64> {
        let [mut created, mut exited, mut kernel, mut user] = [FILETIME::default(); 4];
        // SAFETY: an open handle with PROCESS_QUERY_LIMITED_INFORMATION, and
        // four times to fill.
        let read =
            unsafe { GetProcessTimes(self.0, &mut created, &mut exited, &mut kernel, &mut user) };
        if read == 0 {
            return None;
        }
        let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        (ticks / 10_000_000).checked_sub(EPOCH_GAP)
    }

    /// Whether ending it stops Windows, as ending csrss does.
    fn critical(&self) -> bool {
        let mut critical = 0;
        // SAFETY: an open handle with PROCESS_QUERY_LIMITED_INFORMATION.
        unsafe { IsProcessCritical(self.0, &mut critical) != 0 && critical != 0 }
    }

    /// Whether it has exited, waiting up to `wait`.
    fn exited_within(&self, wait: Duration) -> bool {
        let millis = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: an open handle with SYNCHRONIZE.
        unsafe { WaitForSingleObject(self.0, millis) == WAIT_OBJECT_0 }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: an open handle that this owns.
        unsafe { CloseHandle(self.0) };
    }
}

/// An account's name from its SID, without its domain, as Task Manager
/// shows it: `SYSTEM`, `cntrl-agent`.
fn account_name(sid: &str) -> Option<String> {
    let wide: Vec<u16> = sid.encode_utf16().chain(Some(0)).collect();
    let mut binary: PSID = null_mut();
    // SAFETY: the string is NUL-terminated; the SID made from it is freed
    // below.
    if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut binary) } == 0 {
        return None;
    }
    let mut name = [0u16; 257];
    let mut domain = [0u16; 257];
    let (mut name_length, mut domain_length) = (257u32, 257u32);
    let mut kind: SID_NAME_USE = 0;
    // SAFETY: the buffers hold the lengths given, and `binary` is a valid SID.
    let found = unsafe {
        LookupAccountSidW(
            null(),
            binary,
            name.as_mut_ptr(),
            &mut name_length,
            domain.as_mut_ptr(),
            &mut domain_length,
            &mut kind,
        )
    };
    // SAFETY: ConvertStringSidToSidW allocated it with LocalAlloc.
    unsafe { LocalFree(binary) };
    if found == 0 {
        return None;
    }
    let length = usize::try_from(name_length).ok()?;
    name.get(..length).map(String::from_utf16_lossy)
}
