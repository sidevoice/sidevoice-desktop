use keyring_core::{CredentialStore, Result};
use std::{collections::HashMap, sync::Arc};

pub(super) fn store() -> Result<Arc<CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

// The production reset caller remains macOS-only for now.
#[allow(dead_code)]
pub(super) fn clear(store: &CredentialStore, service: &str) -> Result<()> {
    super::Entries(store.search(&HashMap::from([("service", service)]))?).delete()
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

/// Native provider credential store; failures never install a fallback or prevent a later retry.
pub struct ProviderCredentials;

impl super::Credentials for ProviderCredentials {}

impl super::Backend for ProviderCredentials {
    fn execute(&self, request: super::Request) -> std::result::Result<Option<String>, super::Error> {
        execute(move || request.run(store()?.as_ref(), super::SERVICE, entry, clear))
    }
}

pub(super) fn entry(store: &CredentialStore, service: &str, provider: &str) -> Result<keyring_core::Entry> {
    store.build(service, provider, None)
}

#[cfg(test)]
mod tests {
    use super::super::{tests::*, Credentials, Error};
    use super::ProviderCredentials;

    #[test]
    #[ignore = "requires an unlocked desktop Secret Service; unverified on headless CI"]
    fn native_contract() {
        contract(&native_test_store());
    }

    #[test]
    fn slow_backend_times_out_without_queuing_later_operations() {
        use std::{
            sync::{
                atomic::{AtomicBool, Ordering},
                mpsc,
            },
            time::{Duration, Instant},
        };
        static BUSY: AtomicBool = AtomicBool::new(false);
        let (release, blocked) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let started = Instant::now();
        // A fake backend waiting for an unanswered prompt. No real Secret Service needed.
        assert_eq!(
            super::bounded(&BUSY, Duration::from_millis(20), move || {
                blocked.recv().unwrap();
                finished.send(()).unwrap();
                Ok(())
            }),
            Err(Error::Unavailable)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(BUSY.load(Ordering::Acquire));
        let started = Instant::now();
        assert_eq!(
            super::bounded(&BUSY, Duration::from_secs(5), || -> Result<(), Error> {
                panic!("a retry must not queue another backend operation")
            }),
            Err(Error::Unavailable)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        release.send(()).unwrap();
        completion.recv_timeout(Duration::from_secs(2)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while BUSY.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(super::bounded(&BUSY, Duration::from_secs(2), || Ok(42)), Ok(42));
    }

    #[test]
    fn missing_secret_service_is_explicitly_unavailable() {
        const CHILD: &str = "SIDEVOICE_CREDENTIAL_UNAVAILABLE_TEST";
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(ProviderCredentials.read("openai"), Err(Error::Unavailable));
            assert_eq!(ProviderCredentials.write("openai", Some("test-only")), Err(Error::Unavailable));
            assert_eq!(ProviderCredentials.clear(), Err(Error::Unavailable));
            return;
        }
        // Isolate the bus environment from concurrent tests and any real user's keyring.
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("missing_secret_service_is_explicitly_unavailable")
            .env(CHILD, "1")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-sidevoice-test-bus")
            .status()
            .unwrap();
        assert!(status.success());
    }
}
