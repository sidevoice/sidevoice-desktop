//! Windows: the profile folder the system records for the process's user (`GetUserProfileDirectoryW` on its token),
//! never `USERPROFILE`; and what this account may change in a directory, asked of the system by opening it.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ADD_SUBDIRECTORY, FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::GetUserProfileDirectoryW;

pub(super) struct Native;

impl super::Os for Native {
    fn recorded_home() -> Option<PathBuf> {
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

    /// Asks the system by opening with each right (the access check Windows itself makes, privileges and groups of
    /// this token included): `WRITE_DAC` or `WRITE_OWNER` on `dir` lets this account grant itself anything there;
    /// otherwise it needs to add a directory to `dir` (`FILE_ADD_SUBDIRECTORY`, for the replacement) and to remove
    /// `child` (`DELETE` on it, `FILE_DELETE_CHILD` on `dir`, or `WRITE_DAC`/`WRITE_OWNER` on `child` to grant itself
    /// `DELETE`). Opens neither follow a link (`FILE_FLAG_OPEN_REPARSE_POINT`) nor take a share lock.
    fn may_replace(dir: &Path, child: &Path) -> io::Result<bool> {
        Ok(granted(dir, WRITE_DAC)?
            || granted(dir, WRITE_OWNER)?
            || (granted(dir, FILE_ADD_SUBDIRECTORY)?
                && (granted(child, DELETE)?
                    || granted(dir, FILE_DELETE_CHILD)?
                    || granted(child, WRITE_DAC)?
                    || granted(child, WRITE_OWNER)?)))
    }
}

/// Whether `path` opens with `right`: `false` when access is denied.
fn granted(path: &Path, right: u32) -> io::Result<bool> {
    let opened = std::fs::OpenOptions::new()
        .access_mode(right)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path);
    match opened {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(false),
        Err(error) => Err(error),
    }
}
