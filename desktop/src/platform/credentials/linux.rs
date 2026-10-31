//! The user's Secret Service over an encrypted D-Bus session (`zbus-secret-service-keyring-store`), searched by the
//! exact `service` attribute across collections. A locked collection can hold a call on an unlock prompt.
use keyring_core::{CredentialStore, Result};
use std::sync::Arc;

pub struct Native;

impl super::Os for Native {
    const PROMPTS: bool = true;

    fn store() -> Result<Arc<CredentialStore>> {
        Ok(zbus_secret_service_keyring_store::Store::new()?)
    }
}

// Native fixtures stay here: they need a live Secret Service, or a session bus environment pointed elsewhere, which
// the common requests cannot express.
#[cfg(test)]
mod tests {
    use super::super::{clear_all, copy_key_from, providers_in, read, tests::*, write, Error, Os};
    use super::Native;

    #[test]
    #[ignore = "requires an unlocked desktop Secret Service; unverified on headless CI"]
    fn native_contract() {
        contract(Native::store().unwrap().as_ref(), &test_service(), &Native::clear_namespace);
    }

    #[test]
    #[ignore = "requires an unlocked desktop Secret Service; unverified on headless CI"]
    fn legacy_keys_are_copied() {
        copy_native();
    }

    #[test]
    fn missing_secret_service_is_explicitly_unavailable() {
        const CHILD: &str = "SIDEVOICE_CREDENTIAL_UNAVAILABLE_TEST";
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(read("openai"), Err(Error::Unavailable));
            assert_eq!(write("openai", Some("test-only")), Err(Error::Unavailable));
            assert_eq!(clear_all(), Err(Error::Unavailable));
            assert_eq!(providers_in("dev.sidevoice.desktop"), Err(Error::Unavailable));
            assert_eq!(copy_key_from("dev.sidevoice.desktop", "openai", || true), Err(Error::Unavailable));
            return;
        }
        // Isolate the bus environment from concurrent tests and any real user's keyring.
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("missing_secret_service_is_explicitly_unavailable")
            .env(CHILD, "1")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-sidevoice-test-bus")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
