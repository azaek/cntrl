//! Runs the agent's subsystems as tasks under one cancellation token. A
//! subsystem that stops on its own takes the process down with exit code 70, so
//! the service manager restarts it cleanly instead of leaving a running but
//! broken agent.

use std::future::Future;
use std::process::ExitCode;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::service;

/// `EX_SOFTWARE` from sysexits(3).
pub const EXIT_SOFTWARE: u8 = 70;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

type Outcome = (&'static str, Result<(), String>);

pub struct Supervisor {
    token: CancellationToken,
    tasks: JoinSet<Outcome>,
}

impl Supervisor {
    /// `token` is the service's: the service manager may cancel it to stop.
    pub fn new(token: CancellationToken) -> Self {
        Self {
            token,
            tasks: JoinSet::new(),
        }
    }

    /// The token every subsystem watches; cancelling it starts shutdown.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn spawn(
        &mut self,
        name: &'static str,
        task: impl Future<Output = Result<(), String>> + Send + 'static,
    ) {
        self.tasks.spawn(async move { (name, task.await) });
    }

    /// Waits for SIGTERM, an interrupt, the service manager's stop request or
    /// a subsystem stopping, then cancels everything and waits up to 10 s for
    /// the rest to finish.
    pub async fn run_until_shutdown(mut self) -> ExitCode {
        let mut terminate = match service::Terminate::listen() {
            Ok(terminate) => terminate,
            Err(e) => {
                error!("{e}");
                return ExitCode::FAILURE;
            }
        };
        let failure = tokio::select! {
            reason = terminate.recv() => {
                info!("{reason}, shutting down");
                None
            }
            () = self.token.cancelled() => {
                info!("the service manager asked to stop, shutting down");
                None
            }
            _ = tokio::signal::ctrl_c() => {
                info!("interrupted, shutting down");
                None
            }
            Some(joined) = self.tasks.join_next() => Some(match joined {
                Ok((name, Ok(()))) => format!("{name} stopped unexpectedly"),
                Ok((name, Err(e))) => format!("{name} failed: {e}"),
                Err(e) => format!("a subsystem panicked: {e}"),
            }),
        };

        service::stopping();
        self.token.cancel();
        let drain = async { while self.tasks.join_next().await.is_some() {} };
        if tokio::time::timeout(SHUTDOWN_GRACE, drain).await.is_err() {
            warn!("subsystems didn't stop within {SHUTDOWN_GRACE:?}; aborting them");
            self.tasks.abort_all();
        }

        match failure {
            None => ExitCode::SUCCESS,
            Some(reason) => {
                error!("{reason}");
                ExitCode::from(EXIT_SOFTWARE)
            }
        }
    }
}
