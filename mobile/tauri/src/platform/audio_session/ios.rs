//! iOS: `AVAudioSession.sharedInstance()`, read whole (any thread answers), its notifications (route change, interruption,
//! media services lost and reset) with what their `userInfo` says as Apple documents it, and its `outputVolume`, which
//! Apple documents as key-value observable: the volume keys move it.
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::OnceLock;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, AllocAnyThread};
use objc2_avf_audio::{
    AVAudioSession, AVAudioSessionCategoryOptions, AVAudioSessionInterruptionNotification,
    AVAudioSessionInterruptionOptionKey, AVAudioSessionInterruptionReasonKey,
    AVAudioSessionInterruptionTypeKey, AVAudioSessionMediaServicesWereLostNotification,
    AVAudioSessionMediaServicesWereResetNotification, AVAudioSessionPortBuiltInReceiver,
    AVAudioSessionPortBuiltInSpeaker, AVAudioSessionRouteChangeNotification,
    AVAudioSessionRouteChangePreviousRouteKey, AVAudioSessionRouteChangeReasonKey,
    AVAudioSessionRouteDescription,
};
use objc2_foundation::{
    NSDictionary, NSKeyValueObservingOptions, NSNotification, NSNotificationCenter,
    NSNotificationName, NSNumber, NSObjectNSKeyValueObserverRegistration, NSString,
};

use super::{Heard, Os};

pub struct Native;

impl Os for Native {
    fn on_phone() -> Option<bool> {
        // SAFETY: read-only messages to the shared session, reads of live port descriptions, and constants of AVFAudio.
        unsafe {
            let outputs = AVAudioSession::sharedInstance().currentRoute().outputs();
            if outputs.is_empty() {
                return None;
            }
            Some(outputs.iter().all(|port| {
                let kind = port.portType();
                [
                    AVAudioSessionPortBuiltInReceiver,
                    AVAudioSessionPortBuiltInSpeaker,
                ]
                .into_iter()
                .flatten()
                .any(|own| kind.isEqualToString(own))
            }))
        }
    }

    fn describe() -> Option<String> {
        // SAFETY: read-only messages to the shared session, which answers from any thread.
        unsafe {
            let session = AVAudioSession::sharedInstance();
            let route = session.currentRoute();
            Some(format!(
                "category {} mode {} options {} | route out [{}] in [{}] | outputVolume {:.3} otherAudioPlaying {} \
                 secondaryAudioShouldBeSilenced {} | sampleRate {} Hz (preferred {}) ioBuffer {:.4} s (preferred {:.4}) \
                 outputLatency {:.4} s outputChannels {}",
                session.category(),
                session.mode(),
                options(session.categoryOptions()),
                ports(&route, true),
                ports(&route, false),
                session.outputVolume(),
                session.isOtherAudioPlaying(),
                session.secondaryAudioShouldBeSilencedHint(),
                session.sampleRate(),
                session.preferredSampleRate(),
                session.IOBufferDuration(),
                session.preferredIOBufferDuration(),
                session.outputLatency(),
                session.outputNumberOfChannels(),
            ))
        }
    }

    fn observe(heard: Heard) -> Result<(), String> {
        // SAFETY: constants of AVFAudio, linked by the app.
        let names = unsafe {
            [
                (AVAudioSessionRouteChangeNotification, "route change"),
                (AVAudioSessionInterruptionNotification, "interruption"),
                (
                    AVAudioSessionMediaServicesWereLostNotification,
                    "media services lost",
                ),
                (
                    AVAudioSessionMediaServicesWereResetNotification,
                    "media services reset",
                ),
            ]
        };
        for (name, what) in names {
            let Some(name) = name else {
                return Err(format!("audio session: no {what} notification"));
            };
            notified(name, what, heard.clone());
        }
        volume(heard)
    }
}

/// Reports each notification `name` to `heard`, as `what` and what its `userInfo` says.
fn notified(name: &NSNotificationName, what: &'static str, heard: Heard) {
    let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
        // SAFETY: the center hands the block a live notification for the length of the call.
        let notification = unsafe { notification.as_ref() };
        heard(match notification.userInfo() {
            Some(info) => format!("{what}{}", details(&info)),
            None => what.to_owned(),
        });
    });
    // SAFETY: no object to filter by, no queue (the block runs on the thread that posts, and only hands a line on); the block
    // is Send + Sync (`Heard` is) and the center retains it.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(name),
            None,
            None,
            &block,
        )
    };
    // Kept for the life of the process, as the interruptions' observer.
    std::mem::forget(observer);
}

/// What a session notification's `userInfo` says: the route change's reason and the route before, the interruption's
/// type, options and reason.
fn details(info: &NSDictionary) -> String {
    // SAFETY: constants of AVFAudio, linked by the app.
    let keys = unsafe {
        (
            AVAudioSessionRouteChangeReasonKey,
            AVAudioSessionRouteChangePreviousRouteKey,
            AVAudioSessionInterruptionTypeKey,
            AVAudioSessionInterruptionOptionKey,
            AVAudioSessionInterruptionReasonKey,
        )
    };
    let mut said = String::new();
    if let Some(reason) = keys.0.and_then(|key| number(info, key)) {
        said.push_str(&format!(": reason {}", route_change_reason(reason)));
    }
    if let Some(before) = keys.1.and_then(|key| object(info, key)) {
        if let Ok(before) = before.downcast::<AVAudioSessionRouteDescription>() {
            said.push_str(&format!(
                ", before out [{}] in [{}]",
                ports(&before, true),
                ports(&before, false)
            ));
        }
    }
    if let Some(kind) = keys.2.and_then(|key| number(info, key)) {
        said.push_str(match kind {
            1 => ": began",
            0 => ": ended",
            _ => ": of an unknown type",
        });
    }
    if let Some(options) = keys.3.and_then(|key| number(info, key)) {
        said.push_str(&format!(", shouldResume {}", options & 1 == 1));
    }
    if let Some(reason) = keys.4.and_then(|key| number(info, key)) {
        said.push_str(&format!(", reason {}", interruption_reason(reason)));
    }
    said
}

/// `AVAudioSession.RouteChangeReason`, by its name.
fn route_change_reason(reason: usize) -> String {
    match reason {
        0 => "unknown".into(),
        1 => "newDeviceAvailable".into(),
        2 => "oldDeviceUnavailable".into(),
        3 => "categoryChange".into(),
        4 => "override".into(),
        6 => "wakeFromSleep".into(),
        7 => "noSuitableRouteForCategory".into(),
        8 => "routeConfigurationChange".into(),
        other => format!("{other}"),
    }
}

/// `AVAudioSession.InterruptionReason`, by its name.
fn interruption_reason(reason: usize) -> String {
    match reason {
        0 => "default".into(),
        1 => "appWasSuspended".into(),
        2 => "builtInMicMuted".into(),
        3 => "routeDisconnected".into(),
        other => format!("{other}"),
    }
}

/// The category's options, by name, and their bits.
fn options(options: AVAudioSessionCategoryOptions) -> String {
    let named = [
        (
            AVAudioSessionCategoryOptions::MixWithOthers,
            "mixWithOthers",
        ),
        (AVAudioSessionCategoryOptions::DuckOthers, "duckOthers"),
        (
            AVAudioSessionCategoryOptions::AllowBluetoothHFP,
            "allowBluetoothHFP",
        ),
        (
            AVAudioSessionCategoryOptions::DefaultToSpeaker,
            "defaultToSpeaker",
        ),
        (
            AVAudioSessionCategoryOptions::InterruptSpokenAudioAndMixWithOthers,
            "interruptSpokenAudioAndMixWithOthers",
        ),
        (
            AVAudioSessionCategoryOptions::AllowBluetoothA2DP,
            "allowBluetoothA2DP",
        ),
        (AVAudioSessionCategoryOptions::AllowAirPlay, "allowAirPlay"),
    ];
    let names: Vec<&str> = named
        .into_iter()
        .filter(|(option, _)| options.contains(*option))
        .map(|(_, name)| name)
        .collect();
    format!("[{}] (0x{:x})", names.join(","), options.0)
}

/// The route's outputs (`outputs`) or inputs, each as its port type: the system's enum name (`BluetoothA2DPOutput`), never the port's name, which is the person's ("Ana's AirPods"). The log is public.
fn ports(route: &AVAudioSessionRouteDescription, outputs: bool) -> String {
    // SAFETY: reads of a live route description and its ports.
    unsafe {
        let ports = if outputs {
            route.outputs()
        } else {
            route.inputs()
        };
        ports
            .iter()
            .map(|port| port.portType().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The object `info` holds under `key`.
fn object(info: &NSDictionary, key: &NSString) -> Option<Retained<AnyObject>> {
    let key: &AnyObject = key.as_ref();
    info.objectForKey(key)
}

/// The unsigned number `info` holds under `key`.
fn number(info: &NSDictionary, key: &NSString) -> Option<usize> {
    let value = object(info, key)?.downcast::<NSNumber>().ok()?;
    Some(value.unsignedIntegerValue())
}

/// Where the output volume's moves go: set once, by the one observer.
static VOLUME: OnceLock<Heard> = OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class has no ivars and does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "SidevoiceOutputVolumeObserver"]
    struct VolumeObserver;

    unsafe impl NSObjectProtocol for VolumeObserver {}

    impl VolumeObserver {
        /// Key-value observing's one callback: the session's `outputVolume` changed (the only key observed).
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&AnyObject>,
            _context: *mut c_void,
        ) {
            if let Some(heard) = VOLUME.get() {
                heard("output volume moved".to_owned());
            }
        }
    }
);

/// Reports every move of the session's `outputVolume` to `heard`.
fn volume(heard: Heard) -> Result<(), String> {
    if VOLUME.set(heard).is_err() {
        return Err("audio session: the output volume is already observed".into());
    }
    let observer: Retained<VolumeObserver> =
        // SAFETY: NSObject's `init`, on a class with no ivars.
        unsafe { msg_send![VolumeObserver::alloc(), init] };
    // SAFETY: the shared session is key-value observable for `outputVolume` (Apple's documentation); the observer is never
    // removed, so it lives as long as the process (forgotten below), and no context is used.
    unsafe {
        AVAudioSession::sharedInstance().addObserver_forKeyPath_options_context(
            &observer,
            &NSString::from_str("outputVolume"),
            NSKeyValueObservingOptions::New,
            std::ptr::null_mut(),
        );
    }
    std::mem::forget(observer);
    Ok(())
}
