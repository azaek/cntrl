//! Command-line interface. The binary is also installed as `cntrl`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use super::config;

/// The cntrl agent: a headless, system-level server-management agent.
#[derive(Debug, Parser)]
#[command(name = "cntrl-agent", version, about)]
pub struct Cli {
    /// Agent config file.
    #[arg(
        long,
        env = "CNTRL_CONFIG",
        default_value_os_t = config::default_path(),
        global = true
    )]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the agent; this is what its systemd unit or launchd job starts.
    Run,
    /// Run the privileged helper; its systemd or launchd socket starts it on demand.
    Privd,
    /// Show the running agent's status.
    Status {
        /// Print the raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Enroll this machine with Console, using a single-use token read from
    /// stdin or a file. On a machine that's in another account already, it
    /// asks before moving it there. Needs root.
    Enroll {
        /// Read the token from this file instead of stdin.
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Move the machine to the token's account without asking, replacing
        /// its current enrollment.
        #[arg(long = "move", visible_alias = "force-reenroll")]
        replace: bool,
    },
    /// Update the agent to the release Console offers, after checking the
    /// release's signature, size and SHA-256. The machine stays enrolled.
    /// Needs root.
    Update {
        /// Only say whether a newer release is out.
        #[arg(long)]
        check: bool,
        /// Install the release again even when this is it.
        #[arg(long)]
        force: bool,
    },
    /// Pause the agent: it tells Console who paused it and why, then hangs up
    /// and stays away, even across restarts, until `cntrl resume`. Console
    /// shows the device as paused, and it never alerts. Needs root.
    Pause {
        /// Why, for whoever sees it in Console.
        #[arg(long)]
        reason: Option<String>,
    },
    /// End a pause: the agent reconnects to Console. Needs root.
    Resume,
    /// Work with the config file.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Work with the device policy.
    #[command(subcommand)]
    Policy(PolicyCommand),
    /// Work with the local audit log.
    #[command(subcommand)]
    Audit(AuditCommand),
    /// Work with the history this machine keeps for Console's charts.
    #[command(subcommand)]
    History(HistoryCommand),
    /// Install the agent from this program, or install it again: its two
    /// services, under Program Files and %ProgramData%\cntrl. Needs an
    /// administrator.
    #[cfg(windows)]
    Install {
        /// Where enrollment goes, for a new config.
        #[arg(long)]
        console: Option<String>,
        /// A gateway to use in place of the one enrollment returns, for a new
        /// config.
        #[arg(long)]
        gateway: Option<String>,
    },
    /// Remove the agent: its services and its program. Its identity, config,
    /// audit log and logs stay unless --purge. Needs an administrator.
    #[cfg(windows)]
    Uninstall {
        /// Remove its identity, config, audit log and logs too; the device
        /// stays in Console until someone removes it there.
        #[arg(long)]
        purge: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Check the config file and exit: 0 when it's valid, 78 when it isn't.
    Check,
}

#[derive(Debug, Subcommand)]
pub enum PolicyCommand {
    /// Show the device policy in force.
    Show,
    /// Check the policy file and exit: 0 when it's valid, 1 when it isn't.
    Check,
    /// Allow capabilities, such as `services.manage`: rewrites the policy
    /// file and reconnects the agent. Needs root.
    Allow {
        /// The capabilities to allow.
        #[arg(required = true)]
        capabilities: Vec<String>,
    },
    /// Stop allowing capabilities: rewrites the policy file and reconnects the
    /// agent. Needs root.
    Deny {
        /// The capabilities to stop allowing.
        #[arg(required = true)]
        capabilities: Vec<String>,
    },
    /// Allow some capabilities and stop allowing others at once, as Console's
    /// permissions dialog writes it: rewrites the policy file and reconnects
    /// the agent once. A wrong name changes nothing. Needs root.
    Modify {
        /// Capabilities to allow, comma-separated or repeated.
        #[arg(long, value_delimiter = ',', value_name = "CAPABILITY")]
        allow: Vec<String>,
        /// Capabilities to stop allowing, comma-separated or repeated.
        #[arg(long, value_delimiter = ',', value_name = "CAPABILITY")]
        deny: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuditCommand {
    /// Check the audit log's hash chain: 0 when it's intact, 1 when it isn't.
    Verify,
}

#[derive(Debug, Subcommand)]
pub enum HistoryCommand {
    /// Show how many days are kept, since when, and the room it takes.
    Show,
    /// Keep this many days of history, from 1 to 365; fewer deletes the older
    /// days at once. Needs root.
    Keep {
        /// How many days.
        days: u32,
    },
    /// Delete all of it; the history starts again with the next minute.
    /// Needs root.
    Clear {
        /// Don't ask first.
        #[arg(long)]
        yes: bool,
    },
}
