//! What the call's own surfaces show (docs/MOBILE.md → "The call surface"): the iOS Live Activity (Lock Screen and Dynamic
//! Island) and Android's ongoing call notification, drawn by the app itself rather than by the system's call (CallKit,
//! Telecom: off by default, D-NO-CALLKIT-DEFAULT). One snapshot feeds both, [`CallSurface`], inside the [`CallDetails`] the
//! call's service is started and updated with ([`crate::update_call_service`], on every change).
//!
//! - **The words** are the page's, already in its language: the native side never words the call's states or buttons.
//! - **The creatures** travel by reference: the snapshot names a still (`{id, k}`), and its PNG is written once per id to
//!   the stills folder ([`stills_dir`]) before any snapshot names it, where the widget extension (iOS, the App Group
//!   container) and the notification (Android, the app's cache) read it by the file name the snapshot gives
//!   ([`Still::file`]); the page forgets the ones it no longer shows ([`forget_stills`]).
//! - **What the person does** on a surface (mute, skip, cancel send, hang up, open the conversation list) comes back as
//!   [`CallEvent::Action`](crate::CallEvent::Action), and the app runs it on the page. The audio route is the native
//!   side's own (the system's route, not the page's), so it is not one of them.
//!
//! [`CallDetails`]: crate::CallDetails
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

use crate::platform::call_surface as platform;
use crate::{CallDetails, CallEvent, CallEvents};

/// The snapshot of the call a surface draws, besides the conversation's title and the mute ([`CallDetails`]). Every field
/// has a default, so a page that sends less (or nothing yet) draws an empty, idle call.
///
/// [`CallDetails`]: crate::CallDetails
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CallSurface {
    /// What the call is doing: its tone, its glyph and which layout draws it.
    pub phase: Phase,
    /// Skip is live: a reply is playing.
    pub can_skip: bool,
    /// Cancel send is live: the person's turn can still be taken back.
    pub can_cancel_send: bool,
    /// The conversation's model as the page words it ("Opus 5.5"); empty when it does not say.
    pub model: String,
    /// The machine the conversation runs on; empty when it does not say.
    pub machine: String,
    /// When the working phase began (ms since the epoch), for a native timer while `phase` is `working`. Absent when the
    /// page does not know it: then no time is drawn, never a made-up one.
    pub working_since: Option<f64>,
    /// The current conversation's creature.
    pub still: Option<Still>,
    /// Up to three other conversations of the call, for the stack of faces.
    pub others: Vec<Other>,
    /// The number the stack shows.
    pub conversation_count: u32,
    /// While speaking: the page of the reply the voice is on.
    pub page: Option<SpokenPage>,
    /// The person's words just sent, for the few seconds the "said" moment lasts.
    pub said: Option<String>,
    /// The words, in the page's language.
    pub words: SurfaceWords,
    /// The page's language (a BCP 47 tag, `es`, `zh-Hans`), for what the system words on the surface (iOS: the working
    /// timer on Always-On); empty when the page does not say it, and the device's language is used.
    pub language: String,
}

/// What the call is doing, as a surface draws it (HANDOVER §0: the app's own states, no new ones; "said" is the moment
/// after the person's turn is sent).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    #[default]
    Idle,
    Listening,
    Transcribing,
    Said,
    Working,
    Speaking,
    /// The conversation cannot receive ("No puede recibir").
    Holding,
    Offline,
}

/// A creature's still, by reference: the page's `id` (`<threadId>#<px>`), and its PNG is [`Still::file`] in
/// [`stills_dir`]. `k` is how much of the still the body takes (1 for a creature with no big accessory), for the slots
/// that let an accessory stand out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Still {
    pub id: String,
    #[serde(default = "whole")]
    pub k: f64,
    /// The PNG's file name in [`stills_dir`], filled by the app ([`Still::file_name`]); the page does not send it.
    #[serde(default)]
    pub file: String,
}

fn whole() -> f64 {
    1.0
}

impl Still {
    /// Whether `id` can name a still: not empty, and short enough that its file name is one ([`STILL_ID_LIMIT`]).
    pub fn valid_id(id: &str) -> bool {
        (1..=STILL_ID_LIMIT).contains(&id.len())
    }

    /// The file name of the still `id` in [`stills_dir`]: the id's bytes in hex, so that any id the page gives (its `#`,
    /// a `/`, a `..`) is one plain file name, and two ids are never one file.
    pub fn file_name(id: &str) -> String {
        let hex: String = id.bytes().map(|byte| format!("{byte:02x}")).collect();
        format!("{hex}.png")
    }
}

/// Another conversation of the call, as the stack shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Other {
    pub title: String,
    pub still: Option<Still>,
}

/// The page of a reply the voice is on: about 160 characters, ending in an ellipsis where more follows, and how much of it
/// has been heard (`lit`, in UTF-16 code units of `text`, at a word's start: never ahead of the voice).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpokenPage {
    pub text: String,
    pub lit: u32,
}

/// The words a surface draws, resolved by the page in its language. Empty when the page has not said them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SurfaceWords {
    /// The state line.
    pub state: String,
    /// The offline state's word, always there: a surface the app has stopped updating draws the offline state with it.
    pub offline: String,
    pub mute: String,
    pub unmute: String,
    /// Skip's short label, and its accessible name.
    pub skip: String,
    pub skip_name: String,
    pub cancel_send: String,
    pub hang_up: String,
    /// The audio route's accessible name, with `{route}` for the native side to fill with the output's name.
    pub route: String,
    pub speaker: String,
    /// The stack's accessible name, its count in it.
    pub conversations: String,
}

/// What the person asked for on a surface, which the app runs on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceAction {
    Mute,
    Unmute,
    Skip,
    CancelSend,
    HangUp,
    /// The stack of conversations: the app opens with its conversation list.
    OpenConversations,
}

impl SurfaceAction {
    /// The action named `name` (`skip`, `cancel-send`, …), as a surface's native side asks for it.
    pub fn named(name: &str) -> Option<SurfaceAction> {
        serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
    }

    /// The action a URL the app is opened with asks for (iOS: the Live Activity's links, `sidevoice://conversations`);
    /// none for any other (`sidevoice://call` only brings the app to the front).
    pub fn from_url(url: &str) -> Option<SurfaceAction> {
        let rest = url.strip_prefix(URL_SCHEME)?.strip_prefix("://")?;
        match rest.trim_end_matches('/') {
            "conversations" => Some(SurfaceAction::OpenConversations),
            _ => None,
        }
    }
}

/// The scheme of the app's own links (registered by the iOS app; Android's surfaces use explicit intents).
pub const URL_SCHEME: &str = "sidevoice";

/// Why a still was not kept.
#[derive(Debug)]
pub enum StillError {
    /// The id cannot name a file ([`Still::valid_id`]).
    Id,
    /// Not a PNG of `px` × `px` (at most [`STILL_MAX_PX`]), or larger than [`STILL_LIMIT`].
    Image,
    /// The stills folder holds [`STILLS_CAP`] already.
    Full,
    Io(io::Error),
}

impl std::fmt::Display for StillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StillError::Id => f.write_str("not a still's id"),
            StillError::Image => f.write_str("not a PNG of a still's size"),
            StillError::Full => f.write_str("the stills folder is full"),
            StillError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for StillError {}

/// The longest still id: its file name (twice as long, in hex) stays well under a file system's 255 bytes.
pub const STILL_ID_LIMIT: usize = 120;

/// The largest still kept: a 240 px PNG is far smaller.
pub const STILL_LIMIT: usize = 512 * 1024;

/// The largest still's side, in pixels: the largest slot is 240 px.
pub const STILL_MAX_PX: u32 = 480;

/// What the stills folder may hold in all: the page shows four at a time and forgets the rest.
pub const STILLS_CAP: u64 = 8 * 1024 * 1024;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A line for the system's log, where `log show` reads it (iOS: subsystem `ai.sidevoice`, the category given, public
/// strings): the app's log file is not in the unified log. Nothing on other systems, whose log already is the system's.
pub fn os_log(category: &str, message: &str) {
    platform::os_log(category, message);
}

/// The folder the stills are kept in, by id: in the container the app shares with its widget extension where there is one
/// (iOS, the App Group's), else in the app's cache.
pub fn stills_dir<R: Runtime>(app: &AppHandle<R>) -> io::Result<PathBuf> {
    let shared = platform::shared_container();
    let base = match &shared {
        Some(shared) => shared.clone(),
        None => app.path().app_cache_dir().map_err(io::Error::other)?,
    };
    let dir = base.join(STILLS);
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        // Where the stills go is where the surfaces must read them: a widget extension cannot read the app's cache.
        log::info!(
            target: "stills",
            "call stills: folder {} (shared container: {})",
            dir.display(),
            shared.is_some()
        );
    });
    Ok(dir)
}

const STILLS: &str = "call-stills";

/// Keeps the still `id`, a PNG of `px` × `px`, for the surfaces to read as [`Still::file_name`] in [`stills_dir`]. Written
/// whole or not at all (a temporary file renamed over it), so a surface never reads half of one; refused past
/// [`STILLS_CAP`] in all.
pub fn save_still<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    png: &[u8],
    px: u32,
) -> Result<(), StillError> {
    if let Err(error) = check_still(id, png, px) {
        log::warn!(
            target: "stills",
            "call stills: refused {id} ({} bytes, {px} px): {error}",
            png.len()
        );
        return Err(error);
    }
    let dir = stills_dir(app).map_err(StillError::Io)?;
    fs::create_dir_all(&dir).map_err(StillError::Io)?;
    let name = Still::file_name(id);
    let file = dir.join(&name);
    let others: u64 = fs::read_dir(&dir)
        .map_err(StillError::Io)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path() != file)
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum();
    if others + png.len() as u64 > STILLS_CAP {
        return Err(StillError::Full);
    }
    let partial = dir.join(format!(".{name}.partial"));
    fs::write(&partial, png)
        .and_then(|()| fs::rename(&partial, &file))
        .map_err(StillError::Io)?;
    log::info!(
        target: "stills",
        "call stills: kept {} ({} bytes, {px} px)",
        file.display(),
        png.len()
    );
    Ok(())
}

/// Forgets the stills `ids` (the page shows them no more); an id never kept is nothing.
pub fn forget_stills<R: Runtime>(app: &AppHandle<R>, ids: &[String]) -> io::Result<()> {
    let dir = stills_dir(app)?;
    for id in ids.iter().filter(|id| Still::valid_id(id)) {
        match fs::remove_file(dir.join(Still::file_name(id))) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// Forgets every still (at the app's start: the page sends the ones it shows again).
pub fn clear_stills<R: Runtime>(app: &AppHandle<R>) -> io::Result<()> {
    match fs::remove_dir_all(stills_dir(app)?) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn check_still(id: &str, png: &[u8], px: u32) -> Result<(), StillError> {
    if !Still::valid_id(id) {
        return Err(StillError::Id);
    }
    if png.len() > STILL_LIMIT
        || !(1..=STILL_MAX_PX).contains(&px)
        || png_size(png) != Some((px, px))
    {
        return Err(StillError::Image);
    }
    Ok(())
}

/// A PNG's width and height, from its `IHDR`, when the chunks are those of a PNG: each one's length inside the buffer and
/// its CRC right, `IHDR` first, at least one `IDAT`, `IEND` last and at the very end. The pixels are not decoded.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let mut rest = png.strip_prefix(PNG_SIGNATURE)?;
    let mut size = None;
    let mut data = false;
    loop {
        let length = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        let end = length.checked_add(12)?;
        let chunk = rest.get(..end)?;
        let (kind, body) = (&chunk[4..8], &chunk[8..8 + length]);
        if crc32fast::hash(&chunk[4..8 + length]).to_be_bytes() != chunk[8 + length..] {
            return None;
        }
        match (kind, size) {
            (b"IHDR", None) if length == 13 => {
                let number = |at: usize| u32::from_be_bytes(body[at..at + 4].try_into().unwrap());
                size = Some((number(0), number(4)));
            }
            (_, None) | (b"IHDR", Some(_)) => return None,
            (b"IDAT", _) => data = true,
            (b"IEND", _) => return (data && length == 0 && end == rest.len()).then_some(size?),
            _ => {}
        }
        rest = &rest[end..];
    }
}

/// The call's details as a surface's native side reads them (iOS: `Report` in CallActivityBridge.swift): `title`, `muted`
/// and `surface`, in camelCase.
fn details_json(details: &CallDetails) -> String {
    #[derive(Serialize)]
    struct Details<'a> {
        title: &'a str,
        muted: bool,
        surface: &'a CallSurface,
    }
    serde_json::to_string(&Details {
        title: &details.title,
        muted: details.muted,
        surface: &details.surface,
    })
    .unwrap_or_default()
}

/// Starts the surface the system draws apart from the call's service (iOS: the Live Activity) for the app's own call shown
/// as `details`; what the person asks for on it is reported to `events` as [`CallEvent::Action`].
pub(crate) fn start_surface(details: &CallDetails, events: CallEvents) {
    platform::start(
        &details_json(details),
        Arc::new(move |name| match SurfaceAction::named(name) {
            Some(action) => events(CallEvent::Action { action }),
            None => log::warn!("call surface: no action named {name:?}"),
        }),
    );
}

/// Shows the call as `details` on that surface, if one is started.
pub(crate) fn show_surface(details: &CallDetails) {
    platform::show(&details_json(details));
}

/// Ends that surface, and any an earlier run of the app left.
pub(crate) fn end_surface() {
    platform::end();
}

/// Whether that surface is on the system's screens now, or on its way (see `platform::running`).
pub(crate) fn surface_running() -> bool {
    platform::running()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_s_native_side_reads_the_details_and_names_the_actions() {
        let details = CallDetails {
            title: "Globo".into(),
            muted: true,
            surface: CallSurface {
                phase: Phase::Working,
                ..CallSurface::default()
            },
        };
        let json: serde_json::Value = serde_json::from_str(&details_json(&details)).unwrap();
        assert_eq!(json["title"], "Globo");
        assert_eq!(json["muted"], true);
        assert_eq!(json["surface"]["phase"], "working");
        assert_eq!(json["surface"]["canSkip"], false);
        assert_eq!(
            SurfaceAction::named("cancel-send"),
            Some(SurfaceAction::CancelSend)
        );
        assert_eq!(SurfaceAction::named("hang-up"), Some(SurfaceAction::HangUp));
        assert_eq!(SurfaceAction::named("mute"), Some(SurfaceAction::Mute));
        assert_eq!(
            SurfaceAction::named("route"),
            None,
            "the route is the native side's own"
        );
    }

    /// A chunk of `kind` with `body`, its length and CRC.
    fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut chunk = (body.len() as u32).to_be_bytes().to_vec();
        chunk.extend(kind);
        chunk.extend(body);
        chunk.extend(crc32fast::hash(&chunk[4..]).to_be_bytes());
        chunk
    }

    /// The `IHDR` body of `width` x `height`.
    fn header(width: u32, height: u32) -> Vec<u8> {
        [
            &width.to_be_bytes()[..],
            &height.to_be_bytes(),
            &[8, 6, 0, 0, 0],
        ]
        .concat()
    }

    /// A well-formed PNG of `width` x `height` (its image data is not real pixels: they are not decoded).
    fn png_of(width: u32, height: u32) -> Vec<u8> {
        [
            PNG_SIGNATURE,
            &chunk(b"IHDR", &header(width, height)),
            &chunk(b"IDAT", &[0, 1, 2]),
            &chunk(b"IEND", &[]),
        ]
        .concat()
    }

    #[test]
    fn a_surface_reads_with_defaults_for_what_is_missing() {
        let surface: CallSurface = serde_json::from_str(r#"{"phase":"working"}"#).unwrap();
        assert_eq!(
            surface,
            CallSurface {
                phase: Phase::Working,
                ..CallSurface::default()
            }
        );
        let still: Still = serde_json::from_str(r#"{"id":"abc"}"#).unwrap();
        assert_eq!((still.k, still.file.as_str()), (1.0, ""));
    }

    #[test]
    fn it_reaches_the_native_side_in_camel_case() {
        let surface = CallSurface {
            phase: Phase::Speaking,
            can_cancel_send: true,
            working_since: Some(1.5),
            still: Some(Still {
                id: "a1".into(),
                k: 1.3,
                file: Still::file_name("a1"),
            }),
            ..CallSurface::default()
        };
        let json = serde_json::to_value(&surface).unwrap();
        assert_eq!(json["phase"], "speaking");
        assert_eq!(json["canCancelSend"], true);
        assert_eq!(json["workingSince"], 1.5);
        assert_eq!(json["still"]["k"], 1.3);
        assert_eq!(json["still"]["file"], "6131.png");
        assert_eq!(json["words"]["skipName"], "");
    }

    #[test]
    fn a_still_s_file_is_one_plain_name_whatever_its_id() {
        for id in ["t1#240", "../x", "a/b", &"a".repeat(STILL_ID_LIMIT)] {
            assert!(Still::valid_id(id), "{id}");
        }
        for id in ["", &"a".repeat(STILL_ID_LIMIT + 1)] {
            assert!(!Still::valid_id(id), "{id}");
        }
        assert_eq!(Still::file_name("t1#240"), "743123323430.png");
        assert_eq!(Still::file_name("../x"), "2e2e2f78.png");
        assert_ne!(Still::file_name("ab"), Still::file_name("a b"));
    }

    #[test]
    fn a_still_is_a_png_of_its_size() {
        let png = png_of(240, 240);
        assert!(matches!(check_still("", &png, 240), Err(StillError::Id)));
        assert!(check_still("t1#240", &png, 240).is_ok());
        assert!(check_still("t2#108", &png_of(108, 108), 108).is_ok());
        for (png, px) in [
            (png.clone(), 108),
            (png_of(240, 120), 240),
            (png_of(600, 600), 600),
            (png_of(0, 0), 0),
            (PNG_SIGNATURE.to_vec(), 240),
            (b"GIF89a".to_vec(), 240),
            ([&png[..12], b"IDAT", &png[16..]].concat(), 240),
        ] {
            assert!(
                matches!(check_still("a", &png, px), Err(StillError::Image)),
                "{px}"
            );
        }
        let big = [&png[..], &vec![0u8; STILL_LIMIT][..]].concat();
        assert!(matches!(
            check_still("a", &big, 240),
            Err(StillError::Image)
        ));
    }

    #[test]
    fn a_png_is_read_only_when_its_chunks_are_whole() {
        let ihdr = chunk(b"IHDR", &header(240, 240));
        let (idat, iend) = (chunk(b"IDAT", &[0]), chunk(b"IEND", &[]));
        let png = |chunks: &[&[u8]]| [PNG_SIGNATURE, &chunks.concat()].concat();
        assert_eq!(
            png_size(&png(&[&ihdr, &idat, &idat, &iend])),
            Some((240, 240))
        );
        // A forged header with nothing after it, an impossible length, a wrong CRC, no data, no end, bytes after the end.
        let mut forged = PNG_SIGNATURE.to_vec();
        forged.extend(13u32.to_be_bytes());
        forged.extend(b"IHDR");
        forged.extend(header(240, 240));
        assert_eq!(png_size(&forged), None);
        let mut long = ihdr.clone();
        long[..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(png_size(&png(&[&long, &idat, &iend])), None);
        let mut crc = ihdr.clone();
        *crc.last_mut().unwrap() ^= 1;
        assert_eq!(png_size(&png(&[&crc, &idat, &iend])), None);
        assert_eq!(png_size(&png(&[&ihdr, &iend])), None);
        assert_eq!(png_size(&png(&[&ihdr, &idat])), None);
        assert_eq!(png_size(&png(&[&ihdr, &idat, &iend, &idat])), None);
        assert_eq!(png_size(&png(&[&idat, &ihdr, &iend])), None);
        assert_eq!(png_size(&png(&[&ihdr, &ihdr, &idat, &iend])), None);
    }

    #[test]
    fn only_the_conversations_link_is_an_action() {
        assert_eq!(
            SurfaceAction::from_url("sidevoice://conversations"),
            Some(SurfaceAction::OpenConversations)
        );
        assert_eq!(
            SurfaceAction::from_url("sidevoice://conversations/"),
            Some(SurfaceAction::OpenConversations)
        );
        for url in [
            "sidevoice://call",
            "https://conversations",
            "sidevoicex://conversations",
            "",
        ] {
            assert_eq!(SurfaceAction::from_url(url), None, "{url}");
        }
    }

    #[test]
    fn the_actions_are_read_as_the_native_side_names_them() {
        for (text, action) in [
            ("mute", SurfaceAction::Mute),
            ("unmute", SurfaceAction::Unmute),
            ("skip", SurfaceAction::Skip),
            ("cancel-send", SurfaceAction::CancelSend),
            ("hang-up", SurfaceAction::HangUp),
            ("open-conversations", SurfaceAction::OpenConversations),
        ] {
            assert_eq!(
                serde_json::from_str::<SurfaceAction>(&format!("\"{text}\"")).unwrap(),
                action
            );
        }
        assert!(serde_json::from_str::<SurfaceAction>("\"route\"").is_err());
    }
}
