//! iOS: `AVAudioSessionInterruptionNotification`, posted when the system deactivates the session (a phone call, an alarm,
//! Siri, another app that takes the audio) and again when it is over, with `shouldResume` when the app should go on.
//! The notification is the system's own; the type and option are read from `userInfo` as Apple documents.
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_avf_audio::{
    AVAudioSessionInterruptionNotification, AVAudioSessionInterruptionOptionKey,
    AVAudioSessionInterruptionOptions, AVAudioSessionInterruptionType,
    AVAudioSessionInterruptionTypeKey,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSNumber, NSString};

use super::{Interruption, Listener, Os};

pub struct Native;

impl Os for Native {
    fn observe(listener: Listener) -> Result<(), String> {
        // SAFETY: a constant of AVFAudio, linked by the app.
        let Some(name) = (unsafe { AVAudioSessionInterruptionNotification }) else {
            return Err("audio-session-interruptions-unavailable".into());
        };
        let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
            // SAFETY: the center hands the block a live notification for the length of the call.
            let notification = unsafe { notification.as_ref() };
            if let Some(event) = read(notification) {
                listener(event);
            }
        });
        // SAFETY: no object to filter by, no queue (the block runs on the thread that posts, and only hands the event
        // on); the block is Send + Sync (a listener is) and the center retains it.
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                None,
                &block,
            )
        };
        // The center keeps the observer until it is removed, and it is not removed: it lives as long as the process.
        std::mem::forget(observer);
        Ok(())
    }
}

/// What `notification` says, if it is one the app can read.
fn read(notification: &NSNotification) -> Option<Interruption> {
    let info = notification.userInfo()?;
    // SAFETY: the keys are constants of AVFAudio.
    let (type_key, option_key) = unsafe {
        (
            AVAudioSessionInterruptionTypeKey?,
            AVAudioSessionInterruptionOptionKey?,
        )
    };
    let kind = number(&info, type_key)?;
    match AVAudioSessionInterruptionType(kind) {
        AVAudioSessionInterruptionType::Began => Some(Interruption::Began),
        AVAudioSessionInterruptionType::Ended => {
            // Only given when it ended; absent, the system advises nothing, which is not to resume.
            let options = AVAudioSessionInterruptionOptions(number(&info, option_key).unwrap_or(0));
            Some(Interruption::Ended {
                resume: options.contains(AVAudioSessionInterruptionOptions::ShouldResume),
            })
        }
        _ => None,
    }
}

/// The unsigned number `info` holds under `key`.
fn number(info: &objc2_foundation::NSDictionary, key: &NSString) -> Option<usize> {
    let key: &AnyObject = key.as_ref();
    let value: Retained<AnyObject> = info.objectForKey(key)?;
    let value = value.downcast::<NSNumber>().ok()?;
    Some(value.unsignedIntegerValue())
}
