use keyring_core::{CredentialStore, Result};
use std::{collections::HashMap, sync::Arc};

pub(super) fn store() -> Result<Arc<CredentialStore>> {
    // A service cannot contain the delimiter, so namespace matching is unambiguous even
    // when a future account contains it. Persistence is set explicitly when building entries below.
    Ok(windows_native_keyring_store::Store::new_with_configuration(&HashMap::from([
        ("divider", "/"),
        ("service_no_divider", "true"),
    ]))?)
}

// The production reset caller remains macOS-only for now.
#[allow(dead_code)]
pub(super) fn clear(store: &CredentialStore, service: &str) -> Result<()> {
    // Targets are <account>/<service>. Anchor both ends and escape the
    // service so a similarly named service cannot be deleted. Accounts may be unknown.
    let pattern = format!(r"\A(?s:.*)/{}\z", regex::escape(service));
    super::delete_entries(store.search(&HashMap::from([("pattern", pattern.as_str())]))?)
}
// keyring's set_password encodes UTF-16 and checks CRED_MAX_CREDENTIAL_BLOB_SIZE (2560
// bytes) before CredWriteW. It rejects, never truncates. Provider ids (<=32 ASCII bytes)
// and the fixed service also fit the target-name and username limits.

// CRED_PERSIST_LOCAL_MACHINE: survives logoff on this computer, never roams with a profile.
pub(super) fn entry(store: &CredentialStore, service: &str, provider: &str) -> Result<keyring_core::Entry> {
    store.build(service, provider, Some(&HashMap::from([("persistence", "Local")])))
}
