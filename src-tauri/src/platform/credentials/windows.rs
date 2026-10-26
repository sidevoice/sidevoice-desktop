use keyring_core::{CredentialStore, Entry, Result};
use std::{collections::HashMap, sync::Arc};

/// Credential Manager generic passwords; operations serialized by the shared default admission.
pub(super) struct Store;

impl super::NativeStore for Store {
    fn store() -> Result<Arc<CredentialStore>> {
        // A service cannot contain the delimiter, so namespace matching is unambiguous even
        // when a future account contains it. Persistence is set explicitly when building entries below.
        Ok(windows_native_keyring_store::Store::new_with_configuration(&HashMap::from([
            ("divider", "/"),
            ("service_no_divider", "true"),
        ]))?)
    }

    // keyring's set_password encodes UTF-16 and checks CRED_MAX_CREDENTIAL_BLOB_SIZE (2560
    // bytes) before CredWriteW. It rejects, never truncates. Provider ids (<=32 ASCII bytes)
    // and the fixed service also fit the target-name and username limits.
    // CRED_PERSIST_LOCAL_MACHINE: survives logoff on this computer, never roams with a profile.
    fn entry(store: &CredentialStore, service: &str, provider: &str) -> Result<Entry> {
        if store.as_any().is::<windows_native_keyring_store::Store>() {
            store.build(service, provider, Some(&HashMap::from([("persistence", "Local")])))
        } else {
            // The portable contract uses keyring's in-memory sample store.
            store.build(service, provider, None)
        }
    }

    fn namespace(service: &str) -> (&'static str, String) {
        // Targets are <account>/<service>. Anchor both ends and escape the
        // service so a similarly named service cannot be deleted. Accounts may be unknown.
        ("pattern", format!(r"\A(?s:.*)/{}\z", regex::escape(service)))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{tests::*, Credentials, Error};

    #[test]
    fn native_contract() {
        contract(&native_test_store());
    }

    #[test]
    fn windows_size_limit_rejects_without_replacing() {
        let store = native_test_store();
        for (fits, too_big) in [("a".repeat(1280), "a".repeat(1281)), ("🔐".repeat(640), "🔐".repeat(641))] {
            store.write("openai", Some(&fits)).unwrap();
            assert_eq!(store.write("openai", Some(&too_big)), Err(Error::TooLong));
            assert_eq!(store.read("openai").unwrap().as_deref(), Some(fits.as_str()));
        }
        store.clear().unwrap();
    }

    #[test]
    fn windows_credentials_persist_only_on_this_computer() {
        let store = native_test_store();
        // Read metadata back from CredMan, not just from the entry's creation modifiers.
        store.write("openai", Some("test-only")).unwrap();
        let raw = store.store.build(&store.service, "openai", None).unwrap();
        assert_eq!(raw.get_attributes().unwrap()["persistence"], "Local");
        // Updating an entry written by the previous implementation also removes roaming.
        raw.set_password("old-enterprise-test-only").unwrap();
        assert_eq!(raw.get_attributes().unwrap()["persistence"], "Enterprise");
        store.write("openai", Some("replacement")).unwrap();
        assert_eq!(raw.get_attributes().unwrap()["persistence"], "Local");
        store.clear().unwrap();
    }
}
