//! The unpackaged desktop-app privacy gate, read with the existing maintained winreg crate.
//! AppCapability is documented for packaged apps; Sidevoice ships an unpackaged NSIS executable.
use super::{AccessState, BackendObservation, MicrophoneAccess};
use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    RegKey,
};

pub struct NativeMicrophone;
impl MicrophoneAccess for NativeMicrophone {
    const SETTINGS_URL: Option<&'static str> = Some("ms-settings:privacy-microphone");

    fn state(backend: BackendObservation) -> Result<AccessState, &'static str> {
        const MICROPHONE: &str =
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
        // Machine-wide microphone access and this user's desktop-app switch. Do not use the user root's
        // Store-app switch, nor per-executable usage timestamps as an authorization decision.
        let values =
            [(HKEY_LOCAL_MACHINE, MICROPHONE.to_owned()), (HKEY_CURRENT_USER, format!(r"{MICROPHONE}\NonPackaged"))]
                .map(|(hive, path)| {
                    RegKey::predef(hive).open_subkey(path).and_then(|key| key.get_value::<String, _>("Value")).ok()
                });
        Ok(privacy_state(backend, values))
    }
}

// Unknown registry keys must not turn a browser permission hint into an OS grant. A successful capture
// is stronger evidence; it still cannot override an explicit privacy denial read in this operation.
fn privacy_state(backend: BackendObservation, values: [Option<String>; 2]) -> AccessState {
    if values.iter().any(|value| value.as_deref() == Some("Deny")) {
        return AccessState::Denied;
    }
    let captured = backend.captured;
    let state = backend.state();
    if state == AccessState::Granted && !captured && values.iter().any(|value| value.as_deref() != Some("Allow")) {
        AccessState::Unavailable
    } else {
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(permission: AccessState, has_device: Option<bool>) -> BackendObservation {
        BackendObservation { permission, has_device, ..Default::default() }
    }

    #[test]
    fn privacy_changes_override_backend_hints_and_unknown_is_not_a_grant() {
        let allowed = || Some("Allow".into());
        let denied = || Some("Deny".into());
        let query = || observation(AccessState::Granted, Some(true));
        assert_eq!(privacy_state(query(), [allowed(), allowed()]), AccessState::Granted);
        assert_eq!(privacy_state(query(), [allowed(), denied()]), AccessState::Denied);
        assert_eq!(privacy_state(query(), [denied(), allowed()]), AccessState::Denied);
        assert_eq!(privacy_state(query(), [allowed(), allowed()]), AccessState::Granted);
        assert_eq!(privacy_state(query(), [None, allowed()]), AccessState::Unavailable);
        assert_eq!(privacy_state(query(), [allowed(), Some("future value".into())]), AccessState::Unavailable);
        assert_eq!(
            privacy_state(
                BackendObservation { captured: true, has_device: Some(true), ..Default::default() },
                [None, None]
            ),
            AccessState::Granted
        );
        assert_eq!(
            privacy_state(
                BackendObservation { captured: true, has_device: Some(true), ..Default::default() },
                [None, denied()]
            ),
            AccessState::Denied
        );
        assert_eq!(
            privacy_state(observation(AccessState::Granted, Some(false)), [allowed(), denied()]),
            AccessState::Denied
        );
    }
}
