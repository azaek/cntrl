//! Logs go to journald when systemd connected the agent's output to the journal,
//! to a file a day under `paths.logs`, kept a week, when Windows' Service
//! Control Manager runs the agent (D58), and to stderr otherwise.

use std::path::Path;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// `name` names the files: `agent` or `privd`.
pub fn init(level: &str, logs: &Path, name: &str) {
    let (filter, bad_level) = match EnvFilter::try_new(level) {
        Ok(filter) => (filter, None),
        Err(e) => (EnvFilter::new("info"), Some(e)),
    };
    let registry = tracing_subscriber::registry().with(filter);
    match output(logs, name) {
        #[cfg(unix)]
        Output::Journal(journald) => registry.with(journald).init(),
        #[cfg(windows)]
        Output::Files(files) => registry
            .with(fmt::layer().with_ansi(false).with_writer(files))
            .init(),
        Output::Stderr(problem) => {
            registry
                .with(fmt::layer().with_writer(std::io::stderr))
                .init();
            if let Some(problem) = problem {
                tracing::warn!("{problem}; logging to stderr");
            }
        }
    }
    if let Some(e) = bad_level {
        tracing::warn!("invalid log level {level:?} ({e}); using info");
    }
}

enum Output {
    #[cfg(unix)]
    Journal(tracing_journald::Layer),
    #[cfg(windows)]
    Files(tracing_appender::rolling::RollingFileAppender),
    /// With why the logs aren't where they'd go.
    Stderr(Option<String>),
}

#[cfg(unix)]
fn output(_logs: &Path, _name: &str) -> Output {
    match std::env::var_os("JOURNAL_STREAM").and_then(|_| tracing_journald::layer().ok()) {
        Some(journald) => Output::Journal(journald),
        None => Output::Stderr(None),
    }
}

#[cfg(windows)]
fn output(logs: &Path, name: &str) -> Output {
    use tracing_appender::rolling::{RollingFileAppender, Rotation};

    if !super::service::managed() {
        return Output::Stderr(None);
    }
    let files = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(name)
        .filename_suffix("log")
        .max_log_files(7)
        .build(logs);
    match files {
        Ok(files) => Output::Files(files),
        Err(e) => Output::Stderr(Some(format!("can't write logs to {}: {e}", logs.display()))),
    }
}
