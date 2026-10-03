//! Types for `app.*` operations: apps open in a user's desktop session.

use serde::{Deserialize, Serialize};

/// `app.quit`: one app to quit, named by its bundle ID as `service.list` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppQuit {
    pub app: String,
    /// Whose session; the one user logged in when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Quit at once, as Force Quit does, losing unsaved work. Otherwise the app
    /// is asked to quit the way the Dock asks, and may ask to save first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
}

/// How a quit went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppQuitResult {
    pub app: String,
    pub result: QuitResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum QuitResult {
    /// It closed.
    Quit,
    /// Asked to quit, it's still open: it may be asking to save changes.
    StillOpen,
    /// It wasn't open.
    NotOpen,
    #[serde(other)]
    Unknown,
}
