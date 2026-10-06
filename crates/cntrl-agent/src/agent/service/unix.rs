//! systemd's notifications: readiness, status text and stopping, plus a
//! watchdog fed only while the agent's health check passes. Outside systemd,
//! as under launchd, every call is a no-op.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use sd_notify::NotifyState;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio_util::sync::CancellationToken;

use super::super::health::Health;

/// Runs `body`, the service's main. Nothing cancels its token here: systemd
/// and launchd stop the service with SIGTERM, which [`Terminate`] hears.
pub fn run(_name: &'static str, body: impl FnOnce(CancellationToken) -> ExitCode) -> ExitCode {
    body(CancellationToken::new())
}

/// SIGTERM, which systemd and launchd send to stop the service.
pub struct Terminate(Signal);

impl Terminate {
    pub fn listen() -> Result<Self, String> {
        signal(SignalKind::terminate())
            .map(Self)
            .map_err(|e| format!("can't listen for SIGTERM: {e}"))
    }

    /// Waits for the signal, and says what came.
    pub async fn recv(&mut self) -> &'static str {
        self.0.recv().await;
        "SIGTERM received"
    }
}

pub fn ready(status: &str) {
    notify(&[NotifyState::Ready, NotifyState::Status(status)]);
}

pub fn stopping() {
    notify(&[NotifyState::Stopping]);
}

/// Pings the watchdog at half its interval while the health check is fresh.
/// Without `WatchdogSec=` it just waits to be cancelled.
pub async fn watchdog(health: Arc<Health>, token: CancellationToken) -> Result<(), String> {
    let mut usec = 0;
    if !sd_notify::watchdog_enabled(false, &mut usec) || usec == 0 {
        token.cancelled().await;
        return Ok(());
    }
    let timeout = Duration::from_micros(usec);
    let mut tick = tokio::time::interval(timeout / 2);
    loop {
        tokio::select! {
            () = token.cancelled() => return Ok(()),
            _ = tick.tick() => {
                if health.fresh(timeout / 2) {
                    notify(&[NotifyState::Watchdog]);
                } else {
                    tracing::warn!("health check is stale, so the watchdog isn't fed");
                }
            }
        }
    }
}

fn notify(states: &[NotifyState<'_>]) {
    if let Err(e) = sd_notify::notify(false, states) {
        tracing::debug!("sd_notify failed: {e}");
    }
}
