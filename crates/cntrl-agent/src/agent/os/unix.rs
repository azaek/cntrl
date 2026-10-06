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
