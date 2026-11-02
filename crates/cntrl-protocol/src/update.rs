//! `agent.update`: Console updates the agent to a release it names (D145).
//! The agent fetches that release itself and installs it only if its
//! manifest is signed by cntrl's release key and it's newer than the agent
//! running, so Console can only pick among genuine, newer releases. It
//! answers once the update has started; the agent then restarts on the new
//! version, and if that doesn't come back connected within two minutes, the
//! old one is put back.

use serde::{Deserialize, Serialize};

/// `agent.update`: install this release of the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Update {
    /// The release, such as `0.1.22`: a version newer than the agent's own.
    pub version: String,
}

/// What `agent.update` returns: that the update has started, from which
/// version to which. Done is the next hello carrying `to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateStarted {
    pub from: String,
    pub to: String,
}
