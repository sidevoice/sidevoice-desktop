//! Windows: a program runs by its name (`.exe`), so there is nothing to mark. Which file a path names is its volume's
//! serial number and its file index, read with its last write from one handle on the name itself (a link is not
//! followed: `FILE_FLAG_OPEN_REPARSE_POINT`), through `winapi-util`, with no `unsafe` of ours; std's own file index is
//! not stable yet.

use super::Os;
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::time::SystemTime;
use windows::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};

pub struct Native;

impl Os for Native {
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
