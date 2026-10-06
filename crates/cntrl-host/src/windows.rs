//! The Windows backend (D58): host stats and system info through sysinfo, as
//! on macOS, with CPU time from `GetSystemTimes`, GPUs from the graphics
//! kernel (D59), and what identifies the machine from the registry. Windows
//! keeps no load average, and its temperatures aren't read yet (angle 14).
//!
//! Windows' calls are C, so this module may use `unsafe`; each block says why
//! it's sound.
#![allow(unsafe_code)]

use std::ptr::null_mut;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use cntrl_protocol::stats::{Filesystem, GpuStats, LoadAverage, MemoryStats, SwapStats};
use cntrl_protocol::system::{CpuInfo, OsInfo, SystemInfo};
use sysinfo::{DiskRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind};
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, FILETIME};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows_sys::Win32::System::Threading::GetSystemTimes;

use crate::HostError;
use crate::stats::{Background, CpuTicks, DiskCounters, NetworkCounters, Stats, StatsReading};
use crate::system::System;

pub mod eventlog;
mod gpu;
pub mod network;
pub(crate) mod power;
pub mod services;
pub mod storage;

/// How often filesystems and GPUs are looked at while someone watches.
const FILESYSTEMS_EVERY: Duration = Duration::from_secs(10);
const GPUS_EVERY: Duration = Duration::from_secs(2);
const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const BIOS: &str = r"HARDWARE\DESCRIPTION\System\BIOS";

/// Host stats.
pub struct WinStats {
    state: Mutex<State>,
    filesystems: Background<Vec<Filesystem>>,
    gpus: Background<Vec<GpuStats>>,
    engine_times: Arc<gpu::EngineTimes>,
}

struct State {
    system: sysinfo::System,
    disks: Disks,
    networks: Networks,
}

impl Default for WinStats {
    fn default() -> Self {
        let refresh =
            RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram().with_swap());
        Self {
            state: Mutex::new(State {
                system: sysinfo::System::new_with_specifics(refresh),
                disks: Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing()),
                networks: Networks::new_with_refreshed_list(),
            }),
            filesystems: Background::new(FILESYSTEMS_EVERY),
            gpus: Background::new(GPUS_EVERY),
            engine_times: Arc::default(),
        }
    }
}

impl Stats for WinStats {
    fn read(&self) -> Result<StatsReading, HostError> {
        let cpu = cpu_ticks()?;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let State {
            system,
            disks,
            networks,
        } = &mut *state;
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());
        let total = system.total_memory();
        if total == 0 {
            return Err(HostError::Failed("Windows reported no memory".to_owned()));
        }
        Ok(StatsReading {
            cpu,
            // Windows has none; Console doesn't show it.
            load: LoadAverage::default(),
            memory: MemoryStats {
                total,
                available: system.available_memory(),
            },
            swap: (system.total_swap() > 0).then(|| SwapStats {
                total: system.total_swap(),
                used: system.used_swap(),
            }),
            disk: disk_counters(disks),
            network: network_counters(networks),
            filesystems: self.filesystems.get(filesystems),
            temperatures: Vec::new(),
            gpus: self.gpus.get({
                let times = Arc::clone(&self.engine_times);
                move || gpu::gpus(&times)
            }),
        })
    }
}

/// CPU time across every core since boot, in 100 ns ticks. Kernel time
/// includes the idle time.
fn cpu_ticks() -> Result<CpuTicks, HostError> {
    let [mut idle, mut kernel, mut user] = [FILETIME::default(); 3];
    // SAFETY: three times to fill.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return Err(HostError::Failed(format!(
            "can't read CPU time: {}",
            std::io::Error::last_os_error()
        )));
    }
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    let idle = ticks(idle);
    Ok(CpuTicks {
        busy: ticks(kernel)
            .saturating_sub(idle)
            .saturating_add(ticks(user)),
        idle,
    })
}

/// Bytes moved on the volumes: their counts are disjoint, so their sum is the
/// disks'.
fn disk_counters(disks: &mut Disks) -> Option<DiskCounters> {
    disks.refresh_specifics(true, DiskRefreshKind::nothing().with_io_usage());
    disks
        .iter()
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

/// Bytes moved on the network adapters sysinfo counts as hardware, but for
/// Hyper-V's virtual ones (`vEthernet`), whose traffic crosses a physical
/// adapter too.
fn network_counters(networks: &mut Networks) -> Option<NetworkCounters> {
    networks.refresh(true);
    networks
        .iter()
        .filter(|(name, _)| !name.starts_with("vEthernet"))
        .map(|(_, data)| NetworkCounters {
            received: data.total_received(),
            sent: data.total_transmitted(),
        })
        .reduce(|a, b| NetworkCounters {
            received: a.received.saturating_add(b.received),
            sent: a.sent.saturating_add(b.sent),
        })
}

/// Each drive with a size: `C:\` and the like, named by its label.
fn filesystems() -> Vec<Filesystem> {
    let disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage());
    disks
        .iter()
        .filter_map(|disk| {
            let (total, available) = (disk.total_space(), disk.available_space());
            let name = disk.name().to_string_lossy().into_owned();
            (total > 0).then(|| Filesystem {
                mount: disk.mount_point().to_string_lossy().into_owned(),
                name: (!name.is_empty()).then_some(name),
                kind: disk.file_system().to_string_lossy().into_owned(),
                total,
                used: total.saturating_sub(available),
                available,
            })
        })
        .collect()
}

/// The machine.
#[derive(Debug, Default)]
pub struct WinSystem;

impl System for WinSystem {
    fn info(&self, agent_version: &str) -> Result<SystemInfo, HostError> {
        let build = registry_string(CURRENT_VERSION, "CurrentBuildNumber");
        let revision = registry_dword(CURRENT_VERSION, "UBR");
        Ok(SystemInfo {
            hostname: hostname().unwrap_or_else(|| "unknown".to_owned()),
            os: OsInfo {
                id: "windows".to_owned(),
                // Such as "Windows 11 Pro" or "Windows Server 2025 Datacenter".
                name: sysinfo::System::long_os_version().unwrap_or_else(|| "Windows".to_owned()),
                // Such as "24H2".
                version: registry_string(CURRENT_VERSION, "DisplayVersion")
                    .or_else(|| registry_string(CURRENT_VERSION, "ReleaseId")),
            },
            arch: std::env::consts::ARCH.to_owned(),
            // The build and its update, such as "26100.4061".
            kernel: match (build, revision) {
                (Some(build), Some(revision)) => format!("{build}.{revision}"),
                (Some(build), None) => build,
                (None, _) => String::new(),
            },
            boot_time: sysinfo::System::boot_time(),
            agent_version: agent_version.to_owned(),
            machine: hardware().0.clone(),
            cpu: hardware().1.clone(),
            chassis: chassis(hardware().0.as_deref()),
        })
    }
}

/// The model and the CPU, read once: they don't change while the agent runs.
fn hardware() -> &'static (Option<String>, Option<CpuInfo>) {
    static HARDWARE: OnceLock<(Option<String>, Option<CpuInfo>)> = OnceLock::new();
    HARDWARE.get_or_init(|| (model(), cpu()))
}

/// The maker and model as the firmware says, such as `LENOVO 20XW` or
/// `Microsoft Corporation Virtual Machine`.
fn model() -> Option<String> {
    let maker = registry_string(BIOS, "SystemManufacturer");
    let product = registry_string(BIOS, "SystemProductName");
    match (maker, product) {
        (Some(maker), Some(product)) if product.starts_with(&maker) => Some(product),
        (Some(maker), Some(product)) => Some(format!("{maker} {product}")),
        (maker, product) => maker.or(product),
    }
}

fn cpu() -> Option<CpuInfo> {
    let threads = u32::try_from(std::thread::available_parallelism().ok()?.get()).ok()?;
    let cores = sysinfo::System::physical_core_count()
        .and_then(|cores| u32::try_from(cores).ok())
        .unwrap_or(threads);
    Some(CpuInfo {
        name: registry_string(
            r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "ProcessorNameString",
        )
        .map(|name| name.trim().to_owned()),
        cores,
        threads,
        performance_cores: None,
        efficiency_cores: None,
    })
}

/// The form factor in systemd's words: a virtual machine by its maker, a
/// server by its edition, a laptop by its battery, else a desktop.
fn chassis(model: Option<&str>) -> Option<String> {
    const HYPERVISORS: &[&str] = &[
        "Virtual Machine",
        "VMware",
        "VirtualBox",
        "QEMU",
        "Parallels",
        "Xen",
        "KVM",
        "Amazon EC2",
        "Google Compute Engine",
    ];
    let kind = if model.is_some_and(|model| HYPERVISORS.iter().any(|name| model.contains(name))) {
        "vm"
    } else if registry_string(
        r"SYSTEM\CurrentControlSet\Control\ProductOptions",
        "ProductType",
    )
    .is_some_and(|kind| kind != "WinNT")
    {
        "server"
    } else if has_battery() {
        "laptop"
    } else {
        "desktop"
    };
    Some(kind.to_owned())
}

fn has_battery() -> bool {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: a struct to fill.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return false;
    }
    // 128: no system battery; 255: unknown.
    !matches!(status.BatteryFlag, 128 | 255)
}

/// The computer's DNS host name.
pub fn hostname() -> Option<String> {
    sysinfo::System::host_name().filter(|name| !name.is_empty())
}

/// The ID Windows gives the installation, which stays until it's reinstalled.
pub fn machine_id() -> Option<String> {
    registry_string(r"SOFTWARE\Microsoft\Cryptography", "MachineGuid")
}

/// Windows' count of its boots, which changes with each.
pub fn boot_id() -> Option<String> {
    registry_dword(
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management\PrefetchParameters",
        "BootId",
    )
    .map(|count| count.to_string())
}

/// A handle, closed when dropped.
struct Handle(windows_sys::Win32::Foundation::HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: an open handle that this owns.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// A string value under `HKEY_LOCAL_MACHINE`.
fn registry_string(key: &str, value: &str) -> Option<String> {
    let (key, value) = (wide(key), wide(value));
    let mut size = 0u32;
    // SAFETY: a size query.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: the buffer holds `size` bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length])).filter(|text| !text.trim().is_empty())
}

/// A number value under `HKEY_LOCAL_MACHINE`.
fn registry_dword(key: &str, value: &str) -> Option<u32> {
    let (key, value) = (wide(key), wide(value));
    let mut number = 0u32;
    let mut size = 4u32;
    // SAFETY: four bytes to fill.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&raw mut number).cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_machine() {
        let stats = WinStats::default();
        let first = stats.read().expect("a reading");
        assert!(first.memory.total > 0);
        assert!(first.memory.available <= first.memory.total);
        assert!(first.cpu.busy + first.cpu.idle > 0);
        std::thread::sleep(Duration::from_millis(200));
        let second = stats.read().expect("another reading");
        assert!(second.cpu.idle >= first.cpu.idle);

        let info = WinSystem.info("0.0.0").expect("system info");
        assert_eq!(info.os.id, "windows");
        assert!(info.os.name.starts_with("Windows"), "{}", info.os.name);
        assert!(!info.kernel.is_empty());
        assert!(info.boot_time > 0);
        let cpu = info.cpu.expect("a CPU");
        assert!(cpu.threads >= cpu.cores && cpu.cores > 0);
        assert!(info.chassis.is_some());

        assert!(hostname().is_some());
        assert_eq!(machine_id().map(|id| id.len()), Some(36));
        assert!(boot_id().is_some(), "Windows' boot count");
    }

    #[test]
    fn power_lists_restart_and_shutdown() {
        let actions = power_actions();
        assert!(actions.contains(&cntrl_protocol::power::PowerAction::Reboot));
        assert!(actions.contains(&cntrl_protocol::power::PowerAction::Poweroff));
    }

    fn power_actions() -> Vec<cntrl_protocol::power::PowerAction> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime");
        runtime.block_on(power::info()).expect("power info").actions
    }
}
