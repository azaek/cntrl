//! systemd notifications: readiness, status text and stopping, plus a watchdog
//! fed only while the agent's health check passes. Outside systemd every call is
//! a no-op.

use std::sync::Arc;
use std::time::Duration;

use sd_notify::NotifyState;
use tokio_util::sync::CancellationToken;

use super::health::Health;

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
