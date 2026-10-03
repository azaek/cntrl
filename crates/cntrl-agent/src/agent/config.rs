//! The agent config: `/etc/cntrl/agent.toml`, then `CNTRL_*` environment
//! variables. A missing file means defaults. A file that can't be read or
//! parsed stops the agent and is never rewritten.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub log: LogConfig,
    pub paths: Paths,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LogConfig {
    /// A tracing filter, such as `info` or `cntrl_agent=debug`. `CNTRL_LOG`
    /// overrides it.
    pub level: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Paths {
    /// The local API socket that `cntrl status` talks to.
    pub agent_socket: PathBuf,
    /// The privileged helper's socket.
    pub privd_socket: PathBuf,
    /// Device identity and agent state.
    pub state_dir: PathBuf,
    /// The device policy, owned by root.
    pub policy: PathBuf,
    /// The local audit log.
    pub audit_dir: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            agent_socket: "/run/cntrl-agent/agent.sock".into(),
            privd_socket: "/run/cntrl-privd/privd.sock".into(),
            state_dir: "/var/lib/cntrl".into(),
            policy: "/etc/cntrl/policy.toml".into(),
            audit_dir: "/var/log/cntrl/audit".into(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Read { path: PathBuf, source: io::Error },
    Parse { path: PathBuf, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "can't read {}: {source}", path.display()),
            Self::Parse { path, message } => {
                write!(f, "invalid config {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Loads the config at `path`, then applies environment overrides.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let mut config = match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|message| ConfigError::Parse {
                path: path.to_owned(),
                message,
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        if let Ok(level) = std::env::var("CNTRL_LOG") {
            config.log.level = level;
        }
        Ok(config)
    }

    fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_means_defaults() {
        assert_eq!(Config::parse("").expect("parses"), Config::default());
    }

    #[test]
    fn set_values_override_defaults() {
        let config = Config::parse("[log]\nlevel = \"debug\"\n").expect("parses");
        assert_eq!(config.log.level, "debug");
        assert_eq!(config.paths, Paths::default());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let err = Config::parse("[log]\nlevle = \"debug\"\n").expect_err("rejected");
        assert!(err.contains("levle"), "{err}");
    }

    #[test]
    fn a_missing_file_means_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = Config::load(&dir.path().join("agent.toml")).expect("loads");
        assert_eq!(config.paths, Paths::default());
    }
}
