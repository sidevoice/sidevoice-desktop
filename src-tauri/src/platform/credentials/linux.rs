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

// The synchronous keyring backend cannot cancel an outstanding D-Bus prompt. Do not
// detach a new worker for every retry: retain one admission slot until it really ends.
// A timed-out write has an unknown outcome; refusing overlapping calls prevents a late
// completion from overwriting a subsequent write or running after a successful clear.
static OPERATION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) fn execute<T: Send + 'static>(
    operation: impl FnOnce() -> std::result::Result<T, super::Error> + Send + 'static,
) -> std::result::Result<T, super::Error> {
    bounded(&OPERATION, WAIT, operation)
}

pub(super) fn bounded<T: Send + 'static>(
    busy: &'static std::sync::atomic::AtomicBool,
    timeout: std::time::Duration,
    operation: impl FnOnce() -> std::result::Result<T, super::Error> + Send + 'static,
) -> std::result::Result<T, super::Error> {
    use std::sync::{atomic::Ordering, mpsc};
    busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).map_err(|_| super::Error::Unavailable)?;
    struct Admission(&'static std::sync::atomic::AtomicBool);
    impl Drop for Admission {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let admission = Admission(busy);
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("credentials".into())
        .spawn(move || {
            let result = operation();
            drop(admission);
            let _ = sender.send(result);
        })
        .map_err(|_| super::Error::Unavailable)?;
    receiver.recv_timeout(timeout).map_err(|_| super::Error::Unavailable)?
}
