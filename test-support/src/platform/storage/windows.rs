use super::*;

pub struct NativeStorageFixtures;
impl StorageFixture for NativeStorageFixtures {
    fn assert_not_executable(_path: &Path) {} // Execution is selected by the loader/extension, not a permission bit.
    fn private_directory_violations(path: &Path, mut check: impl FnMut()) {
        Self::readable(path);
        check();
        Self::writable(path);
        check();
    }
    fn record_access_cases(path: &Path, mut check: impl FnMut(bool)) {
        Self::private(path);
        check(true);
        Self::readable(path);
        check(true);
        Self::writable(path);
        check(false);
    }
    fn special_record(path: &Path) {
        fs::create_dir(path).unwrap();
    }
    fn replace_directory(path: &Path, moved: &Path) -> Replacement {
        assert_eq!(fs::rename(path, moved).unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        Replacement::Prevented
    }
    fn with_search_only_ancestor(_path: &Path, mut check: impl FnMut()) {
        check();
    }
    fn system_program() -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").expect("Windows directory")).join("System32/cmd.exe")
    }
    fn protect_shared_ancestor(_path: &Path) -> bool {
        false
    } // No sticky-directory counterpart.
}
