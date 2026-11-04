//! Lowering one app at a time on macOS 14.4 and later, without touching the output volume (docs/MEDIA_DUCK.md): a Core Audio
//! process tap silences the app while this app reads it (`mutedWhenTapped`), and a private aggregate device plays what it
//! read at a lower gain. Giving back is destroying the tap: the app's own volume is never touched. It needs System Audio
//! Recording (TCC `kTCCServiceAudioCapture`); no tap that mutes is ever made without it.
//!
//! The functions that macOS 13 lacks are looked up when called (`dlsym`), never linked: a binary that named them would not
//! start there. Research and sources: briefs/night-2026-11-03/research-187-macos-ducking.md.
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::AllocAnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey, kAudioAggregateDeviceMainSubDeviceKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDevicePropertyMainSubDevice, kAudioAggregateDeviceSubDeviceListKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey,
    kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyIOProcStreamUsage,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreams, kAudioDevicePropertyTransportType,
    kAudioDeviceTransportTypeAggregate, kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices, kAudioHardwarePropertyProcessObjectList,
    kAudioHardwarePropertyTapList, kAudioHardwarePropertyTranslatePIDToProcessObject,
    kAudioHardwarePropertyTranslateUIDToDevice, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectSystemObject, kAudioProcessPropertyBundleID,
    kAudioProcessPropertyIsRunningOutput, kAudioProcessPropertyPID, kAudioSubDeviceUIDKey,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyDescription, AudioDeviceCreateIOProcID,
    AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareDestroyAggregateDevice, AudioHardwareIOProcStreamUsage,
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
    AudioObjectPropertyAddress, AudioObjectRemovePropertyListener, CATapDescription, CATapMuteBehavior,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSObject, NSString};
use sidevoice_desktop_core::media_duck::Access;
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What names this app's taps and aggregate devices, so a run that died can find what it left.
const PREFIX: &str = "Sidevoice-duck-";
/// How long a change of gain takes, so the dip and the restore make no click.
const RAMP_SECS: f32 = 0.03;
/// How long the system is given to settle after the output changes (a Bluetooth headset switching profile changes its rate
/// more than once) before the taps are made again.
const SETTLE: Duration = Duration::from_millis(250);
/// How long a question of System Audio Recording is waited for.
const ASKING: Duration = Duration::from_secs(60);
/// System processes whose sound is never another app's media: alerts, Siri, dictation, speech, accessibility.
const SYSTEM: [&str; 10] = [
    "com.apple.audio.",
    "com.apple.systemsound",
    "com.apple.coreaudiod",
    "com.apple.siri",
    "com.apple.assistant",
    "com.apple.SpeechRecognitionCore",
    "com.apple.speech.",
    "com.apple.corespeech",
    "com.apple.accessibility.",
    "com.apple.VoiceOver",
];

type CreateTap = unsafe extern "C" fn(*const CATapDescription, *mut u32) -> i32;
type DestroyTap = unsafe extern "C" fn(u32) -> i32;
type Responsible = unsafe extern "C" fn(i32) -> i32;
type Preflight = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;

/// A C function looked up by name in what is loaded (`handle` null) or in `handle`.
fn symbol(handle: *mut c_void, name: &CStr) -> Option<NonNull<c_void>> {
    let handle = if handle.is_null() { libc::RTLD_DEFAULT } else { handle };
    // SAFETY: dlsym with a valid handle and a nul-terminated name; the answer is only a pointer.
    NonNull::new(unsafe { libc::dlsym(handle, name.as_ptr()) })
}

struct Api {
    create: CreateTap,
    destroy: DestroyTap,
}

/// The tap functions, where this macOS has them (14.2 and later; 14.4 is required, see [`supported`]).
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let create = symbol(std::ptr::null_mut(), c"AudioHardwareCreateProcessTap")?;
        let destroy = symbol(std::ptr::null_mut(), c"AudioHardwareDestroyProcessTap")?;
        AnyClass::get(c"CATapDescription")?;
        // SAFETY: the functions of those names have these C signatures (AudioHardware.h).
        Some(unsafe {
            Api {
                create: std::mem::transmute::<*mut c_void, CreateTap>(create.as_ptr()),
                destroy: std::mem::transmute::<*mut c_void, DestroyTap>(destroy.as_ptr()),
            }
        })
    })
    .as_ref()
}

/// The macOS version, `(major, minor)`.
fn version() -> (u32, u32) {
    let mut buffer = [0u8; 32];
    let mut size = buffer.len();
    // SAFETY: sysctl of a string into a local buffer of the size given.
    let status = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return (0, 0);
    }
    let text = String::from_utf8_lossy(&buffer[..size.saturating_sub(1)]).into_owned();
    let mut parts = text.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// Whether taps are used here: macOS 14.4 and later. The functions are there from 14.2, but System Audio Recording, its
/// usage string and the prior art this follows start at 14.4; 14.2 and 14.3 are left to the Apple events path.
pub(super) fn supported() -> bool {
    version() >= (14, 4) && api().is_some()
}

/// What the system says of System Audio Recording, through the TCC check every capture app uses (`TCCAccessPreflight`,
/// private and not entitlement-gated): `unknown` when it cannot be asked. Never asks the person.
pub(super) fn permission() -> Access {
    static PREFLIGHT: OnceLock<Option<Preflight>> = OnceLock::new();
    let preflight = PREFLIGHT.get_or_init(|| {
        // SAFETY: dlopen of a system framework by its path; it stays loaded.
        let handle = unsafe {
            libc::dlopen(c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC".as_ptr(), libc::RTLD_LAZY)
        };
        if handle.is_null() {
            return None;
        }
        // SAFETY: `TCCAccessPreflight(CFStringRef service, CFDictionaryRef options) -> int`.
        symbol(handle, c"TCCAccessPreflight")
            .map(|f| unsafe { std::mem::transmute::<*mut c_void, Preflight>(f.as_ptr()) })
    });
    let Some(preflight) = preflight else { return Access::Unknown };
    let service = NSString::from_str("kTCCServiceAudioCapture");
    // SAFETY: an NSString is a CFString (toll-free bridged), alive for the call; no options.
    match unsafe { preflight(Retained::as_ptr(&service).cast(), std::ptr::null()) } {
        0 => Access::Granted,
        1 => Access::Denied,
        _ => Access::Undetermined,
    }
}

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain }
}

/// One fixed-size property of `object` (with an optional qualifier), as `T`.
fn get<T: Copy, Q>(object: u32, selector: u32, scope: u32, qualifier: Option<&Q>) -> Option<T> {
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = std::mem::size_of::<T>() as u32;
    let mut at = address(selector, scope);
    let (q_size, q_ptr) =
        qualifier.map_or((0, std::ptr::null()), |q| (std::mem::size_of::<Q>() as u32, (q as *const Q).cast()));
    // SAFETY: a Core Audio property read into a local of the size `T` has, which is the size the property has.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut at),
            q_size,
            q_ptr,
            NonNull::from(&mut size),
            NonNull::new_unchecked(value.as_mut_ptr()).cast(),
        )
    };
    // SAFETY: the system wrote the whole value.
    (status == 0 && size as usize == std::mem::size_of::<T>()).then(|| unsafe { value.assume_init() })
}

/// A list property of `object`: object ids.
fn list(object: u32, selector: u32, scope: u32) -> Vec<u32> {
    let mut at = address(selector, scope);
    let mut size = 0u32;
    // SAFETY: the size of a Core Audio property.
    if unsafe {
        AudioObjectGetPropertyDataSize(object, NonNull::from(&mut at), 0, std::ptr::null(), NonNull::from(&mut size))
    } != 0
    {
        return Vec::new();
    }
    let mut ids = vec![0u32; size as usize / std::mem::size_of::<u32>()];
    // SAFETY: read into a buffer of the size just asked.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(ids.as_mut_ptr()).cast(),
        )
    };
    if status != 0 {
        return Vec::new();
    }
    ids.truncate(size as usize / std::mem::size_of::<u32>());
    ids
}

/// A string property (a CFString the caller owns).
fn string(object: u32, selector: u32) -> Option<String> {
    let raw: *mut NSString = get::<*mut NSString, ()>(object, selector, kAudioObjectPropertyScopeGlobal, None)?;
    // SAFETY: the property hands over a retained CFString, which is an NSString.
    let text = unsafe { Retained::from_raw(raw) }?;
    Some(text.to_string())
}

/// This process's own id and what is responsible for a process: the app that launched an XPC helper (a WebKit GPU process
/// is its app's), through `responsibility_get_pid_responsible_for_pid` (libsystem; what Activity Monitor uses).
fn responsible(pid: i32) -> Option<i32> {
    static FUNCTION: OnceLock<Option<Responsible>> = OnceLock::new();
    let function = FUNCTION.get_or_init(|| {
        // SAFETY: `pid_t responsibility_get_pid_responsible_for_pid(pid_t)`.
        symbol(std::ptr::null_mut(), c"responsibility_get_pid_responsible_for_pid")
            .map(|f| unsafe { std::mem::transmute::<*mut c_void, Responsible>(f.as_ptr()) })
    });
    // SAFETY: a plain call with a pid.
    function.map(|f| unsafe { f(pid) }).filter(|p| *p > 0)
}

/// A process that is playing, as the audio system lists it.
pub(super) struct Playing {
    pub(super) pid: i32,
    pub(super) bundle: String,
}

/// Whether `pid` is this app's own sound: this process, one it started, or one it is responsible for (its webview's
/// WebKit helpers, whose parent is launchd).
fn ours(pid: i32, table: &super::ours::Ours) -> bool {
    let me = std::process::id() as i32;
    pid == me || responsible(pid) == Some(me) || table.has(pid as u32)
}

/// The processes with output running now, without this app's own sound or the system's. No permission is needed to list
/// them.
pub(super) fn playing() -> Vec<Playing> {
    let table = super::ours::Ours::new();
    list(kAudioObjectSystemObject as u32, kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyScopeGlobal)
        .into_iter()
        .filter(|&process| {
            get::<u32, ()>(process, kAudioProcessPropertyIsRunningOutput, kAudioObjectPropertyScopeGlobal, None)
                == Some(1)
        })
        .filter_map(|process| {
            let pid = get::<i32, ()>(process, kAudioProcessPropertyPID, kAudioObjectPropertyScopeGlobal, None)?;
            let bundle = string(process, kAudioProcessPropertyBundleID).unwrap_or_default();
            (pid > 1 && !ours(pid, &table) && !SYSTEM.iter().any(|s| bundle.starts_with(s)))
                .then_some(Playing { pid, bundle })
        })
        .collect()
}

/// The audio system's object for a process, if it is a client.
fn process_object(pid: i32) -> Option<u32> {
    get::<u32, i32>(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyTranslatePIDToProcessObject,
        kAudioObjectPropertyScopeGlobal,
        Some(&pid),
    )
    .filter(|id| *id != 0)
}

/// Whether a process still has output running.
pub(super) fn running(pid: i32) -> bool {
    process_object(pid).is_some_and(|p| {
        get::<u32, ()>(p, kAudioProcessPropertyIsRunningOutput, kAudioObjectPropertyScopeGlobal, None) == Some(1)
    })
}

/// The gain the IOProc plays at, shared with it without locks: the target, and where it is now on its way there.
struct Gain {
    target: AtomicU32,
    current: AtomicU32,
    /// How far the gain moves per frame.
    step: AtomicU32,
}

impl Gain {
    fn new(current: f32, target: f32) -> Box<Gain> {
        Box::new(Gain {
            target: AtomicU32::new(target.to_bits()),
            current: AtomicU32::new(current.to_bits()),
            step: AtomicU32::new((1.0 / (RAMP_SECS * 48_000.0)).to_bits()),
        })
    }
    fn set(&self, target: f32) {
        self.target.store(target.to_bits(), Ordering::Relaxed);
    }
    fn silence(&self) {
        self.target.store(0f32.to_bits(), Ordering::Relaxed);
        self.current.store(0f32.to_bits(), Ordering::Relaxed);
    }
}

/// One channel of a buffer list: where its first sample is, how far apart its samples are, how many frames it has.
#[derive(Clone, Copy)]
struct Channel {
    data: *mut f32,
    stride: usize,
    frames: usize,
}

fn buffers(list: &AudioBufferList) -> &[AudioBuffer] {
    // SAFETY: a buffer list holds `mNumberBuffers` buffers in a row from `mBuffers`.
    unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize) }
}

/// The last two channels with data of a buffer list (the tap's stereo mixdown comes after the hardware's input streams,
/// which are turned off), left then right; one channel is used for both.
fn tap_channels(list: &AudioBufferList) -> Option<[Channel; 2]> {
    let mut found: [Option<Channel>; 2] = [None, None];
    let mut n = 0;
    'outer: for buffer in buffers(list).iter().rev() {
        if buffer.mData.is_null() || buffer.mNumberChannels == 0 {
            continue;
        }
        let stride = buffer.mNumberChannels as usize;
        let frames = buffer.mDataByteSize as usize / (4 * stride);
        for c in (0..stride).rev() {
            // SAFETY: within the buffer: `c` is below its channel count.
            found[n] = Some(Channel { data: unsafe { buffer.mData.cast::<f32>().add(c) }, stride, frames });
            n += 1;
            if n == 2 {
                break 'outer;
            }
        }
    }
    match found {
        [Some(right), Some(left)] => Some([left, right]),
        [Some(only), None] => Some([only, only]),
        _ => None,
    }
}

/// The IOProc: what the tap read, at the gain, to the output; real-time, so no lock, allocation or log.
unsafe extern "C-unwind" fn render(
    _device: u32,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    data: *mut c_void,
) -> i32 {
    // SAFETY: `data` is the `Gain` this IOProc was made with, which outlives it; the lists are the system's for this cycle.
    let (gain, input, output) = unsafe { (&*(data as *const Gain), input.as_ref(), output.as_ref()) };
    let Some([left, right]) = tap_channels(input) else { return 0 };
    let Some(out) = buffers(output).iter().find(|b| !b.mData.is_null() && b.mNumberChannels > 0) else { return 0 };
    let channels = out.mNumberChannels as usize;
    let frames = (out.mDataByteSize as usize / (4 * channels)).min(left.frames).min(right.frames);
    let target = f32::from_bits(gain.target.load(Ordering::Relaxed));
    let step = f32::from_bits(gain.step.load(Ordering::Relaxed));
    let mut current = f32::from_bits(gain.current.load(Ordering::Relaxed));
    let samples = out.mData.cast::<f32>();
    for frame in 0..frames {
        current = if current < target { (current + step).min(target) } else { (current - step).max(target) };
        // SAFETY: `frame` is below the frames of every channel read and of the output buffer.
        unsafe {
            let (l, r) = (*left.data.add(frame * left.stride), *right.data.add(frame * right.stride));
            if channels == 1 {
                *samples.add(frame) = (l + r) * 0.5 * current;
            } else {
                // Left and right to the first two channels; any others stay as the system zeroed them.
                *samples.add(frame * channels) = l * current;
                *samples.add(frame * channels + 1) = r * current;
            }
        }
    }
    gain.current.store(current.to_bits(), Ordering::Relaxed);
    0
}

/// The output the taps play to: the default output, or the main hardware device of an aggregate one (aggregates do not
/// nest).
#[derive(Clone, PartialEq)]
struct Output {
    device: u32,
    uid: String,
    bluetooth: bool,
    /// Its rate when the tap was made: an IOProc that runs on while it changes plays silence.
    rate: Option<f64>,
}

fn output() -> Option<Output> {
    let mut device = get::<u32, ()>(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
        None,
    )
    .filter(|d| *d != 0)?;
    let transport =
        |d: u32| get::<u32, ()>(d, kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal, None);
    if transport(device) == Some(kAudioDeviceTransportTypeAggregate) {
        let main = string(device, kAudioAggregateDevicePropertyMainSubDevice)?;
        let uid = NSString::from_str(&main);
        let raw = Retained::as_ptr(&uid);
        device = get::<u32, *const NSString>(
            kAudioObjectSystemObject as u32,
            kAudioHardwarePropertyTranslateUIDToDevice,
            kAudioObjectPropertyScopeGlobal,
            Some(&raw),
        )
        .filter(|d| *d != 0)?;
    }
    let uid = string(device, kAudioDevicePropertyDeviceUID)?;
    let kind = transport(device);
    let bluetooth =
        kind == Some(kAudioDeviceTransportTypeBluetooth) || kind == Some(kAudioDeviceTransportTypeBluetoothLE);
    let rate = get::<f64, ()>(device, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, None);
    Some(Output { device, uid, bluetooth, rate })
}

/// One tap with its aggregate device and IOProc, playing one process (or, to ask for the permission, nothing).
struct Tap {
    tap: u32,
    aggregate: u32,
    proc_id: AudioDeviceIOProcID,
    gain: Box<Gain>,
    output: Output,
}

// SAFETY: the ids are plain numbers; the gain is shared with the IOProc only through atomics.
unsafe impl Send for Tap {}

/// A tap that is let go is stopped and destroyed, whatever path let it go.
impl Drop for Tap {
    fn drop(&mut self) {
        if let Some(api) = api() {
            self.stop(api);
        }
    }
}

/// What a tap reads: one process, muted while it is read, or (to ask the person) every process, unmuted.
enum Of {
    Process(u32),
    Nothing,
}

fn ns_number(value: u32) -> Retained<NSObject> {
    Retained::into_super(Retained::into_super(NSNumber::new_u32(value)))
}

fn ns_string(value: &str) -> Retained<NSObject> {
    Retained::into_super(NSString::from_str(value))
}

/// An array of one dictionary, as an object.
fn one(entry: Retained<NSDictionary<NSString, NSObject>>) -> Retained<NSObject> {
    Retained::into_super(NSArray::from_retained_slice(&[entry]))
}

fn key(key: &CStr) -> Retained<NSString> {
    NSString::from_str(key.to_str().unwrap_or_default())
}

fn dictionary(entries: Vec<(&CStr, Retained<NSObject>)>) -> Retained<NSDictionary<NSString, NSObject>> {
    let keys: Vec<Retained<NSString>> = entries.iter().map(|(k, _)| key(k)).collect();
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<Retained<NSObject>> = entries.into_iter().map(|(_, v)| v).collect();
    NSDictionary::from_retained_objects(&keys, &values)
}

impl Tap {
    /// Makes and starts a tap of `of` playing to `output` at `gain`, from `from`. Undoes what it made when a step fails.
    fn start(api: &Api, of: Of, output: &Output, from: f32, gain: f32) -> Option<Tap> {
        let name = format!("{PREFIX}{}", std::process::id());
        // SAFETY: a tap description of one process (or of none, excluded from a global tap), configured before use.
        let description = unsafe {
            let (description, mute) = match of {
                Of::Process(process) => (
                    CATapDescription::initStereoMixdownOfProcesses(
                        CATapDescription::alloc(),
                        &NSArray::from_retained_slice(&[NSNumber::new_u32(process)]),
                    ),
                    CATapMuteBehavior::MutedWhenTapped,
                ),
                Of::Nothing => (
                    CATapDescription::initStereoGlobalTapButExcludeProcesses(
                        CATapDescription::alloc(),
                        &NSArray::from_retained_slice(&[]),
                    ),
                    CATapMuteBehavior::Unmuted,
                ),
            };
            description.setPrivate(true);
            description.setMuteBehavior(mute);
            description.setName(&NSString::from_str(&name));
            description
        };
        // SAFETY: reading the description's UUID.
        let tap_uid = unsafe { description.UUID().UUIDString() }.to_string();
        let mut tap = 0u32;
        // SAFETY: the description is alive for the call; the id is written to a local.
        if unsafe { (api.create)(Retained::as_ptr(&description), &mut tap) } != 0 || tap == 0 {
            return None;
        }
        let made = Tap { tap, aggregate: 0, proc_id: None, gain: Gain::new(from, gain), output: output.clone() };
        made.build(api, &tap_uid)
    }

    /// The aggregate device and IOProc around a tap that exists; the tap is destroyed when this fails.
    fn build(mut self, api: &Api, tap_uid: &str) -> Option<Tap> {
        let output = self.output.clone();
        let description = dictionary(vec![
            (kAudioAggregateDeviceUIDKey, ns_string(&format!("{PREFIX}{tap_uid}"))),
            (kAudioAggregateDeviceNameKey, ns_string(&format!("{PREFIX}{}", std::process::id()))),
            (kAudioAggregateDeviceIsPrivateKey, ns_number(1)),
            (kAudioAggregateDeviceIsStackedKey, ns_number(0)),
            (kAudioAggregateDeviceMainSubDeviceKey, ns_string(&output.uid)),
            (kAudioAggregateDeviceTapAutoStartKey, ns_number(1)),
            (
                kAudioAggregateDeviceSubDeviceListKey,
                one(dictionary(vec![(kAudioSubDeviceUIDKey, ns_string(&output.uid))])),
            ),
            (
                kAudioAggregateDeviceTapListKey,
                one(dictionary(vec![
                    (kAudioSubTapUIDKey, ns_string(tap_uid)),
                    // Drift compensation on Bluetooth makes the system add or drop a sample now and then: a crackle.
                    (kAudioSubTapDriftCompensationKey, ns_number(u32::from(!output.bluetooth))),
                ])),
            ),
        ]);
        let mut aggregate = 0u32;
        // SAFETY: an NSDictionary is a CFDictionary (toll-free bridged), alive for the call.
        let status = unsafe {
            AudioHardwareCreateAggregateDevice(
                &*(Retained::as_ptr(&description) as *const CFDictionary),
                NonNull::from(&mut aggregate),
            )
        };
        if status != 0 || aggregate == 0 {
            self.stop(api);
            return None;
        }
        self.aggregate = aggregate;
        // The rate the IOProc runs at, for the ramp's step.
        if let Some(rate) =
            get::<f64, ()>(aggregate, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, None)
        {
            let step = 1.0 / (RAMP_SECS * rate.max(8_000.0) as f32);
            self.gain.step.store(step.to_bits(), Ordering::Relaxed);
        }
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: the IOProc gets the gain, which lives in this `Tap` until the IOProc is destroyed.
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                aggregate,
                Some(render),
                (&*self.gain as *const Gain).cast_mut().cast(),
                NonNull::from(&mut proc_id),
            )
        };
        if status != 0 || proc_id.is_none() {
            self.stop(api);
            return None;
        }
        self.proc_id = proc_id;
        self.hardware_input_off(&output);
        // SAFETY: starting the IOProc just made on its device.
        if unsafe { AudioDeviceStart(aggregate, proc_id) } != 0 {
            self.stop(api);
            return None;
        }
        Some(self)
    }

    /// Turns off the output device's own input streams in the aggregate, so a headset's microphone is not opened by it
    /// (that alone would switch Bluetooth to its call profile). The tap's stream comes after them.
    fn hardware_input_off(&self, output: &Output) {
        let Some(proc_id) = self.proc_id else { return };
        let hardware = list(output.device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput).len();
        if hardware == 0 {
            return;
        }
        let mut at = address(kAudioDevicePropertyIOProcStreamUsage, kAudioObjectPropertyScopeInput);
        let mut size = 0u32;
        // SAFETY: the size of the stream usage of the aggregate's input.
        if unsafe {
            AudioObjectGetPropertyDataSize(
                self.aggregate,
                NonNull::from(&mut at),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
            )
        } != 0
        {
            return;
        }
        let header = std::mem::size_of::<AudioHardwareIOProcStreamUsage>();
        let mut bytes = vec![0u32; (size as usize).max(header).div_ceil(4)];
        let usage = bytes.as_mut_ptr().cast::<AudioHardwareIOProcStreamUsage>();
        // SAFETY: the buffer holds the header and `mNumberStreams` flags (the size the system asked for); the IOProc is
        // named before the read, as the property requires.
        unsafe {
            (*usage).mIOProc = NonNull::new_unchecked(proc_id as *mut c_void);
            if AudioObjectGetPropertyData(
                self.aggregate,
                NonNull::from(&mut at),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
                NonNull::new_unchecked(usage).cast(),
            ) != 0
            {
                return;
            }
            let streams = (*usage).mNumberStreams as usize;
            let flags = std::ptr::addr_of_mut!((*usage).mStreamIsOn).cast::<u32>();
            for i in 0..hardware.min(streams.saturating_sub(1)) {
                *flags.add(i) = 0;
            }
            objc2_core_audio::AudioObjectSetPropertyData(
                self.aggregate,
                NonNull::from(&mut at),
                0,
                std::ptr::null(),
                size,
                NonNull::new_unchecked(usage).cast(),
            );
        }
    }

    /// Stops and destroys everything, in the order Core Audio needs: IOProc, aggregate, tap.
    fn stop(&mut self, api: &Api) {
        // SAFETY: each id is this tap's own, destroyed once.
        unsafe {
            if self.proc_id.is_some() {
                AudioDeviceStop(self.aggregate, self.proc_id);
                AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
                self.proc_id = None;
            }
            if self.aggregate != 0 {
                AudioHardwareDestroyAggregateDevice(self.aggregate);
                self.aggregate = 0;
            }
            if self.tap != 0 {
                (api.destroy)(self.tap);
                self.tap = 0;
            }
        }
    }
}

/// The taps there are, by process id, and the output device whose changes are listened to.
#[derive(Default)]
struct Taps {
    by_pid: HashMap<i32, Tap>,
    listening: Option<u32>,
}

fn taps() -> std::sync::MutexGuard<'static, Taps> {
    static TAPS: OnceLock<Mutex<Taps>> = OnceLock::new();
    TAPS.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The gain a process is played at, when it is tapped.
pub(super) fn gain(pid: i32) -> Option<f32> {
    taps().by_pid.get(&pid).map(|t| f32::from_bits(t.gain.target.load(Ordering::Relaxed)))
}

/// Plays `pid` at `gain`: a gain below 1 taps it (only with the permission), 1 gives it back by destroying its tap after the
/// ramp. Whether that was done.
pub(super) fn set(pid: i32, gain: f32) -> bool {
    let Some(api) = api() else { return false };
    let mut taps = taps();
    if gain >= 0.999 {
        let Some(mut tap) = taps.by_pid.remove(&pid) else { return true };
        tap.gain.set(1.0);
        std::thread::sleep(Duration::from_secs_f32(RAMP_SECS * 2.0));
        tap.stop(api);
        if taps.by_pid.is_empty() {
            unlisten(&mut taps);
        }
        return true;
    }
    if let Some(tap) = taps.by_pid.get(&pid) {
        tap.gain.set(gain);
        return true;
    }
    // Never a tap that mutes without the permission: the app would go silent instead of quieter.
    if permission() != Access::Granted {
        return false;
    }
    let (Some(process), Some(output)) = (process_object(pid), output()) else { return false };
    let Some(tap) = Tap::start(api, Of::Process(process), &output, 1.0, gain) else { return false };
    listen(&mut taps, output.device);
    taps.by_pid.insert(pid, tap);
    true
}

/// Asks the person for System Audio Recording: the system asks when a tap first reads. A tap that mutes nothing, read for
/// as long as the person takes to answer, or [`ASKING`].
pub(super) fn ask() {
    let (Some(api), Some(output)) = (api(), output()) else { return };
    if permission() != Access::Undetermined {
        return;
    }
    let Some(mut tap) = Tap::start(api, Of::Nothing, &output, 0.0, 0.0) else { return };
    let since = Instant::now();
    while permission() == Access::Undetermined && since.elapsed() < ASKING {
        std::thread::sleep(Duration::from_millis(250));
    }
    tap.stop(api);
}

/// Destroys what a run that died left: aggregate devices and taps named for this app. Private ones go with their process,
/// but this is what other tap apps learned to do anyway (FineTune's orphaned-tap cleanup).
pub(super) fn clean() {
    let Some(api) = api() else { return };
    for device in list(kAudioObjectSystemObject as u32, kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal)
    {
        if string(device, kAudioDevicePropertyDeviceUID).is_some_and(|uid| uid.starts_with(PREFIX)) {
            // SAFETY: destroying an aggregate device this app made.
            unsafe { AudioHardwareDestroyAggregateDevice(device) };
        }
    }
    for tap in list(kAudioObjectSystemObject as u32, kAudioHardwarePropertyTapList, kAudioObjectPropertyScopeGlobal) {
        // SAFETY: the tap's description, a retained object the property hands over; its name.
        let name =
            get::<*mut CATapDescription, ()>(tap, kAudioTapPropertyDescription, kAudioObjectPropertyScopeGlobal, None)
                .and_then(|raw| unsafe { Retained::from_raw(raw) })
                .map(|d| unsafe { d.name() }.to_string());
        if name.is_some_and(|n| n.starts_with(PREFIX)) {
            // SAFETY: destroying a tap this app made.
            unsafe { (api.destroy)(tap) };
        }
    }
}

/// The selectors that make the taps play to the wrong place or rate: the default output, and the output's rate and life.
fn watched(device: u32) -> [(u32, u32); 3] {
    [
        (kAudioObjectSystemObject as u32, kAudioHardwarePropertyDefaultOutputDevice),
        (device, kAudioDevicePropertyNominalSampleRate),
        (device, kAudioDevicePropertyDeviceIsAlive),
    ]
}

/// Called by Core Audio on its own thread: only a note to the rebuilding thread.
unsafe extern "C-unwind" fn changed(
    _object: u32,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    _data: *mut c_void,
) -> i32 {
    if let Some(tx) = REBUILD.get() {
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(());
    }
    0
}

static REBUILD: OnceLock<Mutex<mpsc::Sender<()>>> = OnceLock::new();

fn listen(taps: &mut Taps, device: u32) {
    if taps.listening == Some(device) {
        return;
    }
    unlisten(taps);
    REBUILD.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            while rx.recv().is_ok() {
                // Several changes come at once (a headset switching profile): wait until they stop.
                while rx.recv_timeout(SETTLE).is_ok() {}
                rebuild();
            }
        });
        Mutex::new(tx)
    });
    for (object, selector) in watched(device) {
        let mut at = address(selector, kAudioObjectPropertyScopeGlobal);
        // SAFETY: a listener with no data, removed in `unlisten`.
        unsafe { AudioObjectAddPropertyListener(object, NonNull::from(&mut at), Some(changed), std::ptr::null_mut()) };
    }
    taps.listening = Some(device);
}

fn unlisten(taps: &mut Taps) {
    let Some(device) = taps.listening.take() else { return };
    for (object, selector) in watched(device) {
        let mut at = address(selector, kAudioObjectPropertyScopeGlobal);
        // SAFETY: removing the listener `listen` added.
        unsafe {
            AudioObjectRemovePropertyListener(object, NonNull::from(&mut at), Some(changed), std::ptr::null_mut())
        };
    }
}

/// Makes every tap again for the output as it is now, at the gain it had: the old one silenced first, the new one ramping
/// up from silence (a short, clean dip; a Bluetooth headset renegotiates its link anyway). A tap that cannot be made again
/// is given back: its app plays as it did.
fn rebuild() {
    let Some(api) = api() else { return };
    let mut taps = taps();
    let Some(now) = output() else { return };
    if taps.by_pid.values().all(|t| t.output == now) {
        return;
    }
    for (pid, mut old) in std::mem::take(&mut taps.by_pid) {
        let target = f32::from_bits(old.gain.target.load(Ordering::Relaxed));
        old.gain.silence();
        std::thread::sleep(Duration::from_millis(10));
        let new = process_object(pid).and_then(|process| Tap::start(api, Of::Process(process), &now, 0.0, target));
        old.stop(api);
        if let Some(new) = new {
            taps.by_pid.insert(pid, new);
        }
    }
    if taps.by_pid.is_empty() {
        unlisten(&mut taps);
    } else {
        listen(&mut taps, now.device);
    }
}
