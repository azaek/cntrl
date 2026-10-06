//! Unix: modes and owners.

use std::fs::{DirBuilder, Metadata, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

/// Who must own a file that says what the agent may do: privd's own user,
/// root when it runs as root.
pub type Owner = u32;

/// The owner of files only root changes.
pub const ROOT: Owner = 0;

/// The owner privd's files must have: its own user.
pub fn own_owner() -> Owner {
    rustix::process::getuid().as_raw()
}

/// Whether this process runs as root.
pub fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

/// Who may change who controls the machine, as a sentence names them.
pub const SUPERUSER: &str = "root";

/// How a hint says to run something as root: "run it with sudo".
pub const AS_ROOT: &str = "with sudo";

/// A command as root, to copy: `` `sudo cntrl pause` ``.
pub fn elevated(command: &str) -> String {
    format!("`sudo {command}`")
}

/// Who ran this command, as this machine names them: whoever ran sudo.
pub fn invoker() -> String {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|user| !user.is_empty())
        .unwrap_or_else(|| SUPERUSER.to_owned())
}

/// Files and directories created with who may read them.
pub trait Private {
    /// Only the owner reads and writes it.
    fn private(&mut self) -> &mut Self;
    /// Everyone reads it, only the owner writes it, as the policy is.
    fn shared(&mut self) -> &mut Self;
}

impl Private for OpenOptions {
    fn private(&mut self) -> &mut Self {
        self.mode(0o600)
    }

    fn shared(&mut self) -> &mut Self {
        self.mode(0o644)
    }
}

impl Private for tokio::fs::OpenOptions {
    fn private(&mut self) -> &mut Self {
        self.mode(0o600)
    }

    fn shared(&mut self) -> &mut Self {
        self.mode(0o644)
    }
}

impl Private for DirBuilder {
    fn private(&mut self) -> &mut Self {
        self.mode(0o700)
    }

    fn shared(&mut self) -> &mut Self {
        self.mode(0o755)
    }
}

/// Whether a file that says what the agent may do can be trusted: it belongs
/// to `owner` and nobody else can write it.
pub fn check_trusted(path: &Path, metadata: &Metadata, owner: Owner) -> Result<(), String> {
    if metadata.uid() != owner {
        return Err(format!("{} must be owned by uid {owner}", path.display()));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "{} must not be writable by group or others",
            path.display()
        ));
    }
    Ok(())
}

/// The account the agent runs as, which the installer creates.
#[cfg(target_os = "macos")]
pub const AGENT_ACCOUNT: &str = "_cntrl";
#[cfg(not(target_os = "macos"))]
pub const AGENT_ACCOUNT: &str = "cntrl";

/// An account: its user ID.
pub type Account = u32;

/// The account this process runs as.
pub fn own_account() -> Option<Account> {
    Some(rustix::process::getuid().as_raw())
}

/// Looks an account up in `/etc/passwd`, where the installer creates it.
#[cfg(not(target_os = "macos"))]
pub fn account(name: &str) -> Option<Account> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == name).then_some(())?;
        fields.nth(1)?.parse().ok()
    })
}

/// Looks an account up through `id`, which asks Directory Services: macOS
/// keeps the accounts the installer creates out of `/etc/passwd`.
#[cfg(target_os = "macos")]
pub fn account(name: &str) -> Option<Account> {
    id_of(name, "-u")
}

/// An account's primary group, through `id`.
#[cfg(target_os = "macos")]
pub fn group_of(name: &str) -> Option<u32> {
    id_of(name, "-g")
}

#[cfg(target_os = "macos")]
fn id_of(name: &str, which: &str) -> Option<u32> {
    let output = std::process::Command::new("/usr/bin/id")
        .args([which, name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// The client's end of a connection to a local endpoint.
pub type LocalStream = tokio::net::UnixStream;

/// The server's end of one.
pub type ServerStream = tokio::net::UnixStream;

/// Connects to a local endpoint.
pub async fn connect(path: &Path) -> std::io::Result<LocalStream> {
    tokio::net::UnixStream::connect(path).await
}

/// A local endpoint for a test: a socket in `dir`.
#[cfg(test)]
pub fn test_endpoint(dir: &Path, name: &str) -> std::path::PathBuf {
    dir.join(format!("{name}.sock"))
}

/// Whether a local endpoint is there to connect to: a socket.
pub fn is_endpoint(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.file_type().is_socket())
}

/// Checks that the agent serves a connection to its endpoint. On Unix the
/// service manager makes the socket, or the agent does in a directory only it
/// and root can write, so nobody else can stand in for it.
pub fn check_agent(_stream: &LocalStream) -> Result<(), String> {
    Ok(())
}

/// The account on the other end of a local connection, from the socket's
/// credentials.
#[derive(Debug, Clone, Default)]
pub struct Peer {
    uid: Option<u32>,
}

impl Peer {
    /// Root, who may change who controls the machine.
    pub fn is_root(&self) -> bool {
        self.uid == Some(0)
    }

    pub fn is(&self, account: &Account) -> bool {
        self.uid == Some(*account)
    }
}

/// A local endpoint that hands each connection over with its peer.
pub struct LocalListener(tokio::net::UnixListener);

impl LocalListener {
    /// The socket the service manager made for this process (systemd's through
    /// `LISTEN_FDS`, launchd's from the job's `Listeners` entry), or else
    /// `path`, bound here with mode 0660: the owner and its group, and root.
    pub fn listen(path: &Path, _endpoint: super::Endpoint) -> Result<Self, String> {
        let listener = match activated()? {
            Some(listener) => {
                listener.set_nonblocking(true).map_err(|e| e.to_string())?;
                tokio::net::UnixListener::from_std(listener).map_err(|e| e.to_string())?
            }
            None => bind(path)?,
        };
        Ok(Self(listener))
    }

    pub async fn accept(&mut self) -> std::io::Result<(ServerStream, Peer)> {
        let (stream, _) = self.0.accept().await?;
        let uid = stream.peer_cred().ok().map(|cred| cred.uid());
        Ok((stream, Peer { uid }))
    }
}

impl axum::serve::Listener for LocalListener {
    type Io = ServerStream;
    type Addr = Peer;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match LocalListener::accept(self).await {
                Ok(accepted) => return accepted,
                Err(e) => {
                    tracing::warn!("accept failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(Peer::default())
    }
}

fn activated() -> Result<Option<std::os::unix::net::UnixListener>, String> {
    #[cfg(target_os = "macos")]
    if let Some(listener) = super::super::launchd::listener("Listeners") {
        return Ok(Some(listener));
    }
    listenfd::ListenFd::from_env()
        .take_unix_listener(0)
        .map_err(|e| e.to_string())
}

/// Binds the socket, replacing a stale one from an earlier run.
fn bind(path: &Path) -> Result<tokio::net::UnixListener, String> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "can't remove the stale socket {}: {e}",
                path.display()
            ));
        }
    }
    let listener = tokio::net::UnixListener::bind(path)
        .map_err(|e| format!("can't bind {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
        .map_err(|e| format!("can't set permissions on {}: {e}", path.display()))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_accounts_by_name() {
        assert_eq!(account("root"), Some(0));
        assert_eq!(account("no-such-user-for-cntrl-tests"), None);
    }
}
