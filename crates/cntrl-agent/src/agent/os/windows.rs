//! Windows: access lists, accounts by SID, and named pipes. The installer
//! gives `%ProgramData%\cntrl` a protected list, SYSTEM and Administrators with
//! full control and the agent's service reading, inherited by what's made
//! inside it, so a file needs no mode of its own (angle 04).
//!
//! privd, as LocalSystem, serves its pipe under
//! `\\.\pipe\ProtectedPrefix\Administrators\`, where only administrators can
//! create one, so nobody can stand in for it. The agent's account can't create
//! pipes there, so its pipe is outside, and the CLI checks who serves it
//! before sending anything (D58).
//!
//! Windows' security calls are C, so this module may use `unsafe`; each block
//! says why it's sound.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::fs::{DirBuilder, Metadata, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::Duration;

use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, ERROR_SUCCESS, GENERIC_ALL, GENERIC_WRITE, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation,
    IsWellKnownSid, LookupAccountNameW, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES, SID_NAME_USE, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS,
    TOKEN_QUERY, TOKEN_USER, TokenGroups, TokenUser, WinBuiltinAdministratorsSid,
    WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_CREATE_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
    SECURITY_IDENTIFICATION, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
use windows_sys::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE,
    ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE, ACCESS_ALLOWED_COMPOUND_ACE_TYPE,
    ACCESS_ALLOWED_OBJECT_ACE_TYPE, SE_GROUP_ENABLED,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::Endpoint;

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

/// The account the agent runs as: its service's virtual account.
pub const AGENT_ACCOUNT: &str = r"NT SERVICE\cntrl-agent";

/// An account: its SID, as text.
pub type Account = String;

/// The account this process runs as.
pub fn own_account() -> Option<Account> {
    process_account(std::process::id()).sid
}

/// Looks an account up by name, such as the agent's.
pub fn account(name: &str) -> Option<Account> {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut sid_size = 0u32;
    let mut domain_size = 0u32;
    let mut kind: SID_NAME_USE = 0;
    // SAFETY: a size query, which fails with the sizes needed.
    unsafe {
        LookupAccountNameW(
            null(),
            wide.as_ptr(),
            null_mut(),
            &mut sid_size,
            null_mut(),
            &mut domain_size,
            &mut kind,
        )
    };
    if sid_size == 0 {
        return None;
    }
    // u32s, for a SID's alignment.
    let mut sid = vec![0u32; (sid_size as usize).div_ceil(4)];
    let mut domain = vec![0u16; domain_size as usize];
    // SAFETY: the buffers hold the sizes asked for.
    let found = unsafe {
        LookupAccountNameW(
            null(),
            wide.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        )
    };
    if found == 0 {
        return None;
    }
    sid_text(sid.as_mut_ptr().cast())
}

/// The client's end of a connection to a local endpoint.
pub type LocalStream = NamedPipeClient;

/// The server's end of one.
pub type ServerStream = NamedPipeServer;

/// What a client may do with a pipe: read and write, but not create an
/// instance of it, which would let it answer the next client in the server's
/// place.
const CLIENT_RIGHTS: u32 = FILE_GENERIC_READ | (FILE_GENERIC_WRITE & !FILE_CREATE_PIPE_INSTANCE);

/// Connects to a local endpoint, waiting out a moment when every instance is
/// busy. The server may identify the client, never act as it.
pub async fn connect(path: &Path) -> io::Result<LocalStream> {
    let mut busy = 0;
    loop {
        let opened = OpenOptions::new()
            .access_mode(CLIENT_RIGHTS)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(path);
        match opened {
            // SAFETY: a pipe's client handle, opened for overlapped I/O, that
            // the client now owns.
            Ok(file) => return unsafe { NamedPipeClient::from_raw_handle(file.into_raw_handle()) },
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && busy < 50 => {
                busy += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Checks that the agent, or SYSTEM, serves a connection to the agent's pipe:
/// outside the protected prefix, whoever made it first could.
pub fn check_agent(stream: &LocalStream) -> Result<(), String> {
    let mut pid = 0u32;
    // SAFETY: the handle is a pipe's open client end.
    if unsafe { GetNamedPipeServerProcessId(stream.as_raw_handle(), &mut pid) } == 0 {
        return Err("can't tell which program serves the agent's pipe".to_owned());
    }
    let server = process_account(pid);
    let agent = account(AGENT_ACCOUNT);
    if server.system || (agent.is_some() && server.sid == agent) {
        Ok(())
    } else {
        Err("another program serves the agent's pipe, so cntrl won't talk to it".to_owned())
    }
}

/// The account on the other end of a local connection, from its process's
/// token.
#[derive(Debug, Clone, Default)]
pub struct Peer {
    sid: Option<Account>,
    system: bool,
    /// An enabled member of Administrators: elevated.
    administrator: bool,
}

impl Peer {
    /// SYSTEM or an elevated administrator, who may change who controls the
    /// machine.
    pub fn is_root(&self) -> bool {
        self.system || self.administrator
    }

    pub fn is(&self, account: &Account) -> bool {
        self.sid.as_ref() == Some(account)
    }
}

/// A local endpoint that hands each connection over with its peer.
pub struct LocalListener {
    path: PathBuf,
    descriptor: SecurityDescriptor,
    /// The instance waiting for the next client.
    next: NamedPipeServer,
}

impl LocalListener {
    /// Creates the pipe at `path`, failing if another program made it first.
    /// SYSTEM and Administrators may do anything with it; the agent's account
    /// may connect to privd's, and serves its own.
    pub fn listen(path: &Path, endpoint: Endpoint) -> Result<Self, String> {
        let mut sddl = String::from("D:P(A;;GA;;;SY)(A;;GA;;;BA)");
        if let Some(agent) = account(AGENT_ACCOUNT) {
            match endpoint {
                Endpoint::Privd => sddl.push_str(&format!("(A;;{CLIENT_RIGHTS:#x};;;{agent})")),
                Endpoint::Agent => sddl.push_str(&format!("(A;;GA;;;{agent})")),
            }
        }
        let descriptor = SecurityDescriptor::from_sddl(&sddl)?;
        let next = create(path, &descriptor, true)
            .map_err(|e| format!("can't create the pipe {}: {e}", path.display()))?;
        Ok(Self {
            path: path.to_owned(),
            descriptor,
            next,
        })
    }

    pub async fn accept(&mut self) -> io::Result<(ServerStream, Peer)> {
        let connected = self.next.connect().await;
        // A new instance waits for the next client either way: one whose client
        // left before it connected can't be used again.
        let fresh = create(&self.path, &self.descriptor, false)?;
        let instance = std::mem::replace(&mut self.next, fresh);
        connected?;
        let mut pid = 0u32;
        // SAFETY: the handle is a pipe's connected server end.
        let found = unsafe { GetNamedPipeClientProcessId(instance.as_raw_handle(), &mut pid) };
        let peer = if found != 0 {
            process_account(pid)
        } else {
            Peer::default()
        };
        Ok((instance, peer))
    }
}

impl axum::serve::Listener for LocalListener {
    type Io = ServerStream;
    type Addr = Peer;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match LocalListener::accept(self).await {
                Ok(accepted) => return accepted,
                Err(e) => {
                    tracing::warn!("accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(Peer::default())
    }
}

/// One instance of the pipe, for clients on this machine only.
fn create(
    path: &Path,
    descriptor: &SecurityDescriptor,
    first: bool,
) -> io::Result<NamedPipeServer> {
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: the attributes, and the descriptor they point to, outlive the
    // call.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(path, (&raw mut attributes).cast())
    }
}

/// A security descriptor made from SDDL, freed when dropped.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor isn't changed once made, and is freed once.
unsafe impl Send for SecurityDescriptor {}
// SAFETY: as above.
unsafe impl Sync for SecurityDescriptor {}

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> Result<Self, String> {
        let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: the string is NUL-terminated and the out pointer valid.
        let made = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        if made == 0 || descriptor.is_null() {
            return Err(format!("can't make a security descriptor from {sddl}"));
        }
        Ok(Self(descriptor))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: the system allocated it with LocalAlloc.
        unsafe { LocalFree(self.0) };
    }
}

/// A process's account: its user's SID, whether that's SYSTEM, and whether
/// Administrators is enabled in its token. A process that's gone, or can't be
/// opened, is nobody.
fn process_account(pid: u32) -> Peer {
    let mut peer = Peer::default();
    // SAFETY: each call gets valid arguments; `Handle` closes each handle
    // when it goes out of scope.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return peer;
        }
        let _process = Handle(process);
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return peer;
        }
        let _token = Handle(token);
        if let Some(buffer) = token_information(token, TokenUser) {
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            peer.sid = sid_text(user.User.Sid);
            peer.system = IsWellKnownSid(user.User.Sid, WinLocalSystemSid) != 0;
        }
        if let Some(buffer) = token_information(token, TokenGroups) {
            let groups = buffer.as_ptr().cast::<TOKEN_GROUPS>();
            // The groups run past the struct's one-element array, within the
            // buffer.
            let first = (&raw const (*groups).Groups).cast::<SID_AND_ATTRIBUTES>();
            let list = std::slice::from_raw_parts(first, (*groups).GroupCount as usize);
            peer.administrator = list.iter().any(|group| {
                group.Attributes & SE_GROUP_ENABLED as u32 != 0
                    && IsWellKnownSid(group.Sid, WinBuiltinAdministratorsSid) != 0
            });
        }
    }
    peer
}

/// A token's information of one class, in a buffer aligned for its structs.
///
/// # Safety
///
/// `token` must be an open token with `TOKEN_QUERY` access.
unsafe fn token_information(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Option<Vec<u64>> {
    let mut size = 0u32;
    // SAFETY: a size query, which fails with the size needed.
    unsafe { GetTokenInformation(token, class, null_mut(), 0, &mut size) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: the buffer holds `size` bytes.
    let read =
        unsafe { GetTokenInformation(token, class, buffer.as_mut_ptr().cast(), size, &mut size) };
    (read != 0).then_some(buffer)
}

/// A SID as text, such as `S-1-5-18`.
fn sid_text(sid: PSID) -> Option<String> {
    let mut text: *mut u16 = null_mut();
    // SAFETY: `sid` is valid; the string is freed below.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return None;
    }
    // SAFETY: the system returns a NUL-terminated string.
    let length = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    // SAFETY: `length` characters precede the NUL.
    let string = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    // SAFETY: the system allocated it with LocalAlloc.
    unsafe { LocalFree(text.cast()) };
    Some(string)
}

/// A handle, closed when dropped.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: an open handle that this module owns.
        unsafe { CloseHandle(self.0) };
    }
}
