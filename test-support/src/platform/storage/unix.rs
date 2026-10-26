use super::*;
use std::os::unix::fs::PermissionsExt;

pub struct NativeStorageFixtures;
fn mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
impl StorageFixture for NativeStorageFixtures {
    fn assert_private(path: &Path) {
        Files::check_private(&Files::open_read(path).unwrap()).unwrap();
        let metadata = fs::metadata(path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, if metadata.is_dir() { 0o700 } else { 0o600 });
    }
    fn assert_not_executable(path: &Path) {
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o111, 0);
    }
    fn private_directory_violations(path: &Path, mut check: impl FnMut()) {
        for bits in [0o750, 0o705, 0o770, 0o777, 0o710] {
            mode(path, bits);
            check();
        }
    }
    fn record_access_cases(path: &Path, mut check: impl FnMut(bool)) {
        for bits in [0o600, 0o644, 0o640, 0o700, 0o755] {
            mode(path, bits);
            check(true);
        }
        for bits in [0o602, 0o666, 0o777] {
            mode(path, bits);
            check(false);
        }
    }
    fn special_record(path: &Path) {
        // The POSIX utility covers both supported Unix hosts without introducing own FFI.
        assert!(std::process::Command::new("mkfifo").args(["-m", "600"]).arg(path).status().unwrap().success());
    }
    fn replace_directory(path: &Path, moved: &Path) -> Replacement {
        fs::rename(path, moved).unwrap();
        Replacement::Replaced
    }
    fn with_search_only_ancestor(path: &Path, mut check: impl FnMut()) {
        struct Restore<'a>(&'a Path, fs::Permissions);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                fs::set_permissions(self.0, self.1.clone()).unwrap();
            }
        }
        let _restore = Restore(path, fs::metadata(path).unwrap().permissions());
        // These are distinct native guarantees: 0311 denies listing even to the owner.
        for bits in [0o311, 0o711] {
            mode(path, bits);
            check();
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, bits);
        }
    }
    fn system_program() -> PathBuf {
        PathBuf::from("/bin/sh")
    }
}
