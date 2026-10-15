//! The keys of remote providers (OpenAI, ElevenLabs), kept in the macOS keychain: one generic password per provider,
//! under the service `dev.sidevoice.desktop.providers`. The engine asks for a key each time it needs one
//! (`Credentials`); the page may set, clear and ask whether there is one (`host.engine`), never read it back. The
//! keychain is macOS's; elsewhere the app keeps no keys.
#![cfg(target_os = "macos")]

use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};
use sidevoice_desktop_engine::sidevoice_engine::{self, async_trait, Credentials};
use std::ffi::{c_void, CString};

const SERVICE: &str = "dev.sidevoice.desktop.providers";
/// errSecItemNotFound.
const NOT_FOUND: i32 = -25300;

/// The keychain, as the engine's host asks it.
pub struct Keychain;

#[async_trait]
impl Credentials for Keychain {
    async fn credential(&self, provider: &str) -> sidevoice_engine::Result<Option<String>> {
        read(provider).map_err(|_| sidevoice_engine::Error::new("credentials-failed"))
    }
}

/// Whether `provider` is a provider id the keychain keeps a key for: lowercase letters, digits and dashes.
pub fn valid(provider: &str) -> bool {
    (1..=32).contains(&provider.len())
        && provider.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The key of `provider`, if the keychain has one.
pub fn read(provider: &str) -> Result<Option<String>, String> {
    match get_generic_password(SERVICE, provider) {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| "a key that is not text".into()),
        Err(e) if e.code() == NOT_FOUND => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Keeps `key` for `provider`, or removes the one it has when `key` is `None`.
pub fn write(provider: &str, key: Option<&str>) -> Result<(), String> {
    match key {
        Some(key) => set_generic_password(SERVICE, provider, key.as_bytes()).map_err(|e| e.to_string()),
        None => match delete_generic_password(SERVICE, provider) {
            Err(e) if e.code() != NOT_FOUND => Err(e.to_string()),
            _ => Ok(()),
        },
    }
}

/// Remove only generic passwords under this app's exact service, including provider IDs added later.
/// `SecItemDelete` deletes every matching item; enumerating today's providers would leave future keys behind.
pub fn delete_service() -> Result<(), String> {
    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecClass: *const c_void;
        static kSecClassGenericPassword: *const c_void;
        static kSecAttrService: *const c_void;
        fn SecItemDelete(query: *const c_void) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(allocator: *const c_void, text: *const i8, encoding: u32) -> *const c_void;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(value: *const c_void);
    }
    let service = CString::new(SERVICE).map_err(|_| "invalid service".to_string())?;
    unsafe {
        let name = CFStringCreateWithCString(std::ptr::null(), service.as_ptr(), 0x0800_0100);
        if name.is_null() {
            return Err("service could not be encoded".into());
        }
        let keys = [kSecClass, kSecAttrService];
        let values = [kSecClassGenericPassword, name];
        let query =
            CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), values.as_ptr(), 2, std::ptr::null(), std::ptr::null());
        if query.is_null() {
            CFRelease(name);
            return Err("query could not be made".into());
        }
        let result = SecItemDelete(query);
        CFRelease(query);
        CFRelease(name);
        if result == 0 || result == NOT_FOUND {
            Ok(())
        } else {
            Err(format!("Keychain status {result}"))
        }
    }
}
