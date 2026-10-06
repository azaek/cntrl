//! Policy capabilities: what a device's policy can allow. Every operation and
//! topic names one, and a device policy lists the ones it allows.

/// Every capability of protocol v1.
pub const CAPABILITIES: &[&str] = &[
    "system.read",
    "processes.read",
    "processes.signal",
    "power.read",
    "power.reboot",
    "power.poweroff",
    "power.suspend",
    "power.hibernate",
    "services.read",
    "services.manage",
    "logs.read",
    "network.read",
    "history.manage",
];

/// What a device allows when it has no policy file: monitoring only.
pub const MONITOR_ONLY: &[&str] = &[
    "system.read",
    "processes.read",
    "power.read",
    "services.read",
    "logs.read",
    "network.read",
];

/// Whether `name` is a capability of this protocol version.
pub fn is_capability(name: &str) -> bool {
    CAPABILITIES.contains(&name)
}
