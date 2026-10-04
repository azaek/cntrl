//! Types for `power.*` operations (angle 10). `power.info` tells Console what
//! the machine can do, what an action would interrupt and whether the machine
//! comes back; the actions start a restart, shutdown, sleep or hibernation and
//! answer before the machine goes.

use serde::{Deserialize, Serialize};

/// A power action, by the name of its operation's second part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PowerAction {
    Reboot,
    Poweroff,
    Suspend,
    Hibernate,
}

impl PowerAction {
    pub const ALL: [PowerAction; 4] =
        [Self::Reboot, Self::Poweroff, Self::Suspend, Self::Hibernate];

    /// Its operation and the capability that allows it, which share a name.
    pub fn op(self) -> &'static str {
        match self {
            Self::Reboot => "power.reboot",
            Self::Poweroff => "power.poweroff",
            Self::Suspend => "power.suspend",
            Self::Hibernate => "power.hibernate",
        }
    }
}

/// What `power.info` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PowerInfo {
    /// The actions this machine can take. One can be held off by an inhibitor
    /// and still be listed.
    pub actions: Vec<PowerAction>,
    /// Who's signed in, at the machine or remotely.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<Session>,
    /// Programs holding off a shutdown or sleep, through logind on Linux.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inhibitors: Vec<Inhibitor>,
    /// What a restart waits for before the machine can reconnect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlock_after_restart: Option<DiskUnlock>,
    /// Whether the machine starts by itself after a power cut, where the agent
    /// can tell (macOS's `autorestart`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarts_after_power_loss: Option<bool>,
    /// The wired interface a magic packet would wake, where there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_on_lan: Option<WakeOnLan>,
}

/// A signed-in user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Session {
    pub user: String,
    /// Where: a seat or terminal, such as `seat0` or `console`, or the address
    /// a remote login came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    pub remote: bool,
}

/// A program holding off a shutdown or sleep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Inhibitor {
    /// The program, such as `GNOME Software`.
    pub who: String,
    pub why: String,
    /// What it holds off, such as `shutdown` and `sleep`.
    pub what: Vec<String>,
    /// `block` holds the action off; `delay` only delays it a few seconds.
    pub mode: String,
}

/// Why a restarted machine waits before it can reconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DiskUnlock {
    /// FileVault: the Mac waits at the unlock screen for a password.
    FileVault,
    /// Linux's root filesystem is encrypted, so the machine may wait for a
    /// passphrase at boot, unless a TPM or network unlock is set up.
    EncryptedRoot,
}

/// Wake-on-LAN on a wired interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WakeOnLan {
    /// Such as `eno1` or `en0`.
    pub interface: String,
    /// Where a magic packet goes, such as `d0:11:e5:73:0e:f0`.
    pub mac: String,
    /// Whether it's set to wake the machine; absent where the agent can't
    /// tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// What a power action returns: that it has started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PowerStarted {
    pub action: PowerAction,
}
