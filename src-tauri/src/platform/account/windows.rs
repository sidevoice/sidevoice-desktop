//! Windows: the profile folder the system records for the process's user (`GetUserProfileDirectoryW` on its token),
//! never `USERPROFILE`.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::GetUserProfileDirectoryW;

pub(super) fn recorded_home() -> Option<PathBuf> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the current-process pseudo handle needs no closing; `token` receives a handle closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let mut size = 0u32;
    // SAFETY: a null buffer asks for the size, written to `size`.
    unsafe { GetUserProfileDirectoryW(token, std::ptr::null_mut(), &mut size) };
    let mut buffer = vec![0u16; size as usize];
    // SAFETY: `buffer` holds `size` characters.
    let found = size > 0 && unsafe { GetUserProfileDirectoryW(token, buffer.as_mut_ptr(), &mut size) } != 0;
    // SAFETY: `token` was opened above and is closed once.
    unsafe { CloseHandle(token) };
    if !found {
        return None;
    }
    let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    Some(PathBuf::from(OsString::from_wide(&buffer[..end])))
}
