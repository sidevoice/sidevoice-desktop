use std::path::PathBuf;

use objc2_foundation::{NSFileManager, NSString};

use super::Os;

/// The App Group the app shares with its Live Activity's widget extension (both targets carry it in their entitlements).
const APP_GROUP: &str = "group.ai.sidevoice.mobile";

/// The App Group's container (`containerURLForSecurityApplicationGroupIdentifier:`): none while the app is not entitled to
/// the group.
pub struct Native;

impl Os for Native {
    fn shared_container() -> Option<PathBuf> {
        let manager = NSFileManager::defaultManager();
        let url = manager
            .containerURLForSecurityApplicationGroupIdentifier(&NSString::from_str(APP_GROUP))?;
        url.to_file_path()
    }
}
