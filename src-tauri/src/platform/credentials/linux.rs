use keyring_core::{CredentialStore, Result};
use std::{collections::HashMap, sync::Arc};

pub(super) fn store() -> Result<Arc<CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

// The production reset caller remains macOS-only for now.
#[allow(dead_code)]
pub(super) fn clear(store: &CredentialStore, service: &str) -> Result<()> {
    super::delete_entries(store.search(&HashMap::from([("service", service)]))?)
}
