//! Sockets that launchd made for the job from the `Sockets` entry of its
//! launchd.plist(5), handed over by launch_activate_socket(3). This module is
//! the agent's only unsafe code on Unix (D20): that call into libSystem, and
//! the free(3) its result needs. Windows' is in `os::windows` (D58).

#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_int, c_void};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixListener;

unsafe extern "C" {
    fn launch_activate_socket(
        name: *const c_char,
        fds: *mut *mut c_int,
        count: *mut usize,
    ) -> c_int;
    fn free(pointer: *mut c_void);
}

/// The listening socket launchd made for the job's `name` entry; `None` when
/// launchd didn't start this process or the job has no such entry.
pub fn listener(name: &str) -> Option<UnixListener> {
    let name = CString::new(name).ok()?;
    let mut fds: *mut c_int = std::ptr::null_mut();
    let mut count: usize = 0;
    // SAFETY: `name` is a valid C string, and `fds` and `count` point to
    // locals that the call fills in when it succeeds.
    let result = unsafe { launch_activate_socket(name.as_ptr(), &mut fds, &mut count) };
    if result != 0 || fds.is_null() {
        return None;
    }
    // SAFETY: on success `fds` points to `count` descriptors that this process
    // now owns, in an array from malloc(3) that the caller frees.
    let owned: Vec<OwnedFd> = (0..count)
        .map(|index| unsafe { OwnedFd::from_raw_fd(*fds.add(index)) })
        .collect();
    // SAFETY: `fds` came from launch_activate_socket and is freed once.
    unsafe { free(fds.cast()) };
    // A socket path gives one descriptor; any others close as they drop.
    owned.into_iter().next().map(UnixListener::from)
}
