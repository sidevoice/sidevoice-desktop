//! Keychain Services generic passwords in the user's login keychain, through apple-native-keyring-store as it is.
use apple_native_keyring_store::keychain::{decode_error, Store};
use keyring_core::{CredentialStore, Result};
use std::sync::Arc;

/// errSecItemNotFound.
const NOT_FOUND: i32 = -25300;

pub struct Native;

impl super::Os for Native {
    /// Keychain access and unlock prompts wait for the person.
    const PROMPTS: bool = true;

    fn store() -> Result<Arc<CredentialStore>> {
        Ok(Store::new()?)
    }

    /// One service-scoped SecItemDelete in the store's keychain: every generic password of exactly `service`, whatever
    /// its account. The store's own search would rebuild each account from its attribute text and delete by that name,
    /// so an account that does not round-trip through text would be missed while its `NoEntry` reads as success; this
    /// deletes by service alone and reads no account. SecItemDelete deletes all matches by default; `Limit::All`
    /// states that intent explicitly. Any failure is reported, never partial success.
    fn clear_namespace(service: &str) -> std::result::Result<(), super::Error> {
        use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
        use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};
        let login = SecKeychain::default_for_domain(SecPreferencesDomain::User).map_err(decode_error)?;
        match ItemSearchOptions::new()
            .keychains(&[login])
            .class(ItemClass::generic_password())
            .service(service)
            .limit(Limit::All)
            .delete()
        {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(error) => Err(decode_error(error).into()),
        }
    }
}

// The common contract and the public functions, on the real Keychain in test namespaces.
#[cfg(test)]
mod tests {
    use super::super::{clear_all, read, tests::*, write, Os};
    use super::Native;

    #[test]
    fn native_contract() {
        contract(Native::store().unwrap().as_ref(), &test_service(), &Native::clear_namespace);
    }

    #[test]
    fn legacy_keys_are_copied_in_the_real_keychain() {
        copy_from_native();
    }

    #[test]
    fn public_functions_round_trip_in_the_test_namespace() {
        // Serialised with the other tests that use the per-process namespace.
        let _serial = serial();
        // Test builds give the public functions a per-process namespace (`namespace`): no person's keys are touched.
        assert_eq!(read("openai"), Ok(None));
        write("openai", Some("test-only")).unwrap();
        assert_eq!(read("openai").unwrap().as_deref(), Some("test-only"));
        clear_all().unwrap();
        assert_eq!(read("openai"), Ok(None));
    }
}
