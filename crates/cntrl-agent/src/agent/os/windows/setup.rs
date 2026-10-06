//! What installing on Windows asks of the system that windows-service doesn't
//! cover (D58): access lists made from SDDL, a service's required privileges,
//! the machine's PATH, and the entry in Apps & features.

use std::path::Path;
use std::ptr::{null, null_mut};

use windows_service::service::Service;
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LPARAM};
use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ,
    REG_OPTION_NON_VOLATILE, REG_SZ, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, QueryServiceStatusEx, SC_STATUS_PROCESS_INFO,
    SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO, SERVICE_REQUIRED_PRIVILEGES_INFOW,
    SERVICE_STATUS_PROCESS,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};

use super::SecurityDescriptor;

/// Where the machine's environment lives, under `HKEY_LOCAL_MACHINE`.
const ENVIRONMENT: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
/// Where Apps & features finds the agent, under `HKEY_LOCAL_MACHINE`.
const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\cntrl-agent";

/// A value in the agent's entry in Apps & features.
pub enum Value<'a> {
    Text(&'a str),
    Number(u32),
}

/// Lists the agent in Apps & features with these values, replacing what was
/// there.
pub fn set_uninstall_entry(values: &[(&str, Value<'_>)]) -> Result<(), String> {
    remove_uninstall_entry()?;
    let key = Key::create(UNINSTALL)?;
    for (name, value) in values {
        match value {
            Value::Text(text) => key.write(name, REG_SZ, &text_bytes(text))?,
            Value::Number(number) => key.write(name, REG_DWORD, &number.to_le_bytes())?,
        }
    }
    Ok(())
}

/// Takes the agent out of Apps & features; it's gone already when it isn't
/// there.
pub fn remove_uninstall_entry() -> Result<(), String> {
    let name = wide(UNINSTALL);
    // SAFETY: a NUL-terminated subkey of an open root.
    let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, name.as_ptr()) };
    // ERROR_FILE_NOT_FOUND: no entry.
    if status != ERROR_SUCCESS && status != 2 {
        return Err(format!(
            "can't remove the agent from Apps & features: {}",
            std::io::Error::from_raw_os_error(status as i32)
        ));
    }
    Ok(())
}

/// A string as the registry keeps it: UTF-16 with its NUL.
fn text_bytes(text: &str) -> Vec<u8> {
    wide(text)
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Gives `path` the access list that `sddl` describes, no longer inheriting
/// its parent's, and passes its inheritable entries to what's inside.
pub fn set_access(path: &Path, sddl: &str) -> Result<(), String> {
    let descriptor = SecurityDescriptor::from_sddl(sddl)?;
    let (mut present, mut defaulted) = (0, 0);
    let mut dacl: *mut ACL = null_mut();
    // SAFETY: the descriptor is valid, and the list it gives points into it.
    let found =
        unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) };
    if found == 0 || present == 0 || dacl.is_null() {
        return Err(format!("{sddl} has no access list"));
    }
    let name = wide(&path.to_string_lossy());
    // SAFETY: the name is NUL-terminated, and the list lives as long as
    // `descriptor`, past the call.
    let status = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            dacl,
            null(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!(
            "can't set who may use {}: {}",
            path.display(),
            std::io::Error::from_raw_os_error(status as i32)
        ));
    }
    Ok(())
}

/// Keeps only `privileges` in the service's token from its next start; the
/// Service Control Manager removes the rest, but for
/// `SeChangeNotifyPrivilege`.
pub fn require_privileges(service: &Service, privileges: &[&str]) -> Result<(), String> {
    // A list of strings, each NUL-terminated, ending in another NUL.
    let mut list: Vec<u16> = privileges
        .iter()
        .flat_map(|privilege| privilege.encode_utf16().chain(Some(0)))
        .chain(Some(0))
        .collect();
    let info = SERVICE_REQUIRED_PRIVILEGES_INFOW {
        pmszRequiredPrivileges: list.as_mut_ptr(),
    };
    // SAFETY: the handle is open with SERVICE_CHANGE_CONFIG, and the list
    // outlives the call.
    let changed = unsafe {
        ChangeServiceConfig2W(
            service.raw_handle(),
            SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO,
            (&raw const info).cast(),
        )
    };
    if changed == 0 {
        return Err(format!(
            "can't set the service's privileges: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// A service's process, whatever its state: windows-service gives it only
/// while the service runs.
pub fn service_process(service: &Service) -> Option<u32> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0u32;
    let size = u32::try_from(size_of::<SERVICE_STATUS_PROCESS>()).ok()?;
    // SAFETY: an open handle with SERVICE_QUERY_STATUS, and a struct of
    // `size` bytes to fill.
    let read = unsafe {
        QueryServiceStatusEx(
            service.raw_handle(),
            SC_STATUS_PROCESS_INFO,
            (&raw mut status).cast(),
            size,
            &mut needed,
        )
    };
    (read != 0 && status.dwProcessId != 0).then_some(status.dwProcessId)
}

/// Ends a process at once, as a service that doesn't stop when asked is.
pub fn end_process(pid: u32) -> Result<(), String> {
    // SAFETY: a plain call; the handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if process.is_null() {
        return Err(format!(
            "can't open process {pid}: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: an open handle with PROCESS_TERMINATE, closed once.
    let ended = unsafe {
        let ended = TerminateProcess(process, 1);
        windows_sys::Win32::Foundation::CloseHandle(process);
        ended
    };
    if ended == 0 {
        return Err(format!(
            "can't end process {pid}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Deletes a file or an empty directory when Windows next starts: what can't
/// go now, as a running program can't.
pub fn delete_at_restart(path: &Path) -> Result<(), String> {
    let name = wide(&path.to_string_lossy());
    // SAFETY: the name is NUL-terminated; no new name means deleting.
    if unsafe { MoveFileExW(name.as_ptr(), null(), MOVEFILE_DELAY_UNTIL_REBOOT) } == 0 {
        return Err(format!(
            "can't delete {} at the next restart: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Puts `dir` on the machine's PATH, or with `on` false takes it off, and
/// tells running programs, so terminals opened after see it.
pub fn set_on_path(dir: &Path, on: bool) -> Result<(), String> {
    let key = Key::open(ENVIRONMENT)?;
    let path = key.read("Path")?;
    let dir = dir.to_string_lossy();
    let same = |entry: &str| {
        entry
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(dir.trim_end_matches('\\'))
    };
    let entries: Vec<&str> = path.split(';').filter(|entry| !entry.is_empty()).collect();
    if entries.iter().any(|entry| same(entry)) == on {
        return Ok(());
    }
    let mut kept: Vec<&str> = entries.into_iter().filter(|entry| !same(entry)).collect();
    if on {
        kept.push(&dir);
    }
    key.write_expandable("Path", &kept.join(";"))?;
    let environment = wide("Environment");
    let mut result = 0;
    // SAFETY: the string outlives the call, which waits at most 5 s for each
    // window to answer.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        )
    };
    Ok(())
}

/// An open key under `HKEY_LOCAL_MACHINE`, closed when dropped.
struct Key(HKEY);

impl Key {
    /// Creates the key, or opens it when it's there, to write.
    fn create(path: &str) -> Result<Self, String> {
        let name = wide(path);
        let mut key: HKEY = null_mut();
        // SAFETY: the name is NUL-terminated, and the key is closed on drop.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                name.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                null(),
                &mut key,
                null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "can't create {path}: {}",
                std::io::Error::from_raw_os_error(status as i32)
            ));
        }
        Ok(Self(key))
    }

    /// Writes a value of `kind` from its bytes.
    fn write(&self, value: &str, kind: u32, data: &[u8]) -> Result<(), String> {
        let name = wide(value);
        let size = u32::try_from(data.len()).map_err(|_| format!("{value} is too long"))?;
        // SAFETY: the name is NUL-terminated, and the data `size` bytes long.
        let status = unsafe { RegSetValueExW(self.0, name.as_ptr(), 0, kind, data.as_ptr(), size) };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "can't write {value}: {}",
                std::io::Error::from_raw_os_error(status as i32)
            ));
        }
        Ok(())
    }

    fn open(path: &str) -> Result<Self, String> {
        let name = wide(path);
        let mut key: HKEY = null_mut();
        // SAFETY: the name is NUL-terminated, and the key is closed on drop.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                name.as_ptr(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "can't open {path}: {}",
                std::io::Error::from_raw_os_error(status as i32)
            ));
        }
        Ok(Self(key))
    }

    /// A string value as stored, its `%VARIABLES%` unexpanded.
    fn read(&self, value: &str) -> Result<String, String> {
        let name = wide(value);
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let mut size = 0u32;
        // SAFETY: a size query.
        let status = unsafe {
            RegGetValueW(
                self.0,
                null(),
                name.as_ptr(),
                flags,
                null_mut(),
                null_mut(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "can't read {value}: {}",
                std::io::Error::from_raw_os_error(status as i32)
            ));
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: the buffer holds `size` bytes.
        let status = unsafe {
            RegGetValueW(
                self.0,
                null(),
                name.as_ptr(),
                flags,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "can't read {value}: {}",
                std::io::Error::from_raw_os_error(status as i32)
            ));
        }
        let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Ok(String::from_utf16_lossy(&buffer[..length]))
    }

    fn write_expandable(&self, value: &str, text: &str) -> Result<(), String> {
        let name = wide(value);
        let data = wide(text);
        let bytes = u32::try_from(data.len() * 2).map_err(|_| format!("{value} is too long"))?;
        // SAFETY: the data is NUL-terminated and `bytes` long.
        let status = unsafe {
            RegSetValueExW(
                self.0,
                name.as_ptr(),
                0,
                REG_EXPAND_SZ,
                data.as_ptr().cast(),
                bytes,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "can't write {value}: {}",
                std::io::Error::from_raw_os_error(status as i32)
            ));
        }
        Ok(())
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: an open key that this owns.
        unsafe { RegCloseKey(self.0) };
    }
}
