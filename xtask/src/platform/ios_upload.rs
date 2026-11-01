//! An iOS archive uploaded to App Store Connect (TestFlight). Apple's tool for it is part of Xcode.

use std::path::Path;

use crate::platform::host;
use crate::Result;

/// A host's way to upload an archive. A host without Xcode says so.
trait Os {
    /// `ipa` uploaded with the App Store Connect API key `key_id` of `issuer`. The key's private file is
    /// `private_keys/AuthKey_<key_id>.p8` below `keys_parent`, the directory the tool runs in.
    fn upload(_ipa: &Path, _key_id: &str, _issuer: &str, _keys_parent: &Path) -> Result<()> {
        Err(format!("an iOS archive is uploaded from a Mac with Xcode, not from {}", host::name()))
    }
}

#[cfg_attr(target_os = "macos", path = "ios_upload/macos.rs")]
#[cfg_attr(target_os = "windows", path = "ios_upload/windows.rs")]
#[cfg_attr(target_os = "linux", path = "ios_upload/linux.rs")]
mod imp;

use imp::Native;

/// Uploads `ipa` to App Store Connect, where TestFlight processes it.
pub fn upload(ipa: &Path, key_id: &str, issuer: &str, keys_parent: &Path) -> Result<()> {
    Native::upload(ipa, key_id, issuer, keys_parent)
}
