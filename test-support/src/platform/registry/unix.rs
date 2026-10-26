//! Unix local-host installer adapter; the local-host crate is native-only for now.
use super::{RegistryTransport, Response};
use crate::fake_registry::FakeRegistry;
use sidevoice_local_host::{install::Failure, npm::Fetch};

impl Fetch for FakeRegistry {
    fn get(&self, url: &str) -> Result<sidevoice_local_host::npm::Response, Failure> {
        let Response { status, length, body } = RegistryTransport::get(self, url);
        Ok(sidevoice_local_host::npm::Response { status, length, body })
    }
}
