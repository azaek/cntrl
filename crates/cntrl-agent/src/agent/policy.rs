//! The device policy: `/etc/cntrl/policy.toml`, which only local root changes.
//! It denies anything it doesn't list. With no file, the built-in monitor-only
//! policy applies. A file that's invalid, or writable by anyone but its owner,
//! denies every remote action until it's fixed.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::digest::sha256_hex;
use cntrl_protocol::OpInfo;
use cntrl_protocol::capability;
use cntrl_protocol::frame::{Caps, PolicySummary};
use cntrl_protocol::ops::{OPS, TOPICS};
use serde::{Deserialize, Serialize};

/// Where the policy in force came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    File,
    Default,
}

/// Who may approve agent updates on this device.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMode {
    Auto,
    #[default]
    Notify,
    Off,
}

/// A valid policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub source: Source,
    pub allow: BTreeSet<String>,
    /// Units that service actions refuse to touch.
    pub protect: BTreeSet<String>,
    pub update: UpdateMode,
    /// SHA-256 of the normalized policy, as the agent reports it to Console.
    pub hash: String,
}

/// The outcome of loading the policy. `Invalid` denies everything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PolicyState {
    Valid { policy: Policy },
    Invalid { reason: String },
}

impl PolicyState {
    /// Whether the policy allows `capability`.
    pub fn allows(&self, capability: &str) -> bool {
        matches!(self, Self::Valid { policy } if policy.allow.contains(capability))
    }

    /// The operations and topics this policy lets Console call, for the hello.
    pub fn caps(&self) -> Caps {
        let allowed = |registry: &[OpInfo]| {
            registry
                .iter()
                .filter(|info| self.allows(info.capability))
                .map(|info| (info.name.to_owned(), info.since))
                .collect()
        };
        Caps {
            ops: allowed(OPS),
            topics: allowed(TOPICS),
            features: Vec::new(),
        }
    }

    /// The policy's hash, or why none is in force.
    pub fn summary(&self) -> PolicySummary {
        match self {
            Self::Valid { policy } => PolicySummary {
                hash: policy.hash.clone(),
                error: None,
            },
            Self::Invalid { reason } => PolicySummary {
                hash: String::new(),
                error: Some(reason.clone()),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    version: u32,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    services: ServicesSection,
    #[serde(default)]
    update: UpdateSection,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServicesSection {
    #[serde(default)]
    protect: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateSection {
    #[serde(default)]
    mode: UpdateMode,
}

/// Loads the policy at `path`. The file must belong to `owner` (root when privd
/// runs as root) and be writable by nobody else.
pub fn load(path: &Path, owner: u32) -> PolicyState {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return PolicyState::Valid {
                policy: default_policy(),
            };
        }
        Err(e) => return invalid(format!("can't read {}: {e}", path.display())),
    };
    if metadata.uid() != owner {
        return invalid(format!("{} must be owned by uid {owner}", path.display()));
    }
    if metadata.mode() & 0o022 != 0 {
        return invalid(format!(
            "{} must not be writable by group or others",
            path.display()
        ));
    }
    match fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(policy) => PolicyState::Valid { policy },
            Err(reason) => invalid(format!("{}: {reason}", path.display())),
        },
        Err(e) => invalid(format!("can't read {}: {e}", path.display())),
    }
}

fn parse(text: &str) -> Result<Policy, String> {
    let file: PolicyFile = toml::from_str(text).map_err(|e| e.to_string())?;
    if file.version != 1 {
        return Err(format!("unsupported policy version {}", file.version));
    }
    if let Some(unknown) = file
        .allow
        .iter()
        .find(|name| !capability::is_capability(name))
    {
        return Err(format!("unknown capability `{unknown}`"));
    }
    Ok(build(
        Source::File,
        file.allow,
        file.services.protect,
        file.update.mode,
    ))
}

fn default_policy() -> Policy {
    let allow = capability::MONITOR_ONLY
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    build(Source::Default, allow, Vec::new(), UpdateMode::default())
}

fn build(source: Source, allow: Vec<String>, protect: Vec<String>, update: UpdateMode) -> Policy {
    let allow: BTreeSet<String> = allow.into_iter().collect();
    let protect: BTreeSet<String> = protect.into_iter().collect();
    let normalized = serde_json::json!({
        "version": 1,
        "allow": allow,
        "protect": protect,
        "update": update,
    });
    let hash = sha256_hex(normalized.to_string().as_bytes());
    Policy {
        source,
        allow,
        protect,
        update,
        hash,
    }
}

fn invalid(reason: String) -> PolicyState {
    PolicyState::Invalid { reason }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn write_policy(dir: &Path, text: &str, mode: u32) -> std::path::PathBuf {
        let path = dir.join("policy.toml");
        fs::write(&path, text).expect("write policy");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
        path
    }

    fn own_uid(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").uid()
    }

    #[test]
    fn no_file_means_monitor_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state = load(&dir.path().join("policy.toml"), 0);
        assert!(state.allows("system.read"));
        assert!(!state.allows("services.manage"));
        assert!(matches!(state, PolicyState::Valid { policy } if policy.source == Source::Default));
    }

    #[test]
    fn a_valid_file_allows_what_it_lists() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_policy(
            dir.path(),
            "version = 1\nallow = [\"system.read\", \"services.manage\"]\n",
            0o644,
        );
        let state = load(&path, own_uid(&path));
        assert!(state.allows("services.manage"));
        assert!(!state.allows("power.reboot"));
    }

    #[test]
    fn caps_list_only_what_the_policy_allows() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_policy(
            dir.path(),
            "version = 1\nallow = [\"system.read\"]\n",
            0o644,
        );
        let state = load(&path, own_uid(&path));
        let caps = state.caps();
        assert_eq!(caps.ops.get("system.info"), Some(&1));
        assert!(!caps.ops.contains_key("service.restart"));
        assert_eq!(caps.topics.get("stats"), Some(&1));
        assert!(state.summary().error.is_none());

        let denied = PolicyState::Invalid {
            reason: "broken".to_owned(),
        };
        assert!(denied.caps().ops.is_empty() && denied.caps().topics.is_empty());
        assert_eq!(denied.summary().error.as_deref(), Some("broken"));
    }

    #[test]
    fn the_same_content_hashes_the_same_whatever_its_source() {
        let dir = tempfile::tempdir().expect("temp dir");
        let text = "version = 1\nallow = [\"logs.read\", \"system.read\", \"services.read\", \"processes.read\"]\n";
        let path = write_policy(dir.path(), text, 0o644);
        let (PolicyState::Valid { policy: from_file }, PolicyState::Valid { policy: builtin }) = (
            load(&path, own_uid(&path)),
            load(&dir.path().join("missing.toml"), 0),
        ) else {
            unreachable!("both policies are valid");
        };
        assert_eq!(from_file.hash, builtin.hash);
    }

    #[test]
    fn unknown_capabilities_invalidate_the_policy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_policy(dir.path(), "version = 1\nallow = [\"everything\"]\n", 0o644);
        let state = load(&path, own_uid(&path));
        assert!(matches!(&state, PolicyState::Invalid { reason } if reason.contains("everything")));
        assert!(!state.allows("system.read"));
    }

    #[test]
    fn a_group_writable_file_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_policy(
            dir.path(),
            "version = 1\nallow = [\"system.read\"]\n",
            0o664,
        );
        assert!(matches!(
            load(&path, own_uid(&path)),
            PolicyState::Invalid { .. }
        ));
    }

    #[test]
    fn a_file_owned_by_someone_else_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_policy(dir.path(), "version = 1\n", 0o644);
        let other = own_uid(&path).wrapping_add(1);
        assert!(matches!(load(&path, other), PolicyState::Invalid { .. }));
    }
}
