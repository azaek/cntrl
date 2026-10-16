//! Whether someone's using the machine (D121): input in the last
//! [`IN_USE_MINUTES`](cntrl_protocol::followup::IN_USE_MINUTES) minutes. A
//! follow-up's power step guarded by "skip if someone's using it" asks before
//! it runs; `None` is "can't tell", and the step is skipped then, since the
//! person turned the guard on to protect someone at the machine.
//!
//! Each OS answers in its own way, built in the agent's next step; until then
//! this can't tell anywhere.

/// Whether someone used the machine recently, or `None` if it can't tell.
#[allow(clippy::unused_async)]
pub async fn in_use() -> Option<bool> {
    None
}
