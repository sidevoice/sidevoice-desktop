use super::*;

pub struct Os;

impl WebviewData for Os {
    fn observe(system: &System, locations: &Locations, processes: &mut Vec<(Pid, u64)>) -> io::Result<()> {
        descendants(system, processes);
        let current = system.process(Pid::from_u32(std::process::id())).ok_or_else(busy)?;
        let user = current.user_id().ok_or_else(busy)?;
        for (pid, process) in system.processes() {
            if !process.name().to_str().is_some_and(|name| name.eq_ignore_ascii_case("msedgewebview2.exe")) {
                continue;
            }
            // Unreadable ownership/arguments must not turn an unknown runtime into an absent one.
            let owner = process.user_id().ok_or_else(busy)?;
            if owner != user {
                continue;
            }
            if process.cmd().is_empty() || process.start_time() == 0 {
                return Err(busy());
            }
            let folder = user_data_folder(process.cmd())?;
            if let Some(folder) = folder {
                let owned = locations
                    .paths()
                    .iter()
                    .any(|path| path == &folder || same_file::is_same_file(path, &folder).unwrap_or(false));
                if owned && !processes.contains(&(*pid, process.start_time())) {
                    processes.push((*pid, process.start_time()));
                }
                // WebView2 supports environment/registry overrides. Refuse an app descendant
                // writing outside the native-resolved allowlist instead of deleting the wrong UDF.
                if !owned && processes.contains(&(*pid, process.start_time())) {
                    return Err(busy());
                }
            }
        }
        descendants(system, processes);
        Ok(())
    }
}
