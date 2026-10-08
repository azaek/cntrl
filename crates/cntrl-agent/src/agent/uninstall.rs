//! `cntrl uninstall` on macOS and Linux (D87). The running agent tells Console
//! first, over its link, then the services, the program and the `cntrl`
//! command go; with `--purge`, the identity, config, policy, audit log and logs,
//! and the agent's user, too. What it removes is a list of steps, so the list
//! is what's tested. Windows has its own in `install.rs`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use super::client;
use super::config::Config;
use super::os;
use super::say::{say, say_err};

/// Where the install script puts the program on Linux, and the units.
const LINUX_PROGRAM: &str = "/usr/local/bin/cntrl-agent";
const LINUX_UNITS: &str = "/etc/systemd/system";
const UNITS: [&str; 3] = [
    "cntrl-agent.service",
    "cntrl-privd.service",
    "cntrl-privd.socket",
];
/// The `cntrl` command, a link to the program on both.
const COMMAND: &str = "/usr/local/bin/cntrl";
/// Where the program lives on macOS, outside /usr/local, and its launchd jobs.
const MAC_SUPPORT: &str = "/Library/Application Support/cntrl";
const LABELS: [&str; 2] = ["pw.cntrl.agent", "pw.cntrl.privd"];

/// One step of removing the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// A command. A failure is a warning, unless `quiet`: stopping a service
    /// that's gone already, or deleting a user that isn't there, is fine.
    Run {
        program: &'static str,
        args: Vec<String>,
        quiet: bool,
    },
    /// A file, if it's there.
    File(PathBuf),
    /// A folder and everything in it, if it's there and it's the agent's.
    Tree(PathBuf),
    /// The `cntrl` command, only while it's a link to the agent.
    Link {
        path: PathBuf,
        targets: Vec<PathBuf>,
    },
}

fn run(program: &'static str, args: &[&str], quiet: bool) -> Step {
    Step::Run {
        program,
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        quiet,
    }
}

/// `cntrl uninstall`: tells Console, then removes the agent; with `purge`,
/// what it kept too. Needs root.
pub fn uninstall(config: &Config, config_path: &Path, purge: bool) -> ExitCode {
    if !os::is_root() {
        say_err!(
            "cntrl uninstall: uninstalling needs {}; run {}",
            os::SUPERUSER,
            os::elevated("cntrl uninstall")
        );
        return ExitCode::FAILURE;
    }
    // Before anything stops: the link that tells Console is the agent's.
    client::goodbye(config);
    let steps = if cfg!(target_os = "macos") {
        mac(config, config_path, purge)
    } else {
        linux(config, config_path, purge)
    };
    match carry_out(&steps) {
        Ok(()) if purge => {
            say!(
                "Removed the agent, its identity, config, policy, audit log and logs, and its user."
            );
            ExitCode::SUCCESS
        }
        Ok(()) => {
            say!(
                "Removed the agent. Its identity, config and logs stay; --purge removes them too."
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            say_err!("cntrl uninstall: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Linux: the systemd units the install script wrote, the program, and the
/// `cntrl` link.
fn linux(config: &Config, config_path: &Path, purge: bool) -> Vec<Step> {
    let mut steps = vec![
        // The socket too, or socket activation would start privd again.
        run(
            "systemctl",
            &[
                "disable",
                "--now",
                "cntrl-agent.service",
                "cntrl-privd.socket",
                "cntrl-privd.service",
            ],
            false,
        ),
        run(
            "systemctl",
            &["reset-failed", "cntrl-agent.service", "cntrl-privd.service"],
            true,
        ),
    ];
    steps.extend(
        UNITS
            .iter()
            .map(|unit| Step::File(Path::new(LINUX_UNITS).join(unit))),
    );
    steps.push(run("systemctl", &["daemon-reload"], false));
    steps.push(Step::Link {
        path: COMMAND.into(),
        targets: vec!["cntrl-agent".into(), LINUX_PROGRAM.into()],
    });
    steps.push(Step::File(LINUX_PROGRAM.into()));
    if purge {
        steps.extend(kept(config, config_path));
        // Its group goes with it: the install script made it with --user-group.
        steps.push(run("userdel", &["cntrl"], true));
    }
    steps
}

/// macOS: the launchd jobs, the program under Application Support, and the
/// `cntrl` link, as `packaging/macos/uninstall.sh` removes them.
fn mac(config: &Config, config_path: &Path, purge: bool) -> Vec<Step> {
    let mut steps: Vec<Step> = LABELS
        .iter()
        .map(|label| run("launchctl", &["bootout", &format!("system/{label}")], true))
        .collect();
    steps.extend(
        LABELS
            .iter()
            .map(|label| Step::File(format!("/Library/LaunchDaemons/{label}.plist").into())),
    );
    let bin = Path::new(MAC_SUPPORT).join("bin");
    steps.push(Step::Link {
        path: COMMAND.into(),
        targets: vec![bin.join("cntrl-agent"), "cntrl-agent".into()],
    });
    steps.push(Step::Tree(bin));
    // Test builds before 0.1.0 put the program in /usr/local/bin, and the
    // sockets in /var/run.
    steps.extend(
        [
            "/usr/local/bin/cntrl-agent",
            "/var/run/cntrl-agent.sock",
            "/var/run/cntrl-privd.sock",
        ]
        .map(|path| Step::File(path.into())),
    );
    if purge {
        steps.push(Step::Tree(MAC_SUPPORT.into()));
        steps.extend(kept(config, config_path));
        steps.push(run("dscl", &[".", "-delete", "/Users/_cntrl"], true));
        steps.push(run("dscl", &[".", "-delete", "/Groups/_cntrl"], true));
    }
    steps
}

/// What `--purge` adds: the config and the policy, the identity, privd's
/// state, the audit log and the logs.
fn kept(config: &Config, config_path: &Path) -> Vec<Step> {
    let mut steps = Vec::new();
    // The config's folder goes when it's the agent's own, as /etc/cntrl is;
    // a config kept anywhere else goes alone.
    match config_path.parent() {
        Some(folder) if folder.file_name().is_some_and(|name| name == "cntrl") => {
            steps.push(Step::Tree(folder.to_path_buf()));
        }
        _ => {
            steps.push(Step::File(config_path.to_path_buf()));
            steps.push(Step::File(config.paths.policy.clone()));
        }
    }
    let paths = &config.paths;
    for folder in [
        &paths.state_dir,
        &paths.privd_state_dir,
        &paths.audit_dir,
        &paths.logs,
    ] {
        let step = Step::Tree(folder.clone());
        if !steps.contains(&step) {
            steps.push(step);
        }
    }
    steps
}

/// Whether a folder may go with everything in it: one with "cntrl" in its
/// path, at least two levels down, never a system folder named in a config.
fn ours(path: &Path) -> bool {
    path.is_absolute() && path.components().count() >= 3 && path.to_string_lossy().contains("cntrl")
}

fn carry_out(steps: &[Step]) -> Result<(), String> {
    let gone = |result: io::Result<()>, path: &Path| match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("can't remove {}: {e}", path.display())),
    };
    for step in steps {
        match step {
            Step::Run {
                program,
                args,
                quiet,
            } => match Command::new(program).args(args).output() {
                Ok(output) if output.status.success() || *quiet => {}
                Ok(output) => say_err!(
                    "{program} {}: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
                Err(_) if *quiet => {}
                Err(e) => say_err!("can't run {program}: {e}"),
            },
            Step::File(path) => gone(std::fs::remove_file(path), path)?,
            Step::Tree(path) if ours(path) => gone(std::fs::remove_dir_all(path), path)?,
            Step::Tree(path) => say_err!("leaving {}: it isn't the agent's folder", path.display()),
            Step::Link { path, targets } => {
                if std::fs::read_link(path).is_ok_and(|target| targets.contains(&target)) {
                    gone(std::fs::remove_file(path), path)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::Paths;

    fn linux_config() -> Config {
        Config {
            paths: Paths {
                agent_socket: "/run/cntrl-agent/agent.sock".into(),
                privd_socket: "/run/cntrl-privd/privd.sock".into(),
                state_dir: "/var/lib/cntrl".into(),
                privd_state_dir: "/var/lib/cntrl-privd".into(),
                policy: "/etc/cntrl/policy.toml".into(),
                audit_dir: "/var/log/cntrl/audit".into(),
                logs: "/var/log/cntrl".into(),
            },
            ..Config::default()
        }
    }

    fn mac_config() -> Config {
        Config {
            paths: Paths {
                agent_socket: "/var/run/cntrl/agent.sock".into(),
                privd_socket: "/var/run/cntrl/privd.sock".into(),
                state_dir: "/Library/Application Support/cntrl/agent".into(),
                privd_state_dir: "/Library/Application Support/cntrl/privd".into(),
                policy: "/etc/cntrl/policy.toml".into(),
                audit_dir: "/var/log/cntrl/audit".into(),
                logs: "/var/log/cntrl".into(),
            },
            ..Config::default()
        }
    }

    fn trees(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .filter_map(|step| match step {
                Step::Tree(path) => Some(path.display().to_string()),
                _ => None,
            })
            .collect()
    }

    fn commands(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .filter_map(|step| match step {
                Step::Run { program, args, .. } => Some(format!("{program} {}", args.join(" "))),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn on_linux_the_services_go_first_and_the_socket_with_them() {
        let config_path = Path::new("/etc/cntrl/agent.toml");
        let steps = linux(&linux_config(), config_path, false);
        assert_eq!(
            commands(&steps).first().map(String::as_str),
            Some(
                "systemctl disable --now cntrl-agent.service cntrl-privd.socket cntrl-privd.service"
            )
        );
        for unit in UNITS {
            assert!(
                steps.contains(&Step::File(Path::new(LINUX_UNITS).join(unit))),
                "{unit}"
            );
        }
        assert!(steps.contains(&Step::File(LINUX_PROGRAM.into())));
        assert!(
            trees(&steps).is_empty(),
            "without --purge nothing it kept goes"
        );
        assert!(
            !commands(&steps)
                .iter()
                .any(|command| command.starts_with("userdel"))
        );
    }

    #[test]
    fn on_linux_purge_takes_what_it_kept_and_its_user() {
        let steps = linux(&linux_config(), Path::new("/etc/cntrl/agent.toml"), true);
        assert_eq!(
            trees(&steps),
            [
                "/etc/cntrl",
                "/var/lib/cntrl",
                "/var/lib/cntrl-privd",
                "/var/log/cntrl/audit",
                "/var/log/cntrl"
            ]
        );
        assert!(commands(&steps).contains(&"userdel cntrl".to_owned()));
    }

    #[test]
    fn on_a_mac_it_removes_what_the_script_did() {
        let steps = mac(&mac_config(), Path::new("/etc/cntrl/agent.toml"), false);
        assert_eq!(
            commands(&steps),
            [
                "launchctl bootout system/pw.cntrl.agent",
                "launchctl bootout system/pw.cntrl.privd"
            ]
        );
        assert!(steps.contains(&Step::File(
            "/Library/LaunchDaemons/pw.cntrl.agent.plist".into()
        )));
        assert_eq!(trees(&steps), ["/Library/Application Support/cntrl/bin"]);
        let purged = mac(&mac_config(), Path::new("/etc/cntrl/agent.toml"), true);
        let purged_trees = trees(&purged);
        for folder in [MAC_SUPPORT, "/etc/cntrl", "/var/log/cntrl"] {
            assert!(purged_trees.iter().any(|tree| tree == folder), "{folder}");
        }
        assert!(commands(&purged).contains(&"dscl . -delete /Users/_cntrl".to_owned()));
    }

    #[test]
    fn a_config_kept_elsewhere_goes_alone() {
        let steps = kept(&linux_config(), Path::new("/etc/agent.toml"));
        assert!(steps.contains(&Step::File("/etc/agent.toml".into())));
        assert!(!trees(&steps).contains(&"/etc".to_owned()));
    }

    #[test]
    fn only_the_agents_folders_go_whole() {
        for path in ["/", "/etc", "/var/lib", "relative/cntrl", "/var/lib/other"] {
            assert!(!ours(Path::new(path)), "{path}");
        }
        for path in ["/etc/cntrl", "/var/lib/cntrl-privd", MAC_SUPPORT] {
            assert!(ours(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn the_command_goes_only_while_it_links_to_the_agent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let program = dir.path().join("cntrl-agent");
        std::fs::write(&program, b"").expect("program");
        let ours = dir.path().join("cntrl");
        std::os::unix::fs::symlink(&program, &ours).expect("link");
        let theirs = dir.path().join("other");
        std::os::unix::fs::symlink(dir.path().join("something-else"), &theirs).expect("link");
        let steps = [
            Step::Link {
                path: ours.clone(),
                targets: vec![program.clone()],
            },
            Step::Link {
                path: theirs.clone(),
                targets: vec![program.clone()],
            },
            Step::File(dir.path().join("not-there")),
        ];
        carry_out(&steps).expect("carried out");
        assert!(
            std::fs::symlink_metadata(&ours).is_err(),
            "the agent's link goes"
        );
        assert!(
            std::fs::symlink_metadata(&theirs).is_ok(),
            "another link stays"
        );
    }
}
