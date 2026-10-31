//! Android: `android-native-keyring-store`, one SharedPreferences file in the app's private storage whose values are
//! AES-GCM encrypted by a key that never leaves the Android Keystore (no user authentication required, so a call can
//! read its keys under a locked screen). The Keystore key does not leave the device and is not backed up.
//!
//! The store reaches Android through JNI on the application context `ndk-context` holds. Tauri initialises it before
//! the app runs, so no Kotlin is needed here.
use android_native_keyring_store::Store;
use keyring_core::{api::CredentialStoreApi, CredentialStore, Entry, Error, Result};
use std::{any::Any, collections::HashMap, sync::Arc};

/// The store's name: its file is `keyring-<name>` and its Keystore alias the same, apart from any other keyring user.
const NAME: &str = "sidevoice-providers";

pub struct Native;

impl super::Os for Native {
    /// The library opens the vault of this name once per process and hands it to every later store.
    fn store() -> Result<Arc<CredentialStore>> {
        Ok(Arc::new(Exact(Store::new_with_configuration(&HashMap::from([("name", NAME)]))?)))
    }
}

/// The library store, with its service search made exact as `Os::store` requires: it takes a regular expression that
/// matches anywhere in the service, so the service is escaped and anchored at both ends, and a similarly named
/// service is never found. Accounts may be unknown.
struct Exact(Arc<Store>);

impl CredentialStoreApi for Exact {
    fn vendor(&self) -> String {
        self.0.vendor()
    }
    fn id(&self) -> String {
        self.0.id()
    }
    fn build(&self, service: &str, user: &str, modifiers: Option<&HashMap<&str, &str>>) -> Result<Entry> {
        self.0.build(service, user, modifiers)
    }
    fn search(&self, spec: &HashMap<&str, &str>) -> Result<Vec<Entry>> {
        let service = spec.get("service").ok_or_else(|| Error::Invalid("service".into(), "is required".into()))?;
        let pattern = format!(r"\A{}\z", regex::escape(service));
        self.0.search(&HashMap::from([("service", pattern.as_str())]))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// The common contract on the real Keystore, in a test namespace. It needs the application context of a running app,
// so it runs in the app (a device or emulator build), not as a bare test binary.
#[cfg(test)]
mod tests {
    use super::super::{tests::*, Os};
    use super::Native;

    #[test]
    #[ignore = "requires the application context of a running app: run it inside the app"]
    fn native_contract() {
        contract(Native::store().unwrap().as_ref(), &test_service(), &Native::clear_namespace);
    }
}
