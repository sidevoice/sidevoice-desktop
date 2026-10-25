use super::{Mount, OwnedRemoval};
use std::{fs::File, os::fd::AsRawFd, path::Path};

pub struct NativeOwnedRemoval;

impl OwnedRemoval for NativeOwnedRemoval {
    fn mount(file: &File, _: &Path) -> Result<Mount, &'static str> {
        // The kernel mount ID distinguishes same-device bind mounts, like the pinned removal implementation.
        let info =
            std::fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd())).map_err(|_| "inspect-mount")?;
        info.lines()
            .find_map(|line| line.strip_prefix("mnt_id:\t"))
            .map(|id| Mount(id.as_bytes().to_vec()))
            .ok_or("inspect-mount")
    }
}
