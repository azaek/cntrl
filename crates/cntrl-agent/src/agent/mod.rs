//! The agent on Unix: command dispatch and the `run` loop.

mod cli;
mod client;
mod config;
mod health;
mod local_api;
mod logging;
mod supervisor;
mod systemd;

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use tracing::{error, info};

use cli::{Cli, Command, ConfigCommand};
use config::Config;
use health::Health;
use local_api::AgentState;
use supervisor::Supervisor;

/// `EX_CONFIG` from sysexits(3): the unit's `RestartPreventExitStatus=` stops
/// systemd from restarting into the same bad config.
const EXIT_CONFIG: u8 = 78;

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Run => run(&cli.config),
        Command::Privd => {
            eprintln!("cntrl-agent privd isn't implemented yet");
            ExitCode::FAILURE
        }
        Command::Status { json } => client::print_status(&cli.config, json),
        Command::Config(ConfigCommand::Check) => match Config::load(&cli.config) {
            Ok(_) => {
                println!("{}: OK", cli.config.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(EXIT_CONFIG)
            }
        },
    }
}

fn run(config_path: &Path) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(EXIT_CONFIG);
        }
    };
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
        let state = Arc::new(AgentState::new(config_path.to_owned(), Arc::clone(&health)));

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

        systemd::ready("running, not enrolled");
        info!(version = env!("CARGO_PKG_VERSION"), "cntrl-agent started");
        supervisor.run_until_shutdown().await
    })
}
