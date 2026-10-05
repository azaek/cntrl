//! Pausing the agent on its machine (`cntrl pause`, D46). The agent tells the
//! gateway who paused it and why, hangs up, and stays away until `cntrl
//! resume`, as `tailscale down` disconnects until `tailscale up`. The pause is
//! a file in the agent's state directory, so it holds across restarts.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

const PAUSED_FILE: &str = "paused.json";

/// Who paused the agent, why, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PausedState {
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub since_ms: u64,
}

/// The pause in force, if any. A file that doesn't read still pauses: someone
/// meant it, and `cntrl resume` clears it.
pub fn load(state_dir: &Path) -> Option<PausedState> {
    let text = fs::read_to_string(state_dir.join(PAUSED_FILE)).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|_| PausedState {
        by: "someone".to_owned(),
        reason: None,
        since_ms: 0,
    }))
}

/// Saves the pause: a temporary file renamed into place, so a crash leaves
/// either the old state or the new.
pub fn save(state_dir: &Path, state: &PausedState) -> io::Result<()> {
    let path = state_dir.join(PAUSED_FILE);
    let temporary = state_dir.join(format!("{PAUSED_FILE}.tmp"));
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec(state).map_err(io::Error::other)?)?;
    file.sync_all()?;
    fs::rename(&temporary, &path)
}

/// Ends the pause; true when there was one.
pub fn clear(state_dir: &Path) -> io::Result<bool> {
    match fs::remove_file(state_dir.join(PAUSED_FILE)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pause_holds_until_cleared() {
        let dir = tempfile::tempdir().expect("a directory");
        assert_eq!(load(dir.path()), None);
        let state = PausedState {
            by: "alok".to_owned(),
            reason: Some("replacing the disk".to_owned()),
            since_ms: 1_700_000_000_000,
        };
        save(dir.path(), &state).expect("saved");
        assert_eq!(load(dir.path()), Some(state));
        assert!(clear(dir.path()).expect("cleared"));
        assert_eq!(load(dir.path()), None);
        assert!(!clear(dir.path()).expect("nothing to clear"));
    }

    #[test]
    fn an_unreadable_pause_still_pauses() {
        let dir = tempfile::tempdir().expect("a directory");
        fs::write(dir.path().join(PAUSED_FILE), "not json").expect("written");
        assert_eq!(
            load(dir.path()).map(|state| state.by),
            Some("someone".to_owned())
        );
    }
}
