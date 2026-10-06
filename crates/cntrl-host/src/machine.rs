//! What identifies the machine: its hostname, an ID that outlives the agent,
//! and one that changes with every boot. The agent reports them when it
//! enrolls and in each hello.

/// The machine's hostname.
pub fn hostname() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        read_trimmed("/proc/sys/kernel/hostname")
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::hostname()
    }
    #[cfg(windows)]
    {
        crate::windows::hostname()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// An ID that stays with the machine: systemd's machine ID on Linux, the
/// hardware UUID on macOS, the installation's MachineGuid on Windows. Callers
/// hash it before it leaves the machine.
pub fn machine_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        read_trimmed("/etc/machine-id")
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::machine_id()
    }
    #[cfg(windows)]
    {
        crate::windows::machine_id()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// An ID that changes on every boot.
pub fn boot_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        read_trimmed("/proc/sys/kernel/random/boot_id")
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::boot_id()
    }
    #[cfg(windows)]
    {
        crate::windows::boot_id()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}
