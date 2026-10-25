use keyring_core::{CredentialStore, Result};
use std::sync::Arc;

// Apple's keyring store constrains every lookup to a domain's default keychain.
// Preserve the old app's search-list scope through the maintained security-framework
// API, exposed as keyring entries so shared validation/error handling stays identical.
use apple_native_keyring_store::keychain::decode_error;
use keyring_core::{
    api::{CredentialApi, CredentialStoreApi},
    Credential, Entry,
};
use std::{any::Any, collections::HashMap};

struct SearchList;
struct Password {
    service: String,
    account: String,
}

pub(super) fn store() -> Result<Arc<CredentialStore>> {
    Ok(Arc::new(SearchList))
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
            Err(error) if error.code() == -25300 => {
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

pub(super) fn clear(_store: &CredentialStore, service: &str) -> Result<()> {
    // Preserve the old SecItemDelete search-list scope, including credentials outside the
    // default login keychain. The keyring store's search is limited to its chosen keychain.
    // security-framework owns the bindings; no handwritten Security/CoreFoundation FFI.
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    // Explicitly request all matches: the default limit on macOS deletes only one item.
    match ItemSearchOptions::new().class(ItemClass::generic_password()).service(service).limit(Limit::All).delete() {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(error) => Err(apple_native_keyring_store::keychain::decode_error(error)),
    }
}

/// Native provider credential store; failures never install a fallback or prevent a later retry.
pub struct ProviderCredentials;

impl super::Credentials for ProviderCredentials {}

impl super::Backend for ProviderCredentials {
    fn execute(&self, request: super::Request) -> std::result::Result<Option<String>, super::Error> {
        execute(move || request.run(store()?.as_ref(), super::SERVICE, entry, clear))
    }
}

// Serialize operations on native entries; no secret or backend error is logged.
static ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn execute<T>(
    operation: impl FnOnce() -> std::result::Result<T, super::Error>,
) -> std::result::Result<T, super::Error> {
    let _guard = ACCESS.lock().map_err(|_| super::Error::Failed)?;
    operation()
}

pub(super) fn entry(store: &CredentialStore, service: &str, provider: &str) -> Result<keyring_core::Entry> {
    store.build(service, provider, None)
}

#[cfg(test)]
mod tests {
    use super::super::{tests::*, Credentials};

    #[test]
    fn native_contract() {
        contract(&native_test_store());
    }

    #[test]
    fn legacy_macos_passwords_remain_readable_in_both_directions() {
        use security_framework::passwords::{get_generic_password, set_generic_password};
        let store = native_test_store();
        set_generic_password(&store.service, "openai", b"legacy-test-only").unwrap();
        assert_eq!(store.read("openai").unwrap().as_deref(), Some("legacy-test-only"));
        store.write("openai", Some("updated-test-only")).unwrap();
        assert_eq!(get_generic_password(&store.service, "openai").unwrap(), b"updated-test-only");
        set_generic_password(&store.service, "legacy_unknown", b"legacy-test-only").unwrap();
        store.clear().unwrap();
        assert_eq!(get_generic_password(&store.service, "legacy_unknown").unwrap_err().code(), -25300);
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
            let store = TestCredentials { store: super::store().unwrap(), service, clear: super::clear };
            let secondary = SecKeychain::open(std::env::var(KEYCHAIN_PATH).unwrap()).unwrap();
            // Establish that the fixture is reachable by the old app's lookup API too.
            assert_eq!(
                security_framework::passwords::get_generic_password(&store.service, "openai").unwrap(),
                b"legacy-secondary"
            );
            assert_eq!(store.read("openai").unwrap().as_deref(), Some("legacy-secondary"));
            store.write("openai", Some("updated-secondary")).unwrap();
            use security_framework::os::macos::passwords::find_generic_password;
            assert_eq!(
                find_generic_password(Some(std::slice::from_ref(&secondary)), &store.service, "openai")
                    .unwrap()
                    .0
                    .as_ref(),
                b"updated-secondary"
            );
            store.write("openai", None).unwrap();
            assert_eq!(
                find_generic_password(Some(std::slice::from_ref(&secondary)), &store.service, "openai")
                    .unwrap_err()
                    .code(),
                -25300
            );
            secondary.set_generic_password(&store.service, "unknown_old", b"test-only").unwrap();
            store.clear().unwrap();
            assert_eq!(
                find_generic_password(Some(&[secondary]), &store.service, "unknown_old").unwrap_err().code(),
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
        let store = native_test_store();
        let output = security(&["list-keychains", "-d", "user"]);
        let previous: Vec<String> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| line.trim().trim_matches('"').to_owned())
            .collect();
        let keychain = Keychain {
            path: std::env::temp_dir().join(format!("{}.keychain-db", store.service)).to_str().unwrap().into(),
            previous,
        };
        security(&["create-keychain", "-p", "test-only", &keychain.path]);
        security(&["unlock-keychain", "-p", "test-only", &keychain.path]);
        let mut search = vec!["list-keychains", "-d", "user", "-s"];
        search.extend(keychain.previous.iter().map(String::as_str));
        search.push(&keychain.path);
        security(&search);
        let secondary = SecKeychain::open(&keychain.path).unwrap();
        secondary.set_generic_password(&store.service, "openai", b"legacy-secondary").unwrap();
        // Security.framework caches the search list per process. Other tests may already
        // have used it before the security CLI added this fixture; start a fresh process.
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("macos_searches_nondefault_keychains_for_read_update_delete_and_clear")
            .env(CHILD, &store.service)
            .env(KEYCHAIN_PATH, &keychain.path)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
