use super::*;
use sidevoice_platform::ipc::{Address, Endpoint};

pub struct Os;
impl EndpointCleanup for Os {
    fn forget(data: &Path) {
        for endpoint in [Endpoint::new(&data.join("core"), "local"), Endpoint::new(data, "connector")] {
            let path = std::path::PathBuf::from(endpoint.to_string());
            if let Some(parent) = path.parent().filter(|p| !p.starts_with(data)) {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }
}
