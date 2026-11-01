//! The app's Data Protection keychain (`apple-native-keyring-store`, its `protected` store, the only keychain iOS has):
//! generic passwords in the app's own access group, readable from the first unlock after a restart on, so a call that
//! runs under a locked screen can still open its provider connection. Entries never leave this device (no iCloud
//! synchronization, no migration to another phone through a backup).
use apple_native_keyring_store::protected::Store;
use keyring_core::{api::CredentialStoreApi, CredentialStore, Entry, Result};
use std::{any::Any, collections::HashMap, sync::Arc};

pub struct Native;

impl super::Os for Native {
    fn store() -> Result<Arc<CredentialStore>> {
        Ok(Arc::new(AfterFirstUnlock(Store::new()?)))
    }
}

/// The library store, adapted at its one per-entry option. The access policy is a modifier of `build` (default: only
/// while the device is unlocked), and the common requests build every entry without one. Its service search is already
/// exact, so the default namespace clear applies.
struct AfterFirstUnlock(Arc<Store>);

impl CredentialStoreApi for AfterFirstUnlock {
    fn vendor(&self) -> String {
        self.0.vendor()
    }
    fn id(&self) -> String {
        self.0.id()
    }
    fn build(&self, service: &str, user: &str, _: Option<&HashMap<&str, &str>>) -> Result<Entry> {
        self.0.build(service, user, Some(&HashMap::from([("access-policy", "after-first-unlock-this-device-only")])))
    }
    fn search(&self, spec: &HashMap<&str, &str>) -> Result<Vec<Entry>> {
        self.0.search(spec)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// The common contract on the real keychain, in a test namespace. It needs the signed app's keychain access group, so it
// runs in the app (a device or simulator build), not as a bare test binary.
#[cfg(test)]
mod tests {
    use super::super::{tests::*, Os};
    use super::Native;

    #[test]
    #[ignore = "requires the signed app's keychain access group: run it inside the app"]
    fn native_contract() {
        contract(Native::store().unwrap().as_ref(), &test_service(), &Native::clear_namespace);
    }
}
