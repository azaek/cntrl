//! Logs go to journald when systemd connected the agent's output to the journal,
//! and to stderr otherwise.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

pub fn init(level: &str) {
    let (filter, bad_level) = match EnvFilter::try_new(level) {
        Ok(filter) => (filter, None),
        Err(e) => (EnvFilter::new("info"), Some(e)),
    };
    let registry = tracing_subscriber::registry().with(filter);
    let journald = std::env::var_os("JOURNAL_STREAM").and_then(|_| tracing_journald::layer().ok());
    match journald {
        Some(journald) => registry.with(journald).init(),
        None => registry
            .with(fmt::layer().with_writer(std::io::stderr))
            .init(),
    }
    if let Some(e) = bad_level {
        tracing::warn!("invalid log level {level:?} ({e}); using info");
    }
}
