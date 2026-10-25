use super::*;

pub struct Os;

impl WebviewData for Os {
    fn exited(pid: u32) -> io::Result<bool> {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        if pid <= 1 {
            return Err(Owner::busy());
        }
        // SAFETY: a synchronization-only handle; no process memory is read or modified.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) { Ok(true) } else { Err(error) };
        }
        // SAFETY: the handle is live, waited without blocking and closed exactly once on every path.
        let result = unsafe { WaitForSingleObject(handle, 0) };
        let error = io::Error::last_os_error();
        unsafe { CloseHandle(handle) };
        match result {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(error),
        }
    }

    fn observe(system: &System, locations: &Locations, processes: &mut Vec<(Pid, u64)>) -> io::Result<()> {
        // Capture starts with only the live host. Require positive UDF evidence at that point:
        // an environment/registry override may reuse another app's browser, outside our ancestry.
        // No matching runtime is uncertainty, not permission to delete a guessed default folder.
        let capturing = processes.len() == 1;
        let mut found_folder = false;
        Owner::descendants(system, processes);
        let current = system.process(Pid::from_u32(std::process::id())).ok_or_else(Owner::busy)?;
        let user = current.user_id().ok_or_else(Owner::busy)?;
        for (pid, process) in system.processes() {
            if !process.name().to_str().is_some_and(|name| name.eq_ignore_ascii_case("msedgewebview2.exe")) {
                continue;
            }
            // Unreadable ownership/arguments must not turn an unknown runtime into an absent one.
            let owner = process.user_id().ok_or_else(Owner::busy)?;
            if owner != user {
                continue;
            }
            if process.cmd().is_empty() || process.start_time() == 0 {
                return Err(Owner::busy());
            }
            let folder = Locations::browser_folder(process.cmd())?;
            if let Some(folder) = folder {
                let owned = locations
                    .paths()
                    .iter()
                    .any(|path| path == &folder || same_file::is_same_file(path, &folder).unwrap_or(false));
                found_folder |= owned;
                if owned && !processes.contains(&(*pid, process.start_time())) {
                    processes.push((*pid, process.start_time()));
                }
                // WebView2 supports environment/registry overrides. Refuse an app descendant
                // writing outside the native-resolved allowlist instead of deleting the wrong UDF.
                if !owned && processes.contains(&(*pid, process.start_time())) {
                    return Err(Owner::busy());
                }
            }
        }
        if capturing && !found_folder {
            return Err(Owner::busy());
        }
        Owner::descendants(system, processes);
        Ok(())
    }
}
