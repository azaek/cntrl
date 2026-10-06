//! Power (angle 10): what the machine can do, what an action would interrupt
//! and whether it comes back, for `power.info`, which needs no root; and the
//! actions themselves, which only privd takes, as root. Linux goes through
//! logind, falling back to `systemctl` where logind isn't running, as in a
//! container; macOS through `shutdown` and `pmset`; Windows through
//! InitiateShutdownW and SetSuspendState (`windows::power`). The parsers are
//! plain Rust, so they build and their tests run on any OS.

#[cfg(any(target_os = "linux", all(test, unix)))]
use std::fs;
#[cfg(any(target_os = "linux", all(test, unix)))]
use std::path::Path;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
use cntrl_protocol::power::PowerAction;
#[cfg(any(target_os = "macos", test))]
use cntrl_protocol::power::Session;
#[cfg(any(target_os = "linux", all(test, unix)))]
use cntrl_protocol::power::WakeOnLan;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
use crate::HostError;
#[cfg(any(target_os = "linux", all(test, unix)))]
use crate::hwmon::{entries, read};

#[cfg(any(target_os = "macos", test))]
/// `pmset -g`'s `womp` (wake on magic packet) and `autorestart` (start after a
/// power cut), each a line such as ` womp                 1`.
pub(crate) fn pmset_flags(text: &str) -> (Option<bool>, Option<bool>) {
    let flag = |key: &str| {
        text.lines().find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next() == Some(key)).then(|| words.next() == Some("1"))
        })
    };
    (flag("womp"), flag("autorestart"))
}

#[cfg(any(target_os = "macos", test))]
/// Who's signed in, from `who`: one entry per user at the Mac, and one per
/// user and address for remote logins. Terminal windows are local logins too,
/// so they fold into the user's.
pub(crate) fn who_sessions(text: &str) -> Vec<Session> {
    let mut sessions: Vec<Session> = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(user), Some(tty)) = (words.next(), words.next()) else {
            continue;
        };
        let host = line
            .rfind('(')
            .zip(line.rfind(')'))
            .filter(|(open, close)| open < close)
            .map(|(open, close)| line[open + 1..close].to_owned())
            .filter(|host| !host.is_empty());
        let session = Session {
            user: user.to_owned(),
            place: host
                .clone()
                .or_else(|| (tty == "console").then(|| "console".to_owned())),
            remote: host.is_some(),
        };
        let same = |s: &Session| s.user == session.user && s.remote == session.remote;
        match sessions
            .iter_mut()
            .find(|s| same(s) && (!s.remote || s.place == session.place))
        {
            Some(existing) if existing.place.is_none() => existing.place = session.place,
            Some(_) => {}
            None => sessions.push(session),
        }
    }
    sessions
}

#[cfg(any(target_os = "macos", test))]
/// Wired ports and their MAC addresses from `networksetup
/// -listallhardwareports`, which, unlike `ifconfig`, gives an idle port's real
/// address.
pub(crate) fn wired_ports(text: &str) -> Vec<(String, String)> {
    let mut ports = Vec::new();
    let mut port = None::<String>;
    let mut device = None::<String>;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix("Hardware Port: ") {
            port = Some(name.to_owned());
            device = None;
        } else if let Some(name) = line.strip_prefix("Device: ") {
            device = Some(name.to_owned());
        } else if let Some(mac) = line.strip_prefix("Ethernet Address: ") {
            let wired = port.as_deref().is_some_and(|p| {
                (p.contains("Ethernet") || p.contains("LAN")) && !p.contains("Bridge")
            });
            if let (true, Some(device)) = (wired, device.take()) {
                ports.push((device, mac.to_owned()));
            }
        }
    }
    ports
}

#[cfg(any(target_os = "linux", all(test, unix)))]
/// The wired interface a magic packet would wake on Linux, from sysfs: one
/// with a device behind it that isn't Wi-Fi, up first. Its `power/wakeup`
/// follows its Wake-on-LAN setting in most drivers (angle 10).
pub(crate) fn linux_wake_on_lan(sys: &Path) -> Option<WakeOnLan> {
    let wired: Vec<_> = entries(&sys.join("class/net"))
        .into_iter()
        .filter(|net| {
            net.join("device").exists()
                && !net.join("wireless").exists()
                && !net.join("phy80211").exists()
        })
        .collect();
    let up = |net: &&std::path::PathBuf| read(&net.join("operstate")).as_deref() == Some("up");
    let chosen = wired.iter().find(up).or_else(|| wired.first())?;
    Some(WakeOnLan {
        interface: chosen.file_name()?.to_string_lossy().into_owned(),
        mac: read(&chosen.join("address")).filter(|mac| !mac.is_empty())?,
        enabled: match read(&chosen.join("device/power/wakeup")).as_deref() {
            Some("enabled") => Some(true),
            Some("disabled") => Some(false),
            _ => None,
        },
    })
}

#[cfg(any(target_os = "linux", all(test, unix)))]
/// Whether Linux's root filesystem sits on dm-crypt, directly or under LVM or
/// RAID, so a restart may wait for a passphrase.
pub(crate) fn linux_encrypted_root(sys: &Path, mountinfo: &str) -> bool {
    let Some(device) = mountinfo.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        (fields.get(4) == Some(&"/"))
            .then(|| fields.get(2).map(|d| (*d).to_owned()))
            .flatten()
    }) else {
        return false;
    };
    encrypted(&sys.join("dev/block").join(device), 0)
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn encrypted(block: &Path, depth: u32) -> bool {
    if read(&block.join("dm/uuid")).is_some_and(|uuid| uuid.starts_with("CRYPT-")) {
        return true;
    }
    depth < 4
        && fs::read_dir(block.join("slaves")).is_ok_and(|slaves| {
            slaves
                .flatten()
                .any(|slave| encrypted(&block.join("slaves").join(slave.file_name()), depth + 1))
        })
}

#[cfg(any(target_os = "linux", test))]
/// What logind's `Can*` answers mean: whether the action exists here, and
/// whether something holds it off now.
pub(crate) fn can(answer: &str) -> (bool, bool) {
    match answer {
        "yes" | "challenge" => (true, false),
        "inhibited" | "inhibitor-blocked" | "challenge-inhibitor-blocked" => (true, true),
        _ => (false, false),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub(crate) fn unavailable(action: PowerAction) -> HostError {
    let what = match action {
        PowerAction::Reboot => "restart",
        PowerAction::Poweroff => "shut down",
        PowerAction::Suspend => "sleep",
        PowerAction::Hibernate => "hibernate",
    };
    HostError::Failed(format!("this machine can't {what}"))
}

#[cfg(target_os = "linux")]
pub use linux::{act, check, info};

#[cfg(target_os = "macos")]
pub use macos::{act, check, info};

#[cfg(windows)]
pub use crate::windows::power::{act, check, info};

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;

    use cntrl_protocol::power::{DiskUnlock, Inhibitor, PowerAction, PowerInfo, Session};
    use zbus_systemd::login1::{ManagerProxy, SessionProxy};

    use super::{can, linux_encrypted_root, linux_wake_on_lan, unavailable};
    use crate::HostError;

    /// logind honours `block` inhibitors for root too (systemd 257 does by
    /// default; older ones with this flag).
    const ROOT_CHECK_INHIBITORS: u64 = 0x01;

    async fn logind() -> Result<(zbus::Connection, ManagerProxy<'static>), HostError> {
        let connection = zbus::Connection::system().await.map_err(failed)?;
        let manager = ManagerProxy::new(&connection).await.map_err(failed)?;
        Ok((connection, manager))
    }

    /// logind's `Can*` answer, or none: some machines answer an error rather
    /// than `na`, as `CanHibernate` does without a place to hibernate to, and
    /// without polkit logind refuses to answer anyone but root.
    async fn answer(manager: &ManagerProxy<'_>, action: PowerAction) -> Option<String> {
        match action {
            PowerAction::Reboot => manager.can_reboot().await,
            PowerAction::Poweroff => manager.can_power_off().await,
            PowerAction::Suspend => manager.can_suspend().await,
            PowerAction::Hibernate => manager.can_hibernate().await,
        }
        .ok()
    }

    /// Whether an action exists when logind won't say, as for the agent's
    /// unprivileged user on a machine without polkit: restarting and shutting
    /// down always do, sleep where the kernel lists a sleep state, and
    /// hibernation only when logind says so. privd asks again as root before
    /// acting.
    fn assumed(action: PowerAction) -> bool {
        match action {
            PowerAction::Reboot | PowerAction::Poweroff => true,
            PowerAction::Suspend => {
                std::fs::read_to_string("/sys/power/state").is_ok_and(|states| {
                    states
                        .split_whitespace()
                        .any(|s| s == "mem" || s == "freeze")
                })
            }
            PowerAction::Hibernate => false,
        }
    }

    pub async fn info() -> Result<PowerInfo, HostError> {
        let (connection, manager) = logind().await?;
        let mut actions = Vec::new();
        for action in PowerAction::ALL {
            let exists = match answer(&manager, action).await {
                Some(reply) => can(&reply).0,
                None => assumed(action),
            };
            if exists {
                actions.push(action);
            }
        }
        let mut sessions: Vec<Session> = Vec::new();
        for (_, _, user, _, path) in manager.list_sessions().await.map_err(failed)? {
            let Ok(session) = SessionProxy::builder(&connection)
                .path(path)
                .map_err(failed)?
                .build()
                .await
            else {
                continue;
            };
            if session.class().await.map_err(failed)? != "user" {
                continue;
            }
            let remote = session.remote().await.unwrap_or(false);
            let place = if remote {
                session.remote_host().await.ok()
            } else {
                session
                    .seat()
                    .await
                    .ok()
                    .map(|(seat, _)| seat)
                    .filter(|s| !s.is_empty())
                    .or(session.tty().await.ok())
            }
            .filter(|place| !place.is_empty());
            let session = Session {
                user,
                place,
                remote,
            };
            if !sessions.contains(&session) {
                sessions.push(session);
            }
        }
        let inhibitors = manager
            .list_inhibitors()
            .await
            .map_err(failed)?
            .into_iter()
            .filter_map(|(what, who, why, mode, _, _)| {
                let what: Vec<String> = what
                    .split(':')
                    .filter(|w| matches!(*w, "shutdown" | "sleep"))
                    .map(str::to_owned)
                    .collect();
                (!what.is_empty()).then_some(Inhibitor {
                    who,
                    why,
                    what,
                    mode,
                })
            })
            .collect();
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        Ok(PowerInfo {
            actions,
            sessions,
            inhibitors,
            unlock_after_restart: linux_encrypted_root(Path::new("/sys"), &mountinfo)
                .then_some(DiskUnlock::EncryptedRoot),
            restarts_after_power_loss: None,
            wake_on_lan: linux_wake_on_lan(Path::new("/sys")),
        })
    }

    /// Whether the action can go ahead now: it exists, and nothing holds it off.
    pub async fn check(action: PowerAction) -> Result<(), HostError> {
        let Ok((_, manager)) = logind().await else {
            // Without logind, systemctl will say.
            return Ok(());
        };
        match answer(&manager, action)
            .await
            .as_deref()
            .map(can)
            .unwrap_or((false, false))
        {
            (true, false) => Ok(()),
            (true, true) => {
                let holders: Vec<String> = manager
                    .list_inhibitors()
                    .await
                    .map_err(failed)?
                    .into_iter()
                    .filter(|(what, _, _, mode, _, _)| {
                        mode == "block" && what.split(':').any(|w| w == "shutdown" || w == "sleep")
                    })
                    .map(|(_, who, why, _, _, _)| format!("{who} ({why})"))
                    .collect();
                Err(HostError::Failed(format!(
                    "held off by {}",
                    holders.join(", ")
                )))
            }
            (false, _) => Err(unavailable(action)),
        }
    }

    pub async fn act(action: PowerAction) -> Result<(), HostError> {
        match logind().await {
            Ok((_, manager)) => match action {
                PowerAction::Reboot => manager.reboot_with_flags(ROOT_CHECK_INHIBITORS).await,
                PowerAction::Poweroff => manager.power_off_with_flags(ROOT_CHECK_INHIBITORS).await,
                PowerAction::Suspend => manager.suspend_with_flags(ROOT_CHECK_INHIBITORS).await,
                PowerAction::Hibernate => manager.hibernate_with_flags(ROOT_CHECK_INHIBITORS).await,
            }
            .map_err(failed),
            Err(_) => {
                let verb = match action {
                    PowerAction::Reboot => "reboot",
                    PowerAction::Poweroff => "poweroff",
                    PowerAction::Suspend => "suspend",
                    PowerAction::Hibernate => "hibernate",
                };
                let status = tokio::process::Command::new("systemctl")
                    .arg(verb)
                    .status()
                    .await
                    .map_err(|e| HostError::Failed(format!("systemctl {verb}: {e}")))?;
                if status.success() {
                    Ok(())
                } else {
                    Err(HostError::Failed(format!(
                        "systemctl {verb} failed ({status})"
                    )))
                }
            }
        }
    }

    fn failed(e: impl std::fmt::Display) -> HostError {
        HostError::Failed(e.to_string())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::process::Command;

    use cntrl_protocol::power::{DiskUnlock, PowerAction, PowerInfo, WakeOnLan};

    use super::{pmset_flags, unavailable, who_sessions, wired_ports};
    use crate::HostError;

    /// macOS restarts, shuts down and sleeps; it has no separate hibernate.
    const ACTIONS: [PowerAction; 3] = [
        PowerAction::Reboot,
        PowerAction::Poweroff,
        PowerAction::Suspend,
    ];

    pub async fn info() -> Result<PowerInfo, HostError> {
        tokio::task::spawn_blocking(|| {
            let settings = output("/usr/bin/pmset", &["-g"]).unwrap_or_default();
            let (womp, autorestart) = pmset_flags(&settings);
            let can_womp = output("/usr/bin/pmset", &["-g", "cap"])
                .is_some_and(|cap| cap.lines().any(|l| l.trim() == "womp"));
            let ports =
                output("/usr/sbin/networksetup", &["-listallhardwareports"]).unwrap_or_default();
            let filevault = output("/usr/bin/fdesetup", &["status"])
                .is_some_and(|status| status.contains("FileVault is On"));
            PowerInfo {
                actions: ACTIONS.to_vec(),
                sessions: who_sessions(&output("/usr/bin/who", &[]).unwrap_or_default()),
                inhibitors: Vec::new(),
                unlock_after_restart: filevault.then_some(DiskUnlock::FileVault),
                restarts_after_power_loss: autorestart,
                wake_on_lan: wired_ports(&ports)
                    .into_iter()
                    .next()
                    .map(|(interface, mac)| WakeOnLan {
                        interface,
                        mac,
                        enabled: if can_womp { womp } else { Some(false) },
                    }),
            }
        })
        .await
        .map_err(|e| HostError::Failed(e.to_string()))
    }

    pub async fn check(action: PowerAction) -> Result<(), HostError> {
        if ACTIONS.contains(&action) {
            Ok(())
        } else {
            Err(unavailable(action))
        }
    }

    /// Plain `shutdown` hands off to launchd for an orderly shutdown; apps are
    /// told to quit, not asked to save (angle 06 §3).
    pub async fn act(action: PowerAction) -> Result<(), HostError> {
        let (program, args): (&str, &[&str]) = match action {
            PowerAction::Reboot => ("/sbin/shutdown", &["-r", "now"]),
            PowerAction::Poweroff => ("/sbin/shutdown", &["-h", "now"]),
            PowerAction::Suspend => ("/usr/bin/pmset", &["sleepnow"]),
            PowerAction::Hibernate => return Err(unavailable(action)),
        };
        let status = tokio::process::Command::new(program)
            .args(args)
            .status()
            .await
            .map_err(|e| HostError::Failed(format!("{program}: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(HostError::Failed(format!("{program} failed ({status})")))
        }
    }

    fn output(program: &str, args: &[&str]) -> Option<String> {
        let output = Command::new(program).args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pmset() {
        let text = " standby              0\n autorestart          1\n sleep                0 (sleep prevented by powerd)\n womp                 1\n";
        assert_eq!(pmset_flags(text), (Some(true), Some(true)));
        assert_eq!(pmset_flags(" womp 0\n"), (Some(false), None));
    }

    #[test]
    fn folds_terminal_windows_into_the_local_login() {
        let text = "azaek    console      Oct  2 13:07 \nazaek    ttys000      Oct  3 13:55 \nazaek    ttys001      Oct  4 12:30 (10.0.0.5)\nguest    ttys002      Oct  4 12:31 \n";
        assert_eq!(
            who_sessions(text),
            vec![
                Session {
                    user: "azaek".to_owned(),
                    place: Some("console".to_owned()),
                    remote: false
                },
                Session {
                    user: "azaek".to_owned(),
                    place: Some("10.0.0.5".to_owned()),
                    remote: true
                },
                Session {
                    user: "guest".to_owned(),
                    place: None,
                    remote: false
                },
            ]
        );
    }

    #[test]
    fn finds_wired_ports_with_their_real_address() {
        let text = "\nHardware Port: Ethernet\nDevice: en0\nEthernet Address: d0:11:e5:73:0e:f0\n\nHardware Port: Wi-Fi\nDevice: en1\nEthernet Address: d0:11:e5:7e:42:69\n\nHardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: 36:44:ab:cd:00:01\n";
        assert_eq!(
            wired_ports(text),
            vec![("en0".to_owned(), "d0:11:e5:73:0e:f0".to_owned())]
        );
    }

    // A sysfs fixture, as Linux lays it out.
    #[test]
    #[cfg(unix)]
    fn reads_linux_wake_on_lan_and_disk_encryption() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sys = dir.path();
        let write = |path: &str, text: &str| {
            let path = sys.join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            fs::write(path, text).expect("write");
        };
        write("class/net/eno1/address", "a8:a1:59:12:34:56\n");
        write("class/net/eno1/operstate", "up\n");
        write("class/net/eno1/device/power/wakeup", "enabled\n");
        write("class/net/wlp2s0/address", "f4:4e:e3:00:00:01\n");
        write("class/net/wlp2s0/operstate", "up\n");
        write("class/net/wlp2s0/device/uevent", "");
        write("class/net/wlp2s0/wireless/x", "");
        write("class/net/docker0/address", "02:42:00:00:00:01\n");
        assert_eq!(
            linux_wake_on_lan(sys),
            Some(WakeOnLan {
                interface: "eno1".to_owned(),
                mac: "a8:a1:59:12:34:56".to_owned(),
                enabled: Some(true)
            })
        );
        // Root on LVM (dm-1) on LUKS (dm-0).
        write("dev/block/253:1/dm/uuid", "LVM-abc\n");
        write(
            "dev/block/253:1/slaves/dm-0/dm/uuid",
            "CRYPT-LUKS2-0123-root\n",
        );
        let mountinfo = "29 1 253:1 / / rw,relatime shared:1 - ext4 /dev/mapper/vg-root rw\n";
        assert!(linux_encrypted_root(sys, mountinfo));
        assert!(!linux_encrypted_root(
            sys,
            "29 1 8:2 / / rw shared:1 - ext4 /dev/sda2 rw\n"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_this_macs_power() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime");
        let info = runtime.block_on(info()).expect("power info");
        assert_eq!(
            info.actions,
            [
                PowerAction::Reboot,
                PowerAction::Poweroff,
                PowerAction::Suspend
            ]
        );
        assert!(!info.sessions.is_empty(), "someone runs these tests");
        assert!(info.restarts_after_power_loss.is_some());
        eprintln!("{info:?}");
    }

    #[test]
    fn reads_logind_answers() {
        assert_eq!(can("yes"), (true, false));
        assert_eq!(can("challenge"), (true, false));
        assert_eq!(can("inhibitor-blocked"), (true, true));
        assert_eq!(can("na"), (false, false));
    }
}
