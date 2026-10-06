//! Services on Windows (angle 14): the Service Control Manager's Win32
//! services, listed by any account, and started, stopped, restarted, enabled
//! and disabled by privd. Enabling sets a service to start with Windows;
//! disabling sets it to start only when asked, as `systemctl disable` leaves a
//! unit startable. A service that stopped with an error has failed, as
//! systemd says.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use cntrl_protocol::service::{
    JobResult, ServiceAction, ServiceKind, ServiceScope, ServiceState, ServiceStatus,
};
use windows_sys::Win32::Foundation::{
    ERROR_MORE_DATA, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_NOT_ACTIVE,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, ENUM_SERVICE_STATUS_PROCESSW,
    ENUM_SERVICE_STATUSW, EnumDependentServicesW, EnumServicesStatusExW, GetServiceDisplayNameW,
    OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_ENUM_PROCESS_INFO, SC_HANDLE,
    SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE, SC_STATUS_PROCESS_INFO, SERVICE_ACTIVE,
    SERVICE_AUTO_START, SERVICE_CHANGE_CONFIG, SERVICE_CONTINUE_PENDING, SERVICE_CONTROL_STOP,
    SERVICE_DEMAND_START, SERVICE_ENUMERATE_DEPENDENTS, SERVICE_NO_CHANGE, SERVICE_PAUSE_PENDING,
    SERVICE_PAUSED, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING,
    SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STATUS_PROCESS, SERVICE_STOP, SERVICE_STOP_PENDING,
    SERVICE_STOPPED, SERVICE_WIN32, StartServiceW,
};

use crate::HostError;

/// How long a start or a stop may take before it's a timeout.
const JOB_WAIT: Duration = Duration::from_secs(60);
/// `ERROR_SERVICE_NEVER_STARTED`: a stopped service that never ran.
const NEVER_STARTED: u32 = 1077;
/// `ERROR_DEPENDENT_SERVICES_RUNNING`.
const DEPENDENTS_RUNNING: u32 = 1051;
/// Where each service's settings live, under `HKEY_LOCAL_MACHINE`.
const SERVICES_KEY: &str = r"SYSTEM\CurrentControlSet\Services";

/// Every Win32 service, by name, with its display name as its description.
pub fn list() -> Result<Vec<ServiceStatus>, HostError> {
    let manager = Handle::manager(SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE)?;
    let mut services = Vec::new();
    let mut resume = 0u32;
    loop {
        let mut needed = 0u32;
        let mut count = 0u32;
        // 64 KiB a call, aligned for the pointers in it.
        let mut buffer = vec![0u64; 8 * 1024];
        let size = u32::try_from(buffer.len() * 8).unwrap_or(u32::MAX);
        // SAFETY: the buffer holds `size` bytes; the entries the call writes
        // point into it.
        let listed = unsafe {
            EnumServicesStatusExW(
                manager.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                buffer.as_mut_ptr().cast(),
                size,
                &mut needed,
                &mut count,
                &mut resume,
                null(),
            )
        };
        let more = listed == 0 && last_error() == ERROR_MORE_DATA;
        if listed == 0 && !more {
            return Err(failed("can't list the services"));
        }
        // SAFETY: the call wrote `count` entries at the buffer's start.
        let entries = unsafe {
            std::slice::from_raw_parts(
                buffer.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(),
                count as usize,
            )
        };
        for entry in entries {
            // SAFETY: the names are NUL-terminated strings in the buffer.
            let (name, display) = unsafe { (text(entry.lpServiceName), text(entry.lpDisplayName)) };
            services.push(status(name, display, &entry.ServiceStatusProcess));
        }
        if !more {
            break;
        }
    }
    services.sort_by_cached_key(|service| service.unit.to_lowercase());
    Ok(services)
}

fn status(name: String, display: String, process: &SERVICE_STATUS_PROCESS) -> ServiceStatus {
    let exit = process.dwWin32ExitCode;
    let (state, detail) = match process.dwCurrentState {
        SERVICE_RUNNING => (ServiceState::Running, "running".to_owned()),
        SERVICE_START_PENDING => (ServiceState::Starting, "starting".to_owned()),
        SERVICE_STOP_PENDING => (ServiceState::Stopping, "stopping".to_owned()),
        SERVICE_PAUSED | SERVICE_PAUSE_PENDING | SERVICE_CONTINUE_PENDING => {
            (ServiceState::Unknown, "paused".to_owned())
        }
        SERVICE_STOPPED if exit == 0 || exit == NEVER_STARTED => {
            (ServiceState::Stopped, "stopped".to_owned())
        }
        SERVICE_STOPPED => (ServiceState::Failed, stopped_with(process)),
        _ => (ServiceState::Unknown, String::new()),
    };
    let start = super::registry_dword(&format!(r"{SERVICES_KEY}\{name}"), "Start");
    let vendor = microsofts(&name);
    ServiceStatus {
        description: (!display.is_empty() && display != name).then_some(display),
        unit: name,
        state,
        detail: (!detail.is_empty()).then_some(detail),
        pid: (process.dwProcessId != 0).then_some(process.dwProcessId),
        protected: false,
        scope: ServiceScope::System,
        user: None,
        kind: ServiceKind::Service,
        // Disabled services can't be started, but they're off at boot too.
        enabled: start.map(|start| start == SERVICE_AUTO_START),
        vendor,
    }
}

/// Whether a service came with Windows (D60): its program is Microsoft's, by
/// the company its version information names, as msconfig's Hide all
/// Microsoft services goes; a service svchost runs is its DLL's. A program
/// can name any company, so this sorts services, it doesn't vouch for them.
fn microsofts(name: &str) -> bool {
    let key = format!(r"{SERVICES_KEY}\{name}");
    let Some(mut file) = super::registry_path(&key, "ImagePath").and_then(|image| program(&image))
    else {
        return false;
    };
    if file.to_ascii_lowercase().ends_with(r"\svchost.exe")
        && let Some(dll) = super::registry_path(&format!(r"{key}\Parameters"), "ServiceDll")
    {
        file = dll;
    }
    static KNOWN: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let mut known = KNOWN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *known.entry(file.to_lowercase()).or_insert_with(|| {
        company(&file).is_some_and(|company| company.to_ascii_lowercase().starts_with("microsoft"))
    })
}

/// The program an `ImagePath` runs: the quoted path, else the text through
/// `.exe`, as Windows finds an unquoted path with spaces in it, and one
/// relative to the Windows folder from there.
fn program(image: &str) -> Option<String> {
    let image = image.trim();
    let path = match image.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => match image.to_ascii_lowercase().find(".exe") {
            Some(end) => &image[..end + 4],
            None => image.split_whitespace().next()?,
        },
    };
    let path = path.strip_prefix(r"\??\").unwrap_or(path);
    if path.is_empty() {
        None
    } else if path.contains(':') || path.starts_with(r"\\") {
        Some(path.to_owned())
    } else {
        Some(format!(
            r"{}\{}",
            super::expand("%SystemRoot%"),
            path.trim_start_matches('\\')
        ))
    }
}

/// The company a file's version information names.
fn company(file: &str) -> Option<String> {
    let path = super::wide(file);
    let mut ignored = 0u32;
    // SAFETY: a NUL-terminated path, and a size query.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut ignored) };
    if size == 0 {
        return None;
    }
    // Aligned for the version information's 16- and 32-bit fields.
    let mut data = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: the buffer holds `size` bytes.
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let value = |name: &str| -> Option<(*const c_void, u32)> {
        let name = super::wide(name);
        let mut pointer: *mut c_void = null_mut();
        let mut length = 0u32;
        // SAFETY: `data` holds the version information, and outlives the
        // pointer into it that the call returns.
        let found = unsafe {
            VerQueryValueW(
                data.as_ptr().cast(),
                name.as_ptr(),
                &mut pointer,
                &mut length,
            )
        };
        (found != 0 && !pointer.is_null() && length > 0).then_some((pointer.cast_const(), length))
    };
    // The first language and code page the file lists, else US English.
    let language = value(r"\VarFileInfo\Translation")
        .filter(|&(_, bytes)| bytes >= 4)
        .map(|(pointer, _)| {
            // SAFETY: the translations are pairs of a 16-bit language and code
            // page, at least one of them.
            let pair = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), 2) };
            format!("{:04x}{:04x}", pair[0], pair[1])
        })
        .unwrap_or_else(|| "040904b0".to_owned());
    let (pointer, length) = value(&format!(r"\StringFileInfo\{language}\CompanyName"))?;
    // SAFETY: a string value is `length` UTF-16 characters, its NUL included.
    let text = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), length as usize) };
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    Some(String::from_utf16_lossy(&text[..end]).trim().to_owned()).filter(|name| !name.is_empty())
}

/// A stopped service's error, as Windows records it.
fn stopped_with(process: &SERVICE_STATUS_PROCESS) -> String {
    // ERROR_SERVICE_SPECIFIC_ERROR: the service's own code says.
    if process.dwWin32ExitCode == 1066 {
        format!(
            "stopped with its error {}",
            process.dwServiceSpecificExitCode
        )
    } else {
        format!(
            "stopped: {}",
            std::io::Error::from_raw_os_error(process.dwWin32ExitCode as i32)
        )
    }
}

/// A service's display name, such as `Print Spooler` for `Spooler`, as the
/// Service Control Manager resolves it.
pub fn display_name(name: &str) -> Option<String> {
    let manager = Handle::manager(SC_MANAGER_CONNECT).ok()?;
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut length = 0u32;
    // SAFETY: a size query, in characters without the NUL.
    unsafe { GetServiceDisplayNameW(manager.0, wide.as_ptr(), null_mut(), &mut length) };
    let mut buffer = vec![0u16; length as usize + 1];
    let mut size = u32::try_from(buffer.len()).ok()?;
    // SAFETY: the buffer holds `size` characters.
    if unsafe { GetServiceDisplayNameW(manager.0, wide.as_ptr(), buffer.as_mut_ptr(), &mut size) }
        == 0
    {
        return None;
    }
    let length = (size as usize).min(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length])).filter(|display| !display.is_empty())
}

/// Starts, stops, restarts, enables or disables a service, waiting for a
/// start or a stop to finish. privd calls it.
pub fn act(name: &str, action: ServiceAction) -> Result<JobResult, HostError> {
    let manager = Handle::manager(SC_MANAGER_CONNECT)?;
    let access = SERVICE_QUERY_STATUS
        | SERVICE_START
        | SERVICE_STOP
        | SERVICE_CHANGE_CONFIG
        | SERVICE_ENUMERATE_DEPENDENTS;
    let service = manager.service(name, access)?;
    match action {
        ServiceAction::Start => start(&service),
        ServiceAction::Stop => stop(&service, name),
        ServiceAction::Restart => match stop(&service, name)? {
            JobResult::Done => start(&service),
            other => Ok(other),
        },
        ServiceAction::Enable => set_start(&service, SERVICE_AUTO_START),
        ServiceAction::Disable => set_start(&service, SERVICE_DEMAND_START),
    }
}

fn start(service: &Handle) -> Result<JobResult, HostError> {
    // SAFETY: an open service handle with SERVICE_START; no arguments.
    if unsafe { StartServiceW(service.0, 0, null()) } == 0
        && last_error() != ERROR_SERVICE_ALREADY_RUNNING
    {
        return Err(failed("Windows didn't start it"));
    }
    wait(service, |state| match state {
        SERVICE_RUNNING => Some(JobResult::Done),
        SERVICE_STOPPED => Some(JobResult::Failed),
        _ => None,
    })
}

fn stop(service: &Handle, name: &str) -> Result<JobResult, HostError> {
    let mut status = SERVICE_STATUS::default();
    // SAFETY: an open service handle with SERVICE_STOP, and a status to fill.
    if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0 {
        match last_error() {
            ERROR_SERVICE_NOT_ACTIVE => return Ok(JobResult::Done),
            DEPENDENTS_RUNNING => {
                return Err(HostError::Invalid(format!(
                    "running services depend on {name}: {}; stop them first",
                    dependents(service).join(", ")
                )));
            }
            _ => return Err(failed("Windows didn't stop it")),
        }
    }
    wait(service, |state| {
        (state == SERVICE_STOPPED).then_some(JobResult::Done)
    })
}

fn set_start(service: &Handle, start: u32) -> Result<JobResult, HostError> {
    // SAFETY: an open service handle with SERVICE_CHANGE_CONFIG; null strings
    // and SERVICE_NO_CHANGE leave the rest as it is.
    let changed = unsafe {
        ChangeServiceConfigW(
            service.0,
            SERVICE_NO_CHANGE,
            start,
            SERVICE_NO_CHANGE,
            null(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
            null(),
        )
    };
    if changed == 0 {
        return Err(failed("Windows didn't change how it starts"));
    }
    Ok(JobResult::Done)
}

/// Checks the service until `done` says how it ended, or it's been too long.
fn wait(service: &Handle, done: impl Fn(u32) -> Option<JobResult>) -> Result<JobResult, HostError> {
    let deadline = Instant::now() + JOB_WAIT;
    loop {
        let state = current_state(service)?;
        if let Some(result) = done(state) {
            return Ok(result);
        }
        if Instant::now() > deadline {
            return Ok(JobResult::Timeout);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn current_state(service: &Handle) -> Result<u32, HostError> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0u32;
    let size = u32::try_from(size_of::<SERVICE_STATUS_PROCESS>()).unwrap_or(u32::MAX);
    // SAFETY: an open service handle with SERVICE_QUERY_STATUS, and a struct
    // of `size` bytes to fill.
    let read = unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&raw mut status).cast(),
            size,
            &mut needed,
        )
    };
    if read == 0 {
        return Err(failed("can't ask Windows how it is"));
    }
    Ok(status.dwCurrentState)
}

/// The running services that depend on this one, by name.
fn dependents(service: &Handle) -> Vec<String> {
    let mut buffer = vec![0u64; 4 * 1024];
    let size = u32::try_from(buffer.len() * 8).unwrap_or(u32::MAX);
    let (mut needed, mut count) = (0u32, 0u32);
    // SAFETY: the buffer holds `size` bytes; the entries point into it.
    let listed = unsafe {
        EnumDependentServicesW(
            service.0,
            SERVICE_ACTIVE,
            buffer.as_mut_ptr().cast(),
            size,
            &mut needed,
            &mut count,
        )
    };
    if listed == 0 {
        return Vec::new();
    }
    // SAFETY: the call wrote `count` entries at the buffer's start.
    let entries = unsafe {
        std::slice::from_raw_parts(
            buffer.as_ptr().cast::<ENUM_SERVICE_STATUSW>(),
            count as usize,
        )
    };
    // SAFETY: the names are NUL-terminated strings in the buffer.
    entries
        .iter()
        .map(|entry| unsafe { text(entry.lpServiceName) })
        .collect()
}

/// A handle to the Service Control Manager or a service, closed when dropped.
struct Handle(SC_HANDLE);

impl Handle {
    fn manager(access: u32) -> Result<Self, HostError> {
        // SAFETY: no machine or database names: this machine's active one.
        let handle = unsafe { OpenSCManagerW(null(), null(), access) };
        if handle.is_null() {
            return Err(failed("can't reach the Service Control Manager"));
        }
        Ok(Self(handle))
    }

    fn service(&self, name: &str, access: u32) -> Result<Self, HostError> {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: an open manager, and a NUL-terminated name.
        let handle = unsafe { OpenServiceW(self.0, wide.as_ptr(), access) };
        if handle.is_null() {
            // ERROR_SERVICE_DOES_NOT_EXIST.
            if last_error() == 1060 {
                return Err(HostError::NotFound(format!("there's no service {name}")));
            }
            return Err(failed(&format!("can't open {name}")));
        }
        Ok(Self(handle))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: an open handle that this owns.
        unsafe { CloseServiceHandle(self.0) };
    }
}

/// A NUL-terminated wide string.
///
/// # Safety
///
/// `text` must be null or point to a NUL-terminated string.
unsafe fn text(text: *mut u16) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: the caller promises a NUL ends it.
    let length = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    // SAFETY: `length` characters precede the NUL.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) })
}

fn last_error() -> u32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
        .unwrap_or(0)
}

fn failed(what: &str) -> HostError {
    HostError::Failed(format!("{what}: {}", std::io::Error::last_os_error()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_program_a_service_runs() {
        let root = crate::windows::expand("%SystemRoot%");
        assert_eq!(
            program(r#""C:\Program Files\NVIDIA Corporation\Display.NvContainer\NVDisplay.Container.exe" -s NVDisplay.ContainerLocalSystem"#).as_deref(),
            Some(r"C:\Program Files\NVIDIA Corporation\Display.NvContainer\NVDisplay.Container.exe")
        );
        assert_eq!(
            program(r"C:\WINDOWS\system32\svchost.exe -k LocalServiceNetworkRestricted -p")
                .as_deref(),
            Some(r"C:\WINDOWS\system32\svchost.exe")
        );
        assert_eq!(
            program(r"C:\Program Files\Some App\app.EXE --service").as_deref(),
            Some(r"C:\Program Files\Some App\app.EXE")
        );
        assert_eq!(
            program(r"system32\svchost.exe -k netsvcs").as_deref(),
            Some(format!(r"{root}\system32\svchost.exe").as_str())
        );
        assert_eq!(program("  "), None);
    }

    #[test]
    fn lists_this_machines_services() {
        let services = list().expect("services");
        let event_log = services
            .iter()
            .find(|service| service.unit.eq_ignore_ascii_case("EventLog"))
            .expect("the Event Log's service");
        assert_eq!(event_log.state, ServiceState::Running);
        assert!(event_log.pid.is_some());
        assert_eq!(event_log.enabled, Some(true));
        // svchost runs it, from Microsoft's wevtsvc.dll.
        assert!(event_log.vendor, "{event_log:?}");
        assert_eq!(
            display_name("EventLog").as_deref(),
            Some("Windows Event Log")
        );
        assert!(matches!(
            act("no-such-service-for-cntrl-tests", ServiceAction::Start),
            Err(HostError::NotFound(_))
        ));
    }
}
