//! Keychain Services generic passwords over the whole keychain search list.
//!
//! Apple's keyring store constrains every lookup to a domain's default keychain. The old app's search-list scope is
//! kept through security-framework's maintained API, exposed as a keyring store (`SearchList`) so the common requests,
//! validation and error categories stay identical. No handwritten Security/CoreFoundation FFI.
use apple_native_keyring_store::keychain::decode_error;
use keyring_core::{
    api::{CredentialApi, CredentialStoreApi},
    Credential, CredentialStore, Entry, Result,
};
use std::{any::Any, collections::HashMap, sync::Arc};

/// errSecItemNotFound.
const NOT_FOUND: i32 = -25300;

pub struct Native;

impl super::Os for Native {
    /// Keychain access and unlock prompts wait for the person.
    const PROMPTS: bool = true;

    fn store() -> Result<Arc<CredentialStore>> {
        Ok(Arc::new(SearchList))
    }
}

struct SearchList;
struct Password {
    service: String,
    account: String,
}

impl CredentialStoreApi for SearchList {
    fn vendor(&self) -> String {
        "security-framework Keychain search list".into()
    }
    fn id(&self) -> String {
        "search-list".into()
    }
    fn build(&self, service: &str, user: &str, _: Option<&HashMap<&str, &str>>) -> Result<Entry> {
        Ok(Entry::new_with_credential(Arc::new(Password { service: service.into(), account: user.into() })))
    }
    /// The exact `{"service": ..}` namespace as one entry, `Namespace`: deleting it deletes every generic password of
    /// that service in the search list, whatever its account, without reading or decoding any account.
    fn search(&self, spec: &HashMap<&str, &str>) -> Result<Vec<Entry>> {
        let service =
            spec.get("service").ok_or_else(|| keyring_core::Error::Invalid("service".into(), "is required".into()))?;
        Ok(vec![Entry::new_with_credential(Arc::new(Namespace { service: (*service).into() }))])
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Every generic password of one service, across the search list. It holds no single secret.
struct Namespace {
    service: String,
}

impl CredentialApi for Namespace {
    fn get_secret(&self) -> Result<Vec<u8>> {
        Err(keyring_core::Error::Invalid("namespace".into(), "holds no single secret".into()))
    }
    fn set_secret(&self, _: &[u8]) -> Result<()> {
        Err(keyring_core::Error::Invalid("namespace".into(), "holds no single secret".into()))
    }
    /// One service-scoped SecItemDelete over the search list, as before the keyring adapter. It explicitly asks for
    /// all matches: the default limit on macOS deletes only one item. Any failure is reported, never partial success.
    fn delete_credential(&self) -> Result<()> {
        use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
        match ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(&self.service)
            .limit(Limit::All)
            .delete()
        {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(error) => Err(decode_error(error)),
        }
    }
    fn get_credential(&self) -> Result<Option<Arc<Credential>>> {
        Ok(None)
    }
    fn get_specifiers(&self) -> Option<(String, String)> {
        None
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl CredentialApi for Password {
    fn get_secret(&self) -> Result<Vec<u8>> {
        security_framework::passwords::get_generic_password(&self.service, &self.account).map_err(decode_error)
    }
    fn set_secret(&self, secret: &[u8]) -> Result<()> {
        // Update the matching item in place. Adding first can create a duplicate in the
        // default keychain when an existing item lives elsewhere in the search list.
        use security_framework::os::macos::passwords::find_generic_password;
        match find_generic_password(None, &self.service, &self.account) {
            Ok((_, mut item)) => item.set_password(secret).map_err(decode_error),
            Err(error) if error.code() == NOT_FOUND => {
                security_framework::passwords::set_generic_password(&self.service, &self.account, secret)
                    .map_err(decode_error)
            }
            Err(error) => Err(decode_error(error)),
        }
    }
    fn delete_credential(&self) -> Result<()> {
        security_framework::passwords::delete_generic_password(&self.service, &self.account).map_err(decode_error)
    }
    fn get_credential(&self) -> Result<Option<Arc<Credential>>> {
        self.get_secret().map(|_| None)
    }
    fn get_specifiers(&self) -> Option<(String, String)> {
        Some((self.service.clone(), self.account.clone()))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Native fixtures stay here: they exercise the Keychain's own legacy and search-list APIs, which the common requests
// intentionally hide. Each also runs the common contract where it applies.
#[cfg(test)]
mod tests {
    use super::super::{clear_all, clear_in, read, read_in, tests::*, write, write_in};
    use super::SearchList;

    #[test]
    fn native_contract() {
        contract(&SearchList, &test_service());
    }

    #[test]
    fn public_functions_round_trip_in_the_test_namespace() {
        // Test builds give the public functions a per-process namespace (`namespace`): no person's keys are touched.
        assert_eq!(read("openai"), Ok(None));
        write("openai", Some("test-only")).unwrap();
        assert_eq!(read("openai").unwrap().as_deref(), Some("test-only"));
        clear_all().unwrap();
        assert_eq!(read("openai"), Ok(None));
    }

    #[test]
    fn legacy_macos_passwords_remain_readable_in_both_directions() {
        use security_framework::passwords::{get_generic_password, set_generic_password};
        let _serial = serial();
        let (store, service) = (&SearchList, &test_service());
        set_generic_password(service, "openai", b"legacy-test-only").unwrap();
        assert_eq!(read_in(store, service, &provider("openai")).unwrap().as_deref(), Some("legacy-test-only"));
        write_in(store, service, &provider("openai"), Some("updated-test-only")).unwrap();
        assert_eq!(get_generic_password(service, "openai").unwrap(), b"updated-test-only");
        set_generic_password(service, "legacy_unknown", b"legacy-test-only").unwrap();
        clear_in(store, service).unwrap();
        assert_eq!(get_generic_password(service, "legacy_unknown").unwrap_err().code(), -25300);
    }

    #[test]
    fn macos_searches_nondefault_keychains_for_read_update_delete_and_clear() {
        use security_framework::os::macos::keychain::SecKeychain;
        use std::process::Command;
        fn security(args: &[&str]) -> std::process::Output {
            let output = Command::new("/usr/bin/security").args(args).output().unwrap();
            assert!(output.status.success(), "security test setup/cleanup failed");
            output
        }
        const CHILD: &str = "SIDEVOICE_CREDENTIAL_SEARCH_LIST_TEST";
        const KEYCHAIN_PATH: &str = "SIDEVOICE_CREDENTIAL_KEYCHAIN_PATH";
        if let Ok(service) = std::env::var(CHILD) {
            let (store, service) = (&SearchList, service.as_str());
            let secondary = SecKeychain::open(std::env::var(KEYCHAIN_PATH).unwrap()).unwrap();
            // Establish that the fixture is reachable by the old app's lookup API too.
            assert_eq!(
                security_framework::passwords::get_generic_password(service, "openai").unwrap(),
                b"legacy-secondary"
            );
            assert_eq!(read_in(store, service, &provider("openai")).unwrap().as_deref(), Some("legacy-secondary"));
            write_in(store, service, &provider("openai"), Some("updated-secondary")).unwrap();
            use security_framework::os::macos::passwords::find_generic_password;
            assert_eq!(
                find_generic_password(Some(std::slice::from_ref(&secondary)), service, "openai").unwrap().0.as_ref(),
                b"updated-secondary"
            );
            write_in(store, service, &provider("openai"), None).unwrap();
            assert_eq!(
                find_generic_password(Some(std::slice::from_ref(&secondary)), service, "openai").unwrap_err().code(),
                -25300
            );
            secondary.set_generic_password(service, "unknown_old", b"test-only").unwrap();
            // The same service/account in two searchable keychains: the default one and the secondary.
            secondary.set_generic_password(service, "duplicate", b"test-only").unwrap();
            security_framework::passwords::set_generic_password(service, "duplicate", b"test-only").unwrap();
            clear_in(store, service).unwrap();
            assert_eq!(
                find_generic_password(Some(std::slice::from_ref(&secondary)), service, "unknown_old")
                    .unwrap_err()
                    .code(),
                -25300
            );
            assert_eq!(
                find_generic_password(Some(std::slice::from_ref(&secondary)), service, "duplicate").unwrap_err().code(),
                -25300
            );
            assert_eq!(
                security_framework::passwords::get_generic_password(service, "duplicate").unwrap_err().code(),
                -25300
            );
            return;
        }
        // Preserve the runner/developer's complete search list, including on assertion failure.
        struct Keychain {
            path: String,
            previous: Vec<String>,
        }
        impl Drop for Keychain {
            fn drop(&mut self) {
                let _ = Command::new("/usr/bin/security")
                    .args(["list-keychains", "-d", "user", "-s"])
                    .args(&self.previous)
                    .output();
                let _ = Command::new("/usr/bin/security").args(["delete-keychain", &self.path]).output();
            }
        }
        let service = test_service();
        let output = security(&["list-keychains", "-d", "user"]);
        let previous: Vec<String> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| line.trim().trim_matches('"').to_owned())
            .collect();
        let keychain = Keychain {
            path: std::env::temp_dir().join(format!("{}.keychain-db", service)).to_str().unwrap().into(),
            previous,
        };
        security(&["create-keychain", "-p", "test-only", &keychain.path]);
        security(&["unlock-keychain", "-p", "test-only", &keychain.path]);
        let mut search = vec!["list-keychains", "-d", "user", "-s"];
        search.extend(keychain.previous.iter().map(String::as_str));
        search.push(&keychain.path);
        security(&search);
        let secondary = SecKeychain::open(&keychain.path).unwrap();
        secondary.set_generic_password(&service, "openai", b"legacy-secondary").unwrap();
        // Security.framework caches the search list per process. Other tests may already
        // have used it before the security CLI added this fixture; start a fresh process.
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("macos_searches_nondefault_keychains_for_read_update_delete_and_clear")
            .env(CHILD, &service)
            .env(KEYCHAIN_PATH, &keychain.path)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
