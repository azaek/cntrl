//! The device policy: `/etc/cntrl/policy.toml`, which only local root changes.
//! It denies anything it doesn't list. With no file, the built-in monitor-only
//! policy applies. A file that's invalid, or writable by anyone but its owner,
//! denies every remote action until it's fixed.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use super::digest::sha256_hex;
use super::os::{self, Private};
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

/// Whether `unit` is a per-connection copy of `entry`, as launchd names them on
/// macOS: `com.openssh.sshd.<UUID>` for each SSH session. Restarting one would
/// end that session, so protecting a job protects its copies.
fn instance_of(unit: &str, entry: &str) -> bool {
    cfg!(target_os = "macos")
        && unit
            .strip_prefix(entry)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|id| {
                id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            })
}

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
        policy.protect.iter().any(|entry| {
            service_name(entry).is_ok_and(|entry| entry == unit || instance_of(&unit, &entry))
        })
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
        // Running checks isn't an operation or a topic: the hub sends them in
        // a frame, so the hello says the device takes them (D56).
        let features = if self.allows("checks.run") {
            vec![cntrl_protocol::checks::FEATURE.to_owned()]
        } else {
            Vec::new()
        };
        Caps {
            ops: allowed(OPS),
            topics: allowed(TOPICS),
            features,
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
/// runs as root; on Windows, Administrators or SYSTEM) and be writable by
/// nobody else.
pub fn load(path: &Path, owner: os::Owner) -> PolicyState {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return PolicyState::Valid {
                policy: default_policy(),
            };
        }
        Err(e) => return invalid(format!("can't read {}: {e}", path.display())),
    };
    if let Err(reason) = os::check_trusted(path, &metadata, owner) {
        return invalid(reason);
    }
    match fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(policy) => PolicyState::Valid { policy },
            Err(reason) => invalid(format!("{}: {reason}", path.display())),
        },
        Err(e) => invalid(format!("can't read {}: {e}", path.display())),
    }
}

/// What a change to the policy did: the capabilities it turned on and off.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub allowed: Vec<String>,
    pub denied: Vec<String>,
}

impl Changed {
    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty() && self.denied.is_empty()
    }
}

/// Allows the capabilities in `allow` and stops allowing those in `deny`,
/// rewriting the file once (D53). Every name is checked first, so an unknown
/// one, or one in both lists, changes nothing. Without a file it starts from
/// the built-in policy. The answer says what changed, which may be nothing,
/// and then the file isn't touched.
pub fn modify(
    path: &Path,
    owner: os::Owner,
    allow: &[String],
    deny: &[String],
) -> Result<Changed, String> {
    if let Some(unknown) = allow
        .iter()
        .chain(deny)
        .find(|name| !capability::is_capability(name))
    {
        let all = capability::CAPABILITIES.join(", ");
        return Err(format!("`{unknown}` isn't a capability; there are {all}"));
    }
    if let Some(both) = allow.iter().find(|name| deny.contains(name)) {
        return Err(format!("`{both}` can't be both allowed and denied"));
    }
    let policy = match load(path, owner) {
        PolicyState::Valid { policy } => policy,
        PolicyState::Invalid { reason } => return Err(format!("fix the policy first: {reason}")),
    };
    let mut allowed = policy.allow;
    let mut changed = Changed::default();
    for name in allow {
        if allowed.insert(name.clone()) {
            changed.allowed.push(name.clone());
        }
    }
    for name in deny {
        if allowed.remove(name) {
            changed.denied.push(name.clone());
        }
    }
    if changed.is_empty() {
        return Ok(changed);
    }
    let file = PolicyFile {
        version: 1,
        allow: allowed.into_iter().collect(),
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
         # `sudo cntrl policy allow <capability>`, `deny`, or\n\
         # `modify --allow <a,b> --deny <c>`, which rewrite it.\n{text}"
    );
    write_atomically(path, &text)?;
    Ok(changed)
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
        .shared()
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

// These write policy files with Unix modes and owners; Windows judges access
// lists instead.
#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

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
        // Each SSH session is its own launchd job.
        assert!(state.protects("com.openssh.sshd.13FF5176-EF2C-4879-BC4A-064E004D404F"));
        assert!(!state.protects("com.openssh.sshd.extra"));
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

    /// Names as the command line passes them.
    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    fn allow(path: &Path, owner: u32, capability: &str) -> Result<bool, String> {
        modify(path, owner, &names(&[capability]), &[]).map(|changed| !changed.is_empty())
    }

    fn deny(path: &Path, owner: u32, capability: &str) -> Result<bool, String> {
        modify(path, owner, &[], &names(&[capability])).map(|changed| !changed.is_empty())
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
    fn deny_takes_one_capability_off_and_keeps_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("policy.toml");
        let owner = fs::metadata(dir.path()).expect("temp dir").uid();
        // From the built-in policy, which allows monitoring.
        assert_eq!(deny(&path, owner, "processes.read"), Ok(true));
        let state = load(&path, owner);
        assert!(!state.allows("processes.read"));
        assert!(state.allows("system.read"));
        assert!(state.protects(SSH));
        assert_eq!(deny(&path, owner, "processes.read"), Ok(false));
        assert_eq!(allow(&path, owner, "processes.read"), Ok(true));
        assert!(load(&path, owner).allows("processes.read"));
    }

    #[test]
    fn deny_refuses_unknown_capabilities() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("policy.toml");
        let owner = fs::metadata(dir.path()).expect("temp dir").uid();
        assert!(deny(&path, owner, "root.everything").is_err());
        assert!(!path.exists());
    }

    #[test]
    fn modify_allows_and_denies_in_one_rewrite() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("policy.toml");
        let owner = fs::metadata(dir.path()).expect("temp dir").uid();
        let changed = modify(
            &path,
            owner,
            &names(&["services.manage", "history.manage", "system.read"]),
            &names(&["processes.read", "power.reboot"]),
        )
        .expect("modified");
        // system.read was on already, and power.reboot off.
        assert_eq!(
            changed.allowed,
            names(&["services.manage", "history.manage"])
        );
        assert_eq!(changed.denied, names(&["processes.read"]));
        let state = load(&path, owner);
        assert!(state.allows("services.manage") && state.allows("history.manage"));
        assert!(!state.allows("processes.read"));
        assert!(state.allows("logs.read"));
        // Asking again changes nothing, and the file isn't touched.
        let again = modify(
            &path,
            owner,
            &names(&["services.manage"]),
            &names(&["power.reboot"]),
        )
        .expect("modified");
        assert!(again.is_empty());
        assert!(!dir.path().join("policy.toml.bak").exists());
    }

    #[test]
    fn modify_changes_nothing_when_a_name_is_wrong_or_in_both_lists() {
        let dir = tempfile::tempdir().expect("temp dir");
        let text = "version = 1\nallow = [\"system.read\"]\n";
        let path = write_policy(dir.path(), text, 0o644);
        let owner = own_uid(&path);
        let typo = modify(
            &path,
            owner,
            &names(&["services.manage", "servics.manage"]),
            &[],
        );
        assert!(typo.is_err_and(|e| e.contains("servics.manage")));
        let both = modify(&path, owner, &names(&["logs.read"]), &names(&["logs.read"]));
        assert!(both.is_err_and(|e| e.contains("both")));
        assert_eq!(fs::read_to_string(&path).expect("the policy"), text);
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
        assert!(caps.features.is_empty());
        assert!(state.summary().error.is_none());

        // Checks come in a frame, so the hello names them as a feature (D56).
        let path = write_policy(
            dir.path(),
            "version = 1\nallow = [\"system.read\", \"checks.run\"]\n",
            0o644,
        );
        let checks = load(&path, own_uid(&path)).caps();
        assert_eq!(checks.features, vec!["checks".to_owned()]);

        let denied = PolicyState::Invalid {
            reason: "broken".to_owned(),
        };
        assert!(denied.caps().ops.is_empty() && denied.caps().topics.is_empty());
        assert_eq!(denied.summary().error.as_deref(), Some("broken"));
    }

    #[test]
    fn the_same_content_hashes_the_same_whatever_its_source() {
        let dir = tempfile::tempdir().expect("temp dir");
        let text = "version = 1\nallow = [\"containers.read\", \"logs.read\", \"network.read\", \"power.read\", \"system.read\", \"services.read\", \"processes.read\"]\n";
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
