//! The unpackaged desktop-app privacy gate, read with the existing maintained winreg crate.
//! AppCapability is documented for packaged apps; Sidevoice ships an unpackaged NSIS executable, which has no consent
//! prompt: `ask` is `permission`.
use super::{AccessState, Os};
use std::{io::ErrorKind, ptr};
use windows_sys::Win32::{
    System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED},
    UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION},
};
use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    RegKey, HKEY,
};

pub struct Native;
impl Os for Native {
    const SETTINGS_URL: Option<&'static str> = Some("ms-settings:privacy-microphone");

    fn permission() -> Result<AccessState, &'static str> {
        const MICROPHONE: &str =
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
        // Machine-wide microphone access and this user's desktop-app switch. Do not use the user root's
        // Store-app switch, nor per-executable usage timestamps as an authorization decision.
        Ok(permission([
            setting(HKEY_LOCAL_MACHINE, MICROPHONE),
            setting(HKEY_CURRENT_USER, &format!(r"{MICROPHONE}\NonPackaged")),
        ]))
    }

    /// The device worker joins the multithreaded apartment before cpal's first call. cpal would otherwise make it a
    /// single-threaded apartment, which must pump window messages; the worker parks on a channel and never would.
    /// cpal accepts the thread already being in the multithreaded apartment.
    fn on_worker_thread(work: impl FnOnce()) {
        let _apartment = Apartment::enter();
        work()
    }

    fn remote_session() -> Option<bool> {
        // SAFETY: reads one metric of this process's session and takes nothing that must stay valid.
        Some(unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0)
    }
}

/// This thread's membership of COM's multithreaded apartment, left on drop when entering it succeeded.
struct Apartment {
    entered: bool,
}

impl Apartment {
    fn enter() -> Self {
        // SAFETY: the reserved argument must be null; the call only affects this thread. S_OK and S_FALSE (already
        // initialised the same way) both count once and are balanced on drop. A failure (RPC_E_CHANGED_MODE when the
        // thread is already in another apartment) leaves nothing to balance, and cpal then uses COM as it finds it.
        let entered = unsafe { CoInitializeEx(ptr::null(), COINIT_MULTITHREADED as u32) } >= 0;
        Apartment { entered }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.entered {
            // SAFETY: balances this thread's successful CoInitializeEx, on the same thread.
            unsafe { CoUninitialize() }
        }
    }
}

/// One privacy switch's `Value`.
#[derive(Debug, PartialEq, Eq)]
enum Setting {
    Allow,
    Deny,
    /// No key or no value: Windows never wrote a choice.
    Unset,
    /// A value this code does not know, or a key it could not read.
    Unknown,
}

fn setting(hive: HKEY, path: &str) -> Setting {
    match RegKey::predef(hive).open_subkey(path).and_then(|key| key.get_value::<String, _>("Value")) {
        Ok(value) => match value.as_str() {
            "Allow" => Setting::Allow,
            "Deny" => Setting::Deny,
            _ => Setting::Unknown,
        },
        Err(error) if error.kind() == ErrorKind::NotFound => Setting::Unset,
        Err(_) => Setting::Unknown,
    }
}

/// A `Deny` in either switch denies. Otherwise an `Unset` switch counts as allowed: Windows' default for desktop apps
/// is allow, and a fresh install or a policy-free machine has no key until someone first opens the page. Only a value or
/// a key this code cannot interpret is `Unavailable`, which is never a grant.
fn permission(settings: [Setting; 2]) -> AccessState {
    if settings.contains(&Setting::Deny) {
        AccessState::Denied
    } else if settings.contains(&Setting::Unknown) {
        AccessState::Unavailable
    } else {
        AccessState::Granted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Setting::*;

    #[test]
    fn deny_wins_and_unset_follows_the_windows_default() {
        assert_eq!(permission([Allow, Allow]), AccessState::Granted);
        assert_eq!(permission([Unset, Unset]), AccessState::Granted);
        assert_eq!(permission([Allow, Unset]), AccessState::Granted);
        assert_eq!(permission([Allow, Deny]), AccessState::Denied);
        assert_eq!(permission([Deny, Allow]), AccessState::Denied);
        assert_eq!(permission([Deny, Unknown]), AccessState::Denied);
        assert_eq!(permission([Unknown, Deny]), AccessState::Denied);
        assert_eq!(permission([Allow, Unknown]), AccessState::Unavailable);
        assert_eq!(permission([Unknown, Unset]), AccessState::Unavailable);
    }
}
