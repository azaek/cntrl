//! The agent on Unix: command dispatch, the `run` loop of the network half, and
//! `privd`, the privileged half.

mod audit;
mod cli;
mod client;
mod config;
mod health;
mod ipc;
mod local_api;
mod logging;
mod policy;
mod privd;
mod supervisor;
mod systemd;

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use tracing::{error, info, warn};

use cli::{AuditCommand, Cli, Command, ConfigCommand, PolicyCommand};
use config::Config;
use health::Health;
use local_api::AgentState;
use supervisor::Supervisor;

/// `EX_CONFIG` from sysexits(3): the unit's `RestartPreventExitStatus=` stops
/// systemd from restarting into the same bad config.
const EXIT_CONFIG: u8 = 78;

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match Config::load(&cli.config) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    match cli.command {
        Command::Run => run(&cli.config, config),
        Command::Privd => privd::main(&config),
        Command::Status { json } => client::print_status(&config, json),
        Command::Config(ConfigCommand::Check) => {
            println!("{}: OK", cli.config.display());
            ExitCode::SUCCESS
        }
        Command::Policy(PolicyCommand::Show) => client::print_policy(&config, false),
        Command::Policy(PolicyCommand::Check) => client::print_policy(&config, true),
        Command::Audit(AuditCommand::Verify) => client::print_audit_verify(&config),
    }
}

fn run(config_path: &Path, config: Config) -> ExitCode {
    logging::init(&config.log.level);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(16)
        .enable_all()
        .build();
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(e) => {
            error!("can't start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async move {
        let listener = match local_api::bind(&config.paths.agent_socket) {
            Ok(listener) => listener,
            Err(e) => {
                error!("{e}");
                return ExitCode::FAILURE;
            }
        };
        let health = Arc::new(Health::new());
        let privd_socket = config.paths.privd_socket.clone();
        let state = Arc::new(AgentState::new(
            config_path.to_owned(),
            privd_socket.clone(),
            Arc::clone(&health),
        ));

        let mut supervisor = Supervisor::new();
        let token = supervisor.token();
        supervisor.spawn(
            "local-api",
            local_api::serve(listener, state, token.clone()),
        );
        supervisor.spawn(
            "heartbeat",
            health::heartbeat(Arc::clone(&health), token.clone()),
        );
        supervisor.spawn("watchdog", systemd::watchdog(health, token));

        record_start(&privd_socket).await;
        systemd::ready("running, not enrolled");
        info!(version = env!("CARGO_PKG_VERSION"), "cntrl-agent started");
        supervisor.run_until_shutdown().await
    })
}

/// Records the start in the audit log. privd may be missing in a run by hand,
/// so a failure here is only a warning.
async fn record_start(privd_socket: &Path) {
    let call = ipc::Call::AuditAppend {
        kind: "agent.started".to_owned(),
        data: serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }),
    };
    if let Err(e) = ipc::call_once(privd_socket, call).await {
        warn!("couldn't record the start in the audit log: {e}");
    }
}
