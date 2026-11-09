//! What the install needs to know of this OS's temporary directory to choose where it stages its download
//! ([`crate::install::staging_root`] decides). One value differs and no behaviour does, so this is a plain module with no
//! trait and no OS files, as `sidevoice_platform::data_dir` is.

/// Windows: `%TEMP%` (`TEMP`/`TMP`) is commonly moved out of the profile by policy, a tool or by hand, to a folder below
/// a drive root (`C:\Temp`, `D:\Temp`, a RAM disk, a junction) that inherits the root's "Authenticated Users: Modify",
/// which the shared private-directory policy refuses. macOS gives each user a private `$TMPDIR`, and Linux's `/tmp` is
/// sticky, which the policy accepts.
const TEMP_OFTEN_REDIRECTED: bool = cfg!(windows);

/// Whether this OS's temporary directory is commonly redirected to a place other users can change.
pub fn temp_often_redirected() -> bool {
    TEMP_OFTEN_REDIRECTED
}
