use keyring_core::{CredentialStore, Result};
use std::sync::Arc;

pub(super) fn store() -> Result<Arc<CredentialStore>> {
    // Legacy Keychain Services, not the separate protected-data store. Same service and
    // account, with UTF-8 secrets, as security_framework::passwords used before.
    Ok(apple_native_keyring_store::keychain::Store::new()?)
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
