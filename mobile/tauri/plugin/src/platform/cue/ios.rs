//! iOS: an `AVAudioPlayer` on the cue's WAV, looping, in the audio session the native call has made active (the app's
//! play-and-record voice-chat session, `call::CALL_AUDIO`): it joins that session and changes nothing of it. The player
//! lives on the main thread, where the commands run it (`lib.rs` `cue`). A sound's file is decoded by `AVAudioFile`, which
//! reads a file, not bytes: it is written to the app's temporary folder for that, and removed.
use std::cell::RefCell;

use objc2::{rc::Retained, AnyThread};
use objc2_avf_audio::{AVAudioFile, AVAudioPCMBuffer, AVAudioPlayer};
use objc2_foundation::{NSData, NSString, NSURL};

use super::Os;
use crate::Cue;

pub struct Native;

thread_local! {
    /// The playing cue's player, and what it plays (its key), on the main thread.
    static PLAYER: RefCell<Option<(String, Retained<AVAudioPlayer>)>> = const { RefCell::new(None) };
}

impl Os for Native {
    fn available() -> bool {
        true
    }

    fn decode(m4a: &[u8]) -> Result<(Vec<f32>, u32), String> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sidevoice-cue-{}-{n}.m4a", std::process::id()));
        std::fs::write(&path, m4a).map_err(|error| format!("cue-decode: {error}"))?;
        let decoded = read(&path);
        let _ = std::fs::remove_file(&path);
        decoded
    }

    fn start(_: Cue, key: &str, wav: &[u8]) -> Result<(), String> {
        PLAYER.with(|slot| {
            let mut slot = slot.borrow_mut();
            // SAFETY: `isPlaying` and `stop` on a live player, on the main thread that made it.
            if let Some((playing, player)) = slot.as_ref() {
                if playing == key && unsafe { player.isPlaying() } {
                    return Ok(());
                }
                unsafe { player.stop() };
            }
            *slot = None;
            let data = NSData::with_bytes(wav);
            // SAFETY: a fresh player on a WAV the app made; every call below is on it, on the main thread.
            let player =
                unsafe { AVAudioPlayer::initWithData_error(AVAudioPlayer::alloc(), &data) }
                    .map_err(|error| error.localizedDescription().to_string())?;
            unsafe {
                // A negative count loops until stopped.
                player.setNumberOfLoops(-1);
                player.setVolume(1.0);
                player.prepareToPlay();
                if !player.play() {
                    return Err("cue-play-refused".into());
                }
            }
            *slot = Some((key.to_string(), player));
            Ok(())
        })
    }

    fn stop(_: Cue) {
        PLAYER.with(|slot| {
            if let Some((_, player)) = slot.borrow_mut().take() {
                // SAFETY: `stop` on a live player, on the main thread that made it.
                unsafe { player.stop() };
            }
        });
    }
}

/// The file at `path`, whole, in the standard processing format (deinterleaved 32-bit float), its channels mixed to one.
fn read(path: &std::path::Path) -> Result<(Vec<f32>, u32), String> {
    let path = path
        .to_str()
        .ok_or("cue-decode: a temporary path that is not UTF-8")?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    // SAFETY: a fresh file and buffer on the file the app just wrote, read once on this thread; the samples are copied out
    // before the buffer goes, within the frame length it reports and its channel count.
    unsafe {
        let file = AVAudioFile::initForReading_error(AVAudioFile::alloc(), &url)
            .map_err(|error| format!("cue-decode: {}", error.localizedDescription()))?;
        let format = file.processingFormat();
        let frames = u32::try_from(file.length()).map_err(|_| "cue-decode: too long")?;
        let buffer = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            &format,
            frames,
        )
        .ok_or("cue-decode: no buffer")?;
        file.readIntoBuffer_error(&buffer)
            .map_err(|error| format!("cue-decode: {}", error.localizedDescription()))?;
        let channels = format.channelCount() as usize;
        let length = buffer.frameLength() as usize;
        let data = buffer.floatChannelData();
        if data.is_null() || channels == 0 {
            return Err("cue-decode: not float samples".into());
        }
        let mut mono = vec![0.0_f32; length];
        for channel in 0..channels {
            let samples = std::slice::from_raw_parts((*data.add(channel)).as_ptr(), length);
            for (out, sample) in mono.iter_mut().zip(samples) {
                *out += sample / channels as f32;
            }
        }
        Ok((mono, format.sampleRate() as u32))
    }
}
