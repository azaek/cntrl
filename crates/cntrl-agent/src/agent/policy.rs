//! The device policy: `/etc/cntrl/policy.toml`, which only local root changes.
//! It denies anything it doesn't list. With no file, the built-in monitor-only
//! policy applies. A file that's invalid, or writable by anyone but its owner,
//! denies every remote action until it's fixed.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use super::digest::sha256_hex;
use cntrl_host::services::service_name;
use cntrl_protocol::OpInfo;
use cntrl_protocol::capability;
use cntrl_protocol::frame::{Caps, PolicySummary};
use cntrl_protocol::ops::{OPS, TOPICS};
use serde::{Deserialize, Serialize};

/// Services protected unless the policy file lists its own: losing SSH can lock
/// the owner out.
#[cfg(not(target_os = "macos"))]
const DEFAULT_PROTECTED: &[&str] = &["ssh.service", "sshd.service"];
#[cfg(target_os = "macos")]
const DEFAULT_PROTECTED: &[&str] = &["com.openssh.sshd"];
/// The agent's own services, always protected: a restart through the agent
/// would end the request that asked for it.
#[cfg(not(target_os = "macos"))]
const ALWAYS_PROTECTED: &[&str] = &[
    "cntrl-agent.service",
    "cntrl-privd.service",
    "cntrl-privd.socket",
];
#[cfg(target_os = "macos")]
const ALWAYS_PROTECTED: &[&str] = &["pw.cntrl.agent", "pw.cntrl.privd"];

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

    /// Whether service actions must leave `unit` alone. An invalid policy
    /// protects everything.
    pub fn protects(&self, unit: &str) -> bool {
        let Self::Valid { policy } = self else {
            return true;
        };
        let unit = service_name(unit).unwrap_or_else(|_| unit.to_owned());
        policy
            .protect
            .iter()
            .any(|entry| service_name(entry).is_ok_and(|entry| entry == unit))
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

#[derive(Deserialize, Serialize)]
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

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ServicesSection {
    /// Absent means [`DEFAULT_PROTECTED`].
    #[serde(skip_serializing_if = "Option::is_none")]
    protect: Option<Vec<String>>,
}

#[derive(Default, Deserialize, Serialize)]
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

/// Adds `capability` to the policy file at `path`, starting from the built-in
/// policy when there's no file. Returns whether anything changed. The file is
/// rewritten whole, so comments in it are lost; the old file is kept beside it
/// as `policy.toml.bak`.
pub fn allow(path: &Path, owner: u32, capability: &str) -> Result<bool, String> {
    if !capability::is_capability(capability) {
        let all = capability::CAPABILITIES.join(", ");
        return Err(format!(
            "`{capability}` isn't a capability; there are {all}"
        ));
    }
    let policy = match load(path, owner) {
        PolicyState::Valid { policy } => policy,
        PolicyState::Invalid { reason } => return Err(format!("fix the policy first: {reason}")),
    };
    if policy.allow.contains(capability) {
        return Ok(false);
    }
    let file = PolicyFile {
        version: 1,
        allow: policy
            .allow
            .into_iter()
            .chain([capability.to_owned()])
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        services: ServicesSection {
            protect: Some(policy.protect.into_iter().collect()),
        },
        update: UpdateSection {
            mode: policy.update,
        },
    };
    let text = toml::to_string(&file).map_err(|e| e.to_string())?;
    let text = format!(
        "# The device policy. Only root changes it: edit this file, or run\n\
         # `sudo cntrl policy allow <capability>`, which rewrites it.\n{text}"
    );
    write_atomically(path, &text)?;
    Ok(true)
}

/// Replaces `path` through a temporary file, so a reader never sees half of it.
fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    let context = |e: io::Error| {
        let hint = if e.kind() == io::ErrorKind::PermissionDenied {
            " (run it with sudo)"
        } else {
            ""
        };
        format!("can't write {}: {e}{hint}", path.display())
    };
    if path.exists() {
        fs::copy(path, path.with_extension("toml.bak")).map_err(context)?;
    }
    let temporary = path.with_extension("toml.new");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(&temporary)
        .map_err(context)?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(context)?;
    fs::rename(&temporary, path).map_err(context)
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
    let protect = file.services.protect.unwrap_or_else(|| {
        DEFAULT_PROTECTED
            .iter()
            .map(|unit| (*unit).to_owned())
            .collect()
    });
    Ok(build(Source::File, file.allow, protect, file.update.mode))
}

fn default_policy() -> Policy {
    let allow = capability::MONITOR_ONLY
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let protect = DEFAULT_PROTECTED
        .iter()
        .map(|unit| (*unit).to_owned())
        .collect();
    build(Source::Default, allow, protect, UpdateMode::default())
}

fn build(source: Source, allow: Vec<String>, protect: Vec<String>, update: UpdateMode) -> Policy {
    let allow: BTreeSet<String> = allow.into_iter().collect();
    let protect: BTreeSet<String> = protect
        .into_iter()
        .chain(ALWAYS_PROTECTED.iter().map(|unit| (*unit).to_owned()))
        .collect();
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

    /// SSH's service, as this OS names it.
    #[cfg(not(target_os = "macos"))]
    const SSH: &str = "sshd";
    #[cfg(target_os = "macos")]
    const SSH: &str = "com.openssh.sshd";

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn ssh_and_the_agent_are_protected_by_default() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_policy(
            dir.path(),
            "version = 1\nallow = [\"services.manage\"]\n",
            0o644,
        );
        let state = load(&path, own_uid(&path));
        assert!(state.protects("sshd"));
        assert!(state.protects("ssh.service"));
        assert!(state.protects("cntrl-agent.service"));
        assert!(!state.protects("nginx"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn ssh_and_the_agent_are_protected_by_default() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_policy(
            dir.path(),
            "version = 1\nallow = [\"services.manage\"]\n",
            0o644,
        );
        let state = load(&path, own_uid(&path));
        assert!(state.protects("com.openssh.sshd"));
        assert!(state.protects("pw.cntrl.agent"));
        assert!(state.protects("pw.cntrl.privd"));
        assert!(!state.protects("homebrew.mxcl.nginx"));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn a_protect_list_replaces_the_defaults_but_not_the_agent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let text = "version = 1\n[services]\nprotect = [\"nginx\"]\n";
        let path = write_policy(dir.path(), text, 0o644);
        let state = load(&path, own_uid(&path));
        assert!(state.protects("nginx.service"));
        assert!(!state.protects("sshd"));
        assert!(state.protects("cntrl-privd"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_protect_list_replaces_the_defaults_but_not_the_agent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let text = "version = 1\n[services]\nprotect = [\"homebrew.mxcl.nginx\"]\n";
        let path = write_policy(dir.path(), text, 0o644);
        let state = load(&path, own_uid(&path));
        assert!(state.protects("homebrew.mxcl.nginx"));
        assert!(!state.protects("com.openssh.sshd"));
        assert!(state.protects("pw.cntrl.privd"));
    }

    #[test]
    fn allow_starts_from_the_built_in_policy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("policy.toml");
        let owner = fs::metadata(dir.path()).expect("temp dir").uid();
        assert_eq!(allow(&path, owner, "services.manage"), Ok(true));
        let state = load(&path, owner);
        assert!(state.allows("services.manage"));
        assert!(state.allows("system.read"));
        assert!(state.protects(SSH));
        assert_eq!(allow(&path, owner, "services.manage"), Ok(false));
        assert!(!dir.path().join("policy.toml.bak").exists());
    }

    #[test]
    fn allow_keeps_the_rest_of_the_file_and_a_backup() {
        let dir = tempfile::tempdir().expect("temp dir");
        let text = "version = 1\nallow = []\n[services]\nprotect = [\"nginx\"]\n[update]\nmode = \"off\"\n";
        let path = write_policy(dir.path(), text, 0o644);
        let owner = own_uid(&path);
        assert_eq!(allow(&path, owner, "power.reboot"), Ok(true));
        let policy = match load(&path, owner) {
            PolicyState::Valid { policy } => Some(policy),
            PolicyState::Invalid { .. } => None,
        }
        .expect("the rewritten policy is valid");
        assert!(policy.allow.contains("power.reboot"));
        assert!(policy.protect.contains("nginx"));
        assert_eq!(policy.update, UpdateMode::Off);
        let backup = fs::read_to_string(dir.path().join("policy.toml.bak")).expect("a backup");
        assert_eq!(backup, text);
    }

    #[test]
    fn allow_refuses_unknown_capabilities_and_invalid_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_policy(dir.path(), "version = 2\n", 0o644);
        let owner = own_uid(&path);
        assert!(allow(&path, owner, "root.everything").is_err());
        assert!(allow(&path, owner, "services.manage").is_err());
    }

    #[test]
    fn an_invalid_policy_protects_everything() {
        let state = PolicyState::Invalid {
            reason: "test".to_owned(),
        };
        assert!(state.protects("nginx"));
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
