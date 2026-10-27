use super::Os;
use tauri::utils::config::BundleType;
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

pub(super) struct Native;

impl Os for Native {
    fn host_name() -> Option<String> {
        whoami::hostname().ok()
    }

    const BUNDLE: Option<BundleType> = Some(BundleType::Nsis);

    /// The registered per-user NSIS installation's own executable.
    fn installed_here() -> bool {
        registration().is_some_and(|(location, binary)| {
            std::env::current_exe().is_ok_and(|exe| installed_executable(&exe, &location, &binary))
        })
    }
}

fn registration() -> Option<(String, String)> {
    // Tauri's NSIS template keys the uninstall entry by PRODUCTNAME, not BUNDLEID.
    let config: serde_json::Value = serde_json::from_str(include_str!("../../../tauri.conf.json")).ok()?;
    let product = config.get("productName")?.as_str()?;
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{product}"))
        .ok()?;
    Some((key.get_value("InstallLocation").ok()?, key.get_value("MainBinaryName").ok()?))
}

/// Bind a running executable to the actual NSIS record. NSIS quotes InstallLocation and records
/// MainBinaryName separately. Comparing both directory and file identity rejects copies/hard links
/// outside the installation and handles native path aliases/casing without hand-written normalization.
fn installed_executable(exe: &std::path::Path, location: &str, binary: &str) -> bool {
    let location = location.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(location);
    let directory = std::path::Path::new(location);
    let mut components = std::path::Path::new(binary).components();
    if !directory.is_absolute()
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return false;
    }
    exe.parent().is_some_and(|parent| same_file::is_same_file(parent, directory).unwrap_or(false))
        && same_file::is_same_file(exe, directory.join(binary)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_registered_executable_is_eligible_even_when_a_copy_has_identical_bytes() {
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("Installed Sidevoice");
        let copied = root.path().join("Copied Sidevoice");
        std::fs::create_dir(&installed).unwrap();
        std::fs::create_dir(&copied).unwrap();
        let binary = "sidevoice-desktop.exe";
        let original = installed.join(binary);
        let copy = copied.join(binary);
        std::fs::write(&original, b"an NSIS-marked executable").unwrap();
        std::fs::copy(&original, &copy).unwrap();
        let location = format!("\"{}\"", installed.display());
        assert!(installed_executable(&original, &location, binary));
        assert!(!installed_executable(&copy, &location, binary));
        assert!(!installed_executable(&original, "", binary));
        assert!(!installed_executable(&original, "relative", binary));
        assert!(!installed_executable(&original, &location, "missing.exe"));
        assert!(!installed_executable(&original, &location, "../sidevoice-desktop.exe"));
    }
}
