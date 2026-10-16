//! Power on Windows (angles 06 and 10): restarting and shutting down through
//! InitiateShutdownW, and sleeping through SetSuspendState, as privd with
//! SeShutdownPrivilege enabled; never ExitWindowsEx, which acts on the
//! caller's own session. Who's signed in comes from Remote Desktop Services'
//! sessions.

use std::ptr::{null, null_mut};
use std::time::Duration;

use cntrl_protocol::power::{PowerAction, PowerInfo, Session};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE, LUID,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
};
use windows_sys::Win32::System::Power::{
    GetPwrCapabilities, PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired,
    PowerSetRequest, SYSTEM_POWER_CAPABILITIES, SetSuspendState,
};
use windows_sys::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_INFO_CLASS, WTS_SESSION_INFOW, WTS_SESSIONSTATE_UNLOCK,
    WTSActive, WTSClientName, WTSClientProtocolType, WTSDisconnected, WTSEnumerateSessionsW,
    WTSFreeMemory, WTSGetActiveConsoleSessionId, WTSINFOEXW, WTSQuerySessionInformationW,
    WTSSessionInfoEx, WTSUserName,
};
use windows_sys::Win32::System::Shutdown::{
    InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER,
    SHTDN_REASON_MINOR_OTHER, SHUTDOWN_FORCE_OTHERS, SHUTDOWN_FORCE_SELF, SHUTDOWN_POWEROFF,
    SHUTDOWN_RESTART,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};

use crate::HostError;
use crate::power::unavailable;

/// Remote Desktop's protocol, as WTSClientProtocolType names it.
const RDP: u16 = 2;
/// REASON_CONTEXT's version, from SystemServices.
const POWER_REQUEST_CONTEXT_VERSION: u32 = 0;

pub async fn info() -> Result<PowerInfo, HostError> {
    tokio::task::spawn_blocking(|| PowerInfo {
        actions: actions(),
        sessions: sessions(),
        inhibitors: Vec::new(),
        unlock_after_restart: None,
        restarts_after_power_loss: None,
        wake_on_lan: None,
    })
    .await
    .map_err(|e| HostError::Failed(e.to_string()))
}

pub async fn check(action: PowerAction) -> Result<(), HostError> {
    if actions().contains(&action) {
        Ok(())
    } else {
        Err(unavailable(action))
    }
}

/// Restarts and shutdowns start at once, signing everyone out without asking
/// their programs to save, as `shutdown` does elsewhere (angle 06 §3). Sleep
/// returns only once the machine wakes, so it starts on its own thread a
/// moment later, after privd has answered.
pub async fn act(action: PowerAction) -> Result<(), HostError> {
    check(action).await?;
    enable_shutdown()?;
    match action {
        PowerAction::Reboot | PowerAction::Poweroff => {
            let (flag, message) = if action == PowerAction::Reboot {
                (SHUTDOWN_RESTART, "Restarting, as asked in cntrl Console")
            } else {
                (
                    SHUTDOWN_POWEROFF,
                    "Shutting down, as asked in cntrl Console",
                )
            };
            let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
            // SAFETY: the message is NUL-terminated; no machine name means
            // this one.
            let status = unsafe {
                InitiateShutdownW(
                    null(),
                    message.as_ptr(),
                    0,
                    flag | SHUTDOWN_FORCE_OTHERS | SHUTDOWN_FORCE_SELF,
                    SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(HostError::Failed(format!(
                    "Windows didn't start it: {}",
                    std::io::Error::from_raw_os_error(status as i32)
                )));
            }
            Ok(())
        }
        PowerAction::Suspend | PowerAction::Hibernate => {
            let hibernate = action == PowerAction::Hibernate;
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                // SAFETY: a plain call.
                unsafe { SetSuspendState(hibernate, false, false) };
            });
            Ok(())
        }
    }
}

/// What this machine can do: restart and shut down always; sleep where it
/// has S1 to S3, which a Modern Standby machine hasn't, and it can't be put
/// to sleep this way; hibernate where it has S4 and a hibernation file.
fn actions() -> Vec<PowerAction> {
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    // SAFETY: a struct to fill.
    let read = unsafe { GetPwrCapabilities(&mut caps) };
    let mut actions = vec![PowerAction::Reboot, PowerAction::Poweroff];
    if read && (caps.SystemS1 || caps.SystemS2 || caps.SystemS3) {
        actions.push(PowerAction::Suspend);
    }
    if read && caps.SystemS4 && caps.HiberFilePresent {
        actions.push(PowerAction::Hibernate);
    }
    actions
}

/// Enables SeShutdownPrivilege in this process's token. Its service keeps
/// the privilege, disabled until it's needed.
fn enable_shutdown() -> Result<(), HostError> {
    let failed = |what: &str| {
        HostError::Failed(format!("can't {what}: {}", std::io::Error::last_os_error()))
    };
    let name: Vec<u16> = "SeShutdownPrivilege"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut luid = LUID::default();
    // SAFETY: the name is NUL-terminated.
    if unsafe { LookupPrivilegeValueW(null(), name.as_ptr(), &mut luid) } == 0 {
        return Err(failed("find the shutdown privilege"));
    }
    let mut token: HANDLE = null_mut();
    // SAFETY: this process's own pseudo-handle; the token is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token) } == 0 {
        return Err(failed("open this process's token"));
    }
    let _token = super::Handle(token);
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // SAFETY: an open token with TOKEN_ADJUST_PRIVILEGES, and one privilege.
    let adjusted =
        unsafe { AdjustTokenPrivileges(token, 0, &privileges, 0, null_mut(), null_mut()) };
    // It succeeds without the privilege, saying so only in the last error.
    let error = std::io::Error::last_os_error();
    if adjusted == 0 || error.raw_os_error() != Some(0) {
        return Err(HostError::Failed(format!(
            "privd may not shut the machine down: {error}"
        )));
    }
    Ok(())
}

/// Who's signed in: each session with a user, at the console or over Remote
/// Desktop, connected or not, since a shutdown ends both.
fn sessions() -> Vec<Session> {
    let mut list: *mut WTS_SESSION_INFOW = null_mut();
    let mut count = 0u32;
    // SAFETY: the list is freed below.
    if unsafe { WTSEnumerateSessionsW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut list, &mut count) } == 0
    {
        return Vec::new();
    }
    // SAFETY: the system returned `count` entries.
    let entries = unsafe { std::slice::from_raw_parts(list, count as usize) };
    // SAFETY: a plain call.
    let console = unsafe { WTSGetActiveConsoleSessionId() };
    let sessions = entries
        .iter()
        .filter(|entry| entry.State == WTSActive || entry.State == WTSDisconnected)
        .filter_map(|entry| {
            let user = query(entry.SessionId, WTSUserName)
                .map(|bytes| text(&bytes))
                .filter(|user| !user.is_empty())?;
            let remote = query(entry.SessionId, WTSClientProtocolType)
                .and_then(|bytes| Some(u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?])))
                == Some(RDP);
            let place = if remote {
                query(entry.SessionId, WTSClientName)
                    .map(|bytes| text(&bytes))
                    .filter(|name| !name.is_empty())
            } else if entry.SessionId == console {
                Some("console".to_owned())
            } else {
                None
            };
            Some(Session {
                user,
                place,
                remote,
            })
        })
        .collect();
    // SAFETY: the list from WTSEnumerateSessionsW, freed once.
    unsafe { WTSFreeMemory(list.cast()) };
    sessions
}

/// Whether someone's using the machine (D121): input within `within` in a
/// signed-in session, at the console or over Remote Desktop. Some versions
/// of Windows leave a session's last input at zero; then a signed-in,
/// unlocked session counts as someone there. None when no session could be
/// read.
pub async fn in_use(within: Duration) -> Option<bool> {
    tokio::task::spawn_blocking(move || input_within(within))
        .await
        .ok()?
}

fn input_within(within: Duration) -> Option<bool> {
    let mut list: *mut WTS_SESSION_INFOW = null_mut();
    let mut count = 0u32;
    // SAFETY: the list is freed below.
    if unsafe { WTSEnumerateSessionsW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut list, &mut count) } == 0
    {
        return None;
    }
    // SAFETY: the system returned `count` entries.
    let entries = unsafe { std::slice::from_raw_parts(list, count as usize) };
    let ids: Vec<u32> = entries
        .iter()
        .filter(|entry| entry.State == WTSActive)
        .map(|entry| entry.SessionId)
        .collect();
    // SAFETY: the list from WTSEnumerateSessionsW, freed once.
    unsafe { WTSFreeMemory(list.cast()) };
    // FILETIME's 100 ns.
    let window = i64::try_from(within.as_nanos() / 100).unwrap_or(i64::MAX);
    let mut unsure = false;
    for id in ids {
        let Some(bytes) = query(id, WTSSessionInfoEx) else {
            unsure = true;
            continue;
        };
        if bytes.len() < std::mem::size_of::<WTSINFOEXW>() {
            unsure = true;
            continue;
        }
        // SAFETY: the system wrote a WTSINFOEXW; the copy is read unaligned,
        // since a Vec<u8> promises no alignment.
        let info: WTSINFOEXW = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
        if info.Level != 1 {
            unsure = true;
            continue;
        }
        // SAFETY: level 1 is the union's only member.
        let session = unsafe { info.Data.WTSInfoExLevel1 };
        // Nobody signed in, as at the sign-in screen.
        if session.UserName[0] == 0 {
            continue;
        }
        if session.LastInputTime > 0 && session.CurrentTime > 0 {
            if session.CurrentTime - session.LastInputTime < window {
                return Some(true);
            }
        } else if u32::try_from(session.SessionFlags) == Ok(WTS_SESSIONSTATE_UNLOCK) {
            return Some(true);
        }
    }
    (!unsure).then_some(false)
}

/// The machine kept from sleeping on its own, until this is dropped: a power
/// request that the system is needed, the way a service asks, which
/// `powercfg /requests` lists.
pub struct Awake(usize);

#[allow(clippy::unused_async)]
pub async fn keep_awake(reason: &str) -> Result<Awake, HostError> {
    let mut wide: Vec<u16> = reason.encode_utf16().chain(Some(0)).collect();
    let context = REASON_CONTEXT {
        Version: POWER_REQUEST_CONTEXT_VERSION,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 {
            SimpleReasonString: wide.as_mut_ptr(),
        },
    };
    // SAFETY: the context and its NUL-terminated string outlive the call,
    // which copies the string.
    let request = unsafe { PowerCreateRequest(&context) };
    if request.is_null() || request == INVALID_HANDLE_VALUE {
        let e = std::io::Error::last_os_error();
        return Err(HostError::Failed(format!(
            "can't ask Windows to stay awake: {e}"
        )));
    }
    // SAFETY: the request just made.
    if unsafe { PowerSetRequest(request, PowerRequestSystemRequired) } == 0 {
        let e = std::io::Error::last_os_error();
        // SAFETY: closed once, here.
        unsafe { CloseHandle(request) };
        return Err(HostError::Failed(format!(
            "can't ask Windows to stay awake: {e}"
        )));
    }
    Ok(Awake(request as usize))
}

impl Drop for Awake {
    fn drop(&mut self) {
        let request = self.0 as HANDLE;
        // SAFETY: the request this owns, cleared and closed once.
        unsafe {
            PowerClearRequest(request, PowerRequestSystemRequired);
            CloseHandle(request);
        }
    }
}

/// One piece of a session's information, as bytes.
fn query(session: u32, class: WTS_INFO_CLASS) -> Option<Vec<u8>> {
    let mut buffer: *mut u16 = null_mut();
    let mut size = 0u32;
    // SAFETY: the buffer is freed below.
    let read = unsafe {
        WTSQuerySessionInformationW(
            WTS_CURRENT_SERVER_HANDLE,
            session,
            class,
            &mut buffer,
            &mut size,
        )
    };
    if read == 0 || buffer.is_null() {
        return None;
    }
    // SAFETY: the system returned `size` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), size as usize) }.to_vec();
    // SAFETY: allocated by WTSQuerySessionInformationW, freed once.
    unsafe { WTSFreeMemory(buffer.cast()) };
    Some(bytes)
}

/// UTF-16 bytes up to their NUL.
fn text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}
