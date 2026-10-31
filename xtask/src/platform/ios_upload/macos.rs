//! macOS: `xcrun altool`, Xcode's uploader. It reads its API key from `private_keys/` in the directory it runs in.

use std::path::Path;

use super::Os;
use crate::util::Cmd;
use crate::Result;

pub struct Native;

impl Os for Native {
    fn upload(ipa: &Path, key_id: &str, issuer: &str, keys_parent: &Path) -> Result<()> {
        Cmd::new("xcrun")
            .args(["altool", "--upload-app", "--type", "ios", "--file"])
            .arg(ipa)
            .arg("--apiKey")
            .secret(key_id)
            .arg("--apiIssuer")
            .secret(issuer)
            .dir(keys_parent)
            .run()
    }
}
