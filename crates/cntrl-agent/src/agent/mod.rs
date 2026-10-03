//! The agent on Unix: command dispatch, the `run` loop of the network half, and
//! `privd`, the privileged half.

mod audit;
mod cli;
mod client;
mod config;
mod digest;
mod enroll;
mod health;
mod host;
mod identity;
mod ipc;
mod keys;
#[cfg(target_os = "macos")]
mod launchd;
mod local_api;
mod logging;
mod outbox;
mod policy;
mod privd;
mod processes;
mod stats;
mod supervisor;
mod systemd;
mod uplink;

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use tracing::{error, info, warn};

use cli::{AuditCommand, Cli, Command, ConfigCommand, PolicyCommand};
use config::Config;
use health::Health;
use local_api::AgentState;
use outbox::Outbox;
use supervisor::Supervisor;
use uplink::{Uplink, UplinkConfig};

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
        Command::Enroll {
            token_file,
            replace,
        } => client::run_enroll(&config, token_file.as_deref(), replace),
        Command::Config(ConfigCommand::Check) => {
            println!("{}: OK", cli.config.display());
            ExitCode::SUCCESS
        }
        Command::Policy(PolicyCommand::Show) => client::print_policy(&config, false),
        Command::Policy(PolicyCommand::Check) => client::print_policy(&config, true),
        Command::Policy(PolicyCommand::Allow { capability }) => {
            client::allow_capability(&config, &capability)
        }
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
        let listener = match local_api::listen(&config.paths.agent_socket) {
            Ok(listener) => listener,
            Err(e) => {
                error!("{e}");
                return ExitCode::FAILURE;
            }
        };
        let health = Arc::new(Health::new());
        let uplink = Arc::new(Uplink::new());
        let latest_stats = Arc::new(stats::Latest::new(None));
        let latest_processes = Arc::new(processes::Latest::new(None));
        let state_dir = config.paths.state_dir.clone();
        let outbox = Arc::new(Outbox::open(&state_dir).await);
        let privd_socket = config.paths.privd_socket.clone();
        let uplink_config = UplinkConfig {
            state_dir: state_dir.clone(),
            privd_socket: privd_socket.clone(),
            gateway_url: config.console.gateway_url.clone(),
            stats: Arc::clone(&latest_stats),
            processes: Arc::clone(&latest_processes),
            outbox: Arc::clone(&outbox),
        };
        let state = Arc::new(AgentState::new(
            config,
            config_path.to_owned(),
            Arc::clone(&health),
            Arc::clone(&uplink),
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
        supervisor.spawn("watchdog", systemd::watchdog(health, token.clone()));
        let host_stats = cntrl_host::stats::backend();
        supervisor.spawn(
            "stats",
            stats::run(host_stats, latest_stats, Arc::clone(&outbox), token.clone()),
        );
        supervisor.spawn(
            "processes",
            processes::run(latest_processes, privd_socket.clone(), token.clone()),
        );
        supervisor.spawn(
            "checkpoints",
            audit::checkpoints(outbox, state_dir, privd_socket.clone(), token.clone()),
        );
        supervisor.spawn("uplink", uplink::run(uplink_config, uplink, token));

        record_start(&privd_socket).await;
        systemd::ready("running");
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
