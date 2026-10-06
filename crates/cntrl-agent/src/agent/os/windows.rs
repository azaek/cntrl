//! Windows: access lists. The installer gives `%ProgramData%\cntrl` a
//! protected list, SYSTEM and Administrators with full control and the agent's
//! service reading, inherited by what's made inside it, so a file needs no
//! mode of its own (angle 04).
//!
//! Windows' security calls are C, so this module may use `unsafe`; each block
//! says why it's sound.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::fs::{DirBuilder, Metadata, OpenOptions};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, GENERIC_ALL, GENERIC_WRITE, LocalFree};
use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, IsWellKnownSid,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, WinBuiltinAdministratorsSid,
    WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, WRITE_DAC,
    WRITE_OWNER,
};
use windows_sys::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE,
    ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE, ACCESS_ALLOWED_COMPOUND_ACE_TYPE,
    ACCESS_ALLOWED_OBJECT_ACE_TYPE,
};

/// Who must own a file that says what the agent may do: on Windows, the
/// Administrators group or SYSTEM, whichever created it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner;

/// The owner of files only administrators change.
pub const ROOT: Owner = Owner;

/// The owner privd's files must have.
pub fn own_owner() -> Owner {
    Owner
}

/// Files and directories created with who may read them: on Windows, what
/// they inherit from the install's directory.
pub trait Private {
    fn private(&mut self) -> &mut Self;
    fn shared(&mut self) -> &mut Self;
}

impl Private for OpenOptions {
    fn private(&mut self) -> &mut Self {
        self
    }

    fn shared(&mut self) -> &mut Self {
        self
    }
}

impl Private for tokio::fs::OpenOptions {
    fn private(&mut self) -> &mut Self {
        self
    }

    fn shared(&mut self) -> &mut Self {
        self
    }
}

impl Private for DirBuilder {
    fn private(&mut self) -> &mut Self {
        self
    }

    fn shared(&mut self) -> &mut Self {
        self
    }
}

/// Any right that changes a file, its attributes or who may.
const WRITES: u32 = GENERIC_ALL
    | GENERIC_WRITE
    | FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_WRITE_EA
    | FILE_WRITE_ATTRIBUTES
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER;

/// Whether a file that says what the agent may do can be trusted: owned by
/// Administrators or SYSTEM, with no access list entry that lets anyone else
/// change it. Entries that deny only narrow access, and those that don't
/// grant it, such as audit entries, are fine; an allowing entry of a rarer
/// kind isn't, as it can't be read here.
pub fn check_trusted(path: &Path, _metadata: &Metadata, _owner: Owner) -> Result<(), String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the name is NUL-terminated, the out pointers are valid, and the
    // descriptor they point into is freed by `Descriptor` below.
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!(
            "can't read who may change {}: error {status}",
            path.display()
        ));
    }
    let _descriptor = Descriptor(descriptor);
    if !administrative(owner) {
        return Err(format!(
            "{} must be owned by Administrators or SYSTEM",
            path.display()
        ));
    }
    if dacl.is_null() {
        // No list at all lets everyone do anything.
        return Err(format!("{} has no access list", path.display()));
    }
    // SAFETY: `dacl` points into the descriptor, alive for this function.
    let count = unsafe { (*dacl).AceCount };
    for index in 0..u32::from(count) {
        let mut ace: *mut c_void = null_mut();
        // SAFETY: `index` is below the list's count.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(format!("can't read {}'s access list", path.display()));
        }
        // SAFETY: every entry starts with a header.
        let kind = u32::from(unsafe { (*ace.cast::<ACE_HEADER>()).AceType });
        if kind == ACCESS_ALLOWED_ACE_TYPE {
            // SAFETY: an entry of this kind is an ACCESS_ALLOWED_ACE whose SID
            // starts at `SidStart`.
            let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            let sid: PSID = (&raw const allowed.SidStart).cast_mut().cast();
            if !administrative(sid) && allowed.Mask & WRITES != 0 {
                return Err(format!(
                    "{} must not be writable by anyone but Administrators and SYSTEM",
                    path.display()
                ));
            }
        } else if [
            ACCESS_ALLOWED_COMPOUND_ACE_TYPE,
            ACCESS_ALLOWED_OBJECT_ACE_TYPE,
            ACCESS_ALLOWED_CALLBACK_ACE_TYPE,
            ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE,
        ]
        .contains(&kind)
        {
            return Err(format!(
                "{} has an access list entry cntrl can't judge",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Whether a SID is SYSTEM or the Administrators group.
fn administrative(sid: PSID) -> bool {
    if sid.is_null() {
        return false;
    }
    // SAFETY: `sid` is a valid SID from the descriptor.
    unsafe {
        IsWellKnownSid(sid, WinLocalSystemSid) != 0
            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
    }
}

/// A security descriptor from `GetNamedSecurityInfoW`, freed when dropped.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the system allocated it with LocalAlloc.
            unsafe { LocalFree(self.0) };
        }
    }
}
