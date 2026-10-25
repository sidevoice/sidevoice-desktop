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
