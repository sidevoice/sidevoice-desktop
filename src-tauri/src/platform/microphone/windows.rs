//! The unpackaged desktop-app privacy gate, read with the existing maintained winreg crate.
//! AppCapability is documented for packaged apps; Sidevoice ships an unpackaged NSIS executable.
use super::{privacy_state, AccessState, BackendObservation, MicrophoneAccess};
use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    RegKey,
};

pub struct NativeMicrophone;
impl MicrophoneAccess for NativeMicrophone {
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
