//! Whether someone's using the machine (D121): input in the last
//! [`IN_USE_MINUTES`](cntrl_protocol::followup::IN_USE_MINUTES) minutes. A
//! follow-up's power step guarded by "skip if someone's using it" asks before
//! it runs; `None` is "can't tell", and the step is skipped then, since the
//! person turned the guard on to protect someone at the machine. macOS says
//! how long since the last input, Linux's logind whether each session is idle,
//! and Windows each session's last input, or failing that whether it's locked.

use std::time::Duration;

use cntrl_protocol::followup::IN_USE_MINUTES;

/// Whether someone used the machine recently, or `None` if it can't tell.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub async fn in_use() -> Option<bool> {
    cntrl_host::power::in_use(Duration::from_secs(u64::from(IN_USE_MINUTES) * 60)).await
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
#[allow(clippy::unused_async)]
pub async fn in_use() -> Option<bool> {
    let _ = Duration::from_secs(u64::from(IN_USE_MINUTES) * 60);
    None
}
