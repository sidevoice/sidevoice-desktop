//! Credential Manager generic passwords (`windows-native-keyring-store`), as a keyring store whose entries persist
//! on this computer only and whose service search is the exact `<provider>/<service>` target namespace.
use keyring_core::{api::CredentialStoreApi, CredentialStore, Entry, Result};
use std::{any::Any, collections::HashMap, sync::Arc};

pub struct Native;

impl super::Os for Native {
    fn store() -> Result<Arc<CredentialStore>> {
        Ok(Arc::new(Local(credential_manager()?)))
    }
}

/// Targets are `<account>/<service>`. A service cannot contain the delimiter, so namespace matching is unambiguous
/// even when a future account contains it.
fn credential_manager() -> Result<Arc<windows_native_keyring_store::Store>> {
    windows_native_keyring_store::Store::new_with_configuration(&HashMap::from([
        ("divider", "/"),
        ("service_no_divider", "true"),
    ]))
}

/// The library store, adapted at its two Windows-specific points. Persistence is a per-entry modifier in
/// windows-native-keyring-store 1.1 (default `Enterprise`, which roams); `new_with_configuration` accepts only the
/// target delimiters. Its search takes a `pattern` regex instead of an exact `service`. Wrapping `build` and `search`
/// keeps both inside this file, and the common requests and default `clear_namespace` unchanged.
struct Local(Arc<windows_native_keyring_store::Store>);

impl CredentialStoreApi for Local {
    fn vendor(&self) -> String {
        self.0.vendor()
    }
    fn id(&self) -> String {
        self.0.id()
    }
    // keyring's set_password encodes UTF-16 and checks CRED_MAX_CREDENTIAL_BLOB_SIZE (2560
    // bytes) before CredWriteW. It rejects, never truncates. Provider ids (<=32 ASCII bytes)
    // and the fixed service also fit the target-name and username limits.
    // CRED_PERSIST_LOCAL_MACHINE: survives logoff on this computer, never roams with a profile.
    fn build(&self, service: &str, user: &str, _: Option<&HashMap<&str, &str>>) -> Result<Entry> {
        self.0.build(service, user, Some(&HashMap::from([("persistence", "Local")])))
    }
    fn search(&self, spec: &HashMap<&str, &str>) -> Result<Vec<Entry>> {
        let service =
            spec.get("service").ok_or_else(|| keyring_core::Error::Invalid("service".into(), "is required".into()))?;
        // Anchor both ends and escape the service so a similarly named service is never found. Accounts may be unknown.
        let pattern = format!(r"\A(?s:.*)/{}\z", regex::escape(service));
        self.0.search(&HashMap::from([("pattern", pattern.as_str())]))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Native fixtures stay here: they read Credential Manager's own persistence metadata and UTF-16 limits, which the
// common requests intentionally hide. Each also runs the common contract where it applies.
#[cfg(test)]
mod tests {
    use super::super::Os;
    use super::super::{clear_all, clear_in, read, read_in, tests::*, write, write_in, Error};
    use super::{credential_manager, Local, Native};
    use keyring_core::api::CredentialStoreApi;

    fn native() -> Local {
        Local(credential_manager().unwrap())
    }

    #[test]
    fn native_contract() {
        contract(&native(), &test_service(), &Native::clear_namespace);
    }

    #[test]
    fn legacy_keys_are_carried_over_in_credential_manager() {
        carry_over_native();
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

    #[test]
    fn windows_size_limit_rejects_without_replacing() {
        let _serial = serial();
        let (store, service, openai) = (&native(), &test_service(), provider("openai"));
        for (fits, too_big) in [("a".repeat(1280), "a".repeat(1281)), ("🔐".repeat(640), "🔐".repeat(641))] {
            write_in(store, service, &openai, Some(&fits)).unwrap();
            assert_eq!(write_in(store, service, &openai, Some(&too_big)), Err(Error::TooLong));
            assert_eq!(read_in(store, service, &openai).unwrap().as_deref(), Some(fits.as_str()));
        }
        clear_in(store, service).unwrap();
    }

    #[test]
    fn windows_credentials_persist_only_on_this_computer() {
        let _serial = serial();
        let (store, service, openai) = (&native(), &test_service(), provider("openai"));
        // Read metadata back from CredMan, not just from the entry's creation modifiers.
        write_in(store, service, &openai, Some("test-only")).unwrap();
        let raw = store.0.build(service, "openai", None).unwrap();
        assert_eq!(raw.get_attributes().unwrap()["persistence"], "Local");
        // Updating an entry written by the previous implementation also removes roaming.
        raw.set_password("old-enterprise-test-only").unwrap();
        assert_eq!(raw.get_attributes().unwrap()["persistence"], "Enterprise");
        write_in(store, service, &openai, Some("replacement")).unwrap();
        assert_eq!(raw.get_attributes().unwrap()["persistence"], "Local");
        clear_in(store, service).unwrap();
    }
}
