//! Command-line interface. The binary is also installed as `cntrl`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// The cntrl agent: a headless, system-level server-management agent.
#[derive(Debug, Parser)]
#[command(name = "cntrl-agent", version, about)]
pub struct Cli {
    /// Agent config file.
    #[arg(
        long,
        env = "CNTRL_CONFIG",
        default_value = "/etc/cntrl/agent.toml",
        global = true
    )]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the agent; this is what its systemd unit starts.
    Run,
    /// Run the privileged helper; its systemd socket starts it on demand.
    Privd,
    /// Show the running agent's status.
    Status {
        /// Print the raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Enroll this machine with Console, using a single-use token read from
    /// stdin or a file. Needs root.
    Enroll {
        /// Read the token from this file instead of stdin.
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Replace an existing enrollment.
        #[arg(long)]
        force_reenroll: bool,
    },
    /// Work with the config file.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Work with the device policy.
    #[command(subcommand)]
    Policy(PolicyCommand),
    /// Work with the local audit log.
    #[command(subcommand)]
    Audit(AuditCommand),
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
}

#[derive(Debug, Subcommand)]
pub enum AuditCommand {
    /// Check the audit log's hash chain: 0 when it's intact, 1 when it isn't.
    Verify,
}
