//! The agent config: `/etc/cntrl/agent.toml`, or `%ProgramData%\cntrl\agent.toml`
//! on Windows, then `CNTRL_*` environment variables. A missing file means
//! defaults. A file that can't be read or parsed stops the agent and is never
//! rewritten.

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
    pub console: ConsoleConfig,
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
    /// The local API socket that the `cntrl` CLI talks to.
    pub agent_socket: PathBuf,
    /// The privileged helper's socket.
    pub privd_socket: PathBuf,
    /// The agent's identity and device key.
    pub state_dir: PathBuf,
    /// privd's own state, including the audit key; root only.
    pub privd_state_dir: PathBuf,
    /// The device policy, owned by root.
    pub policy: PathBuf,
    /// The local audit log.
    pub audit_dir: PathBuf,
    /// Logs: launchd writes the agent's and privd's here on macOS, and on
    /// Windows they write their own, a file a day (D58). systemd's journal
    /// keeps them on Linux.
    pub logs: PathBuf,
}

/// The config file read unless `--config` or `CNTRL_CONFIG` names another.
pub fn default_path() -> PathBuf {
    #[cfg(windows)]
    {
        program_data().join("cntrl").join("agent.toml")
    }
    #[cfg(not(windows))]
    {
        "/etc/cntrl/agent.toml".into()
    }
}

/// `%ProgramData%`, where Windows keeps what services share.
#[cfg(windows)]
fn program_data() -> PathBuf {
    std::env::var_os("ProgramData").map_or_else(|| r"C:\ProgramData".into(), PathBuf::from)
}

impl Default for Paths {
    /// systemd makes the Linux sockets' directories; on macOS launchd makes
    /// the sockets themselves in `/var/run`, and state lives under
    /// `/Library/Application Support` (D20). On Windows the endpoints are
    /// named pipes, privd's where only administrators can create one, and the
    /// rest is under `%ProgramData%\cntrl` (D58).
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                agent_socket: "/var/run/cntrl-agent.sock".into(),
                privd_socket: "/var/run/cntrl-privd.sock".into(),
                state_dir: "/Library/Application Support/cntrl/agent".into(),
                privd_state_dir: "/Library/Application Support/cntrl/privd".into(),
                policy: "/etc/cntrl/policy.toml".into(),
                audit_dir: "/var/log/cntrl/audit".into(),
                logs: "/var/log/cntrl".into(),
            }
        }
        #[cfg(windows)]
        {
            let data = program_data().join("cntrl");
            Self {
                agent_socket: r"\\.\pipe\cntrl\agent".into(),
                privd_socket: r"\\.\pipe\ProtectedPrefix\Administrators\cntrl\privd".into(),
                state_dir: data.join("agent"),
                privd_state_dir: data.join("privd"),
                policy: data.join("policy.toml"),
                audit_dir: data.join("audit"),
                logs: data.join("logs"),
            }
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            Self {
                agent_socket: "/run/cntrl-agent/agent.sock".into(),
                privd_socket: "/run/cntrl-privd/privd.sock".into(),
                state_dir: "/var/lib/cntrl".into(),
                privd_state_dir: "/var/lib/cntrl-privd".into(),
                policy: "/etc/cntrl/policy.toml".into(),
                audit_dir: "/var/log/cntrl/audit".into(),
                logs: "/var/log/cntrl".into(),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ConsoleConfig {
    /// Base URL of Console's API, where enrollment goes: the gateway.
    pub url: String,
    /// Replaces the gateway URL from enrollment, for a staging gateway or
    /// debugging.
    pub gateway_url: Option<String>,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            url: "https://gw.cntrl.pw".to_owned(),
            gateway_url: None,
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
        let config =
            Config::parse("[log]\nlevel = \"debug\"\n[console]\nurl = \"http://localhost:8787\"\n")
                .expect("parses");
        assert_eq!(config.log.level, "debug");
        assert_eq!(config.console.url, "http://localhost:8787");
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
