//! Windows: a running process's open files cannot be removed, so cleanup also waits for the parent to have
//! terminated, held by a synchronization-only process handle.

use super::Os;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

pub(super) struct Native;

impl Os for Native {
    const OPEN_FILES_BLOCK_REMOVAL: bool = true;

    type Held = OwnedHandle;

    fn hold(pid: u32) -> io::Result<OwnedHandle> {
        // SAFETY: a synchronization-only handle: nothing of the process is read or changed through it.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a live handle this function just opened and nothing else owns; closed once, on drop.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    fn terminated(held: &OwnedHandle) -> io::Result<bool> {
        // SAFETY: the handle is live for the borrow; a zero wait never blocks. A process object is signaled once the
        // process has terminated and every handle it had is closed.
        match unsafe { WaitForSingleObject(held.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(io::Error::last_os_error()),
        }
    }
}
