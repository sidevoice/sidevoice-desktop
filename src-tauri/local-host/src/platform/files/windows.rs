//! Windows: a program runs by its name (`.exe`), so there is nothing to mark; who else may run it is the file's DACL,
//! made private when the file is created (`sidevoice_platform::fs`, as the install unpacks it) and only checked here:
//! one that is not private is refused, never repaired. Which file a path names is its volume's serial number and its
//! file index, read with its last write from one handle on the name itself (a link is not followed:
//! `FILE_FLAG_OPEN_REPARSE_POINT`), through `winapi-util`, with no `unsafe` of ours; std's own file index is not stable
//! yet.

use super::Os;
use sidevoice_platform::fs::{Files, Fs};
use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::time::SystemTime;
use windows::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};

pub struct Native;

impl Os for Native {
    /// Running is not a permission on the file: it is this user's alone when its DACL is private, a regular file with
    /// one name, opened without following a link.
    fn make_runnable(path: &Path) -> io::Result<()> {
        Files::check_private(&Files::open_read(path)?)
    }

    fn identity(path: &Path) -> Option<((u64, u128), SystemTime)> {
        // No access asked but the attributes: the writer is never kept from replacing it; a directory opens too.
        let file = OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(path)
            .ok()?;
        let information = winapi_util::file::information(&file).ok()?;
        let modified = file.metadata().ok()?.modified().ok()?;
        Some(((information.volume_serial_number(), information.file_index().into()), modified))
    }
}
