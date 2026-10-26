//! Whether an audio input device exists, from cpal's host enumeration (WASAPI active endpoints, ALSA PCM hints).
//! Enumeration opens no stream and so never triggers a privacy prompt or a capture indicator.
use super::Devices;
use cpal::traits::HostTrait;

pub fn present() -> Devices {
    let Some(id) = cpal::available_hosts().into_iter().next() else { return Devices::Unknown };
    // A host or a listing that fails says nothing about the hardware.
    match cpal::host_from_id(id).and_then(|host| host.input_devices()) {
        Ok(mut devices) => devices.next().is_some().into(),
        Err(_) => Devices::Unknown,
    }
}
