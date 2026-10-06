//! The agent's own health check. A heartbeat task proves the runtime still makes
//! progress, and the systemd watchdog is fed only while it's fresh; on Windows
//! a thread of the agent's own watches it instead.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

pub struct Health {
    origin: Instant,
    last_beat_ms: AtomicU64,
}

impl Health {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            last_beat_ms: AtomicU64::new(0),
        }
    }

    /// Time since the agent started.
    pub fn uptime(&self) -> Duration {
        self.origin.elapsed()
    }

    pub fn beat(&self) {
        self.last_beat_ms
            .store(self.elapsed_ms(), Ordering::Relaxed);
    }

    /// Whether the last beat is at most `max_age` old.
    pub fn fresh(&self, max_age: Duration) -> bool {
        let age = self
            .elapsed_ms()
            .saturating_sub(self.last_beat_ms.load(Ordering::Relaxed));
        u128::from(age) <= max_age.as_millis()
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Beats once a second until cancelled.
pub async fn heartbeat(
    health: std::sync::Arc<Health>,
    token: CancellationToken,
) -> Result<(), String> {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            () = token.cancelled() => return Ok(()),
            _ = tick.tick() => health.beat(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_beat_is_fresh_and_ages_out() {
        let health = Health::new();
        health.beat();
        assert!(health.fresh(Duration::from_secs(1)));
        std::thread::sleep(Duration::from_millis(30));
        assert!(!health.fresh(Duration::from_millis(10)));
    }
}
