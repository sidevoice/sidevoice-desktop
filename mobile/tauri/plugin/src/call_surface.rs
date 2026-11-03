//! What the call's own surfaces show (docs/MOBILE.md → "The call surface"): the iOS Live Activity (Lock Screen and Dynamic
//! Island) and Android's ongoing call notification, drawn by the app itself rather than by the system's call (CallKit,
//! Telecom: off by default, D-NO-CALLKIT-DEFAULT). One snapshot feeds both, [`CallSurface`], inside the [`CallDetails`] the
//! call's service is started and updated with ([`crate::update_call_service`], on every change).
//!
//! - **The words** are the page's, already in its language: the native side never words the call's states or buttons.
//! - **The creatures** travel by reference: the snapshot names a still (`{id, k}`), and its PNG is written once per id to
//!   the stills folder ([`stills_dir`]), where the widget extension (iOS, the App Group container) and the notification
//!   (Android, the app's own files) read it as `<id>.png`.
//! - **What the person does** on a surface (mute, skip, cancel send, hang up, open the conversation list) comes back as
//!   [`CallEvent::Action`](crate::CallEvent::Action), and the app runs it on the page. The audio route is the native
//!   side's own (the system's route, not the page's), so it is not one of them.
//!
//! [`CallDetails`]: crate::CallDetails
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

use crate::platform::call_surface as platform;

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

/// A creature's still, by reference: its PNG is `<id>.png` in [`stills_dir`]. `k` is how much of the still the body takes
/// (1 for a creature with no big accessory), for the slots that let an accessory stand out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Still {
    pub id: String,
    #[serde(default = "whole")]
    pub k: f64,
}

fn whole() -> f64 {
    1.0
}

impl Still {
    /// Whether `id` can name a still: it becomes a file name, so only letters, digits, `-` and `_`, at most 64 of them.
    pub fn valid_id(id: &str) -> bool {
        (1..=64).contains(&id.len())
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }

    /// The still's file name in [`stills_dir`].
    pub fn file_name(&self) -> String {
        format!("{}.png", self.id)
    }
}

/// Another conversation of the call, as the stack shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Other {
    pub title: String,
    pub still: Option<Still>,
}

/// The page of a reply the voice is on: about 160 characters, ending in an ellipsis where more follows, and how many of
/// its characters have been heard (never ahead of the voice).
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
    /// Not a PNG, or larger than [`STILL_LIMIT`].
    Image,
    Io(io::Error),
}

impl std::fmt::Display for StillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StillError::Id => f.write_str("not a still's id"),
            StillError::Image => f.write_str("not a PNG of a still's size"),
            StillError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for StillError {}

/// The largest still kept: a 240 px PNG is far smaller.
pub const STILL_LIMIT: usize = 512 * 1024;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The folder the stills are kept in, by id: in the container the app shares with its widget extension where there is one
/// (iOS, the App Group's), else in the app's cache.
pub fn stills_dir<R: Runtime>(app: &AppHandle<R>) -> io::Result<PathBuf> {
    let base = match platform::shared_container() {
        Some(shared) => shared,
        None => app.path().app_cache_dir().map_err(io::Error::other)?,
    };
    Ok(base.join(STILLS))
}

const STILLS: &str = "call-stills";

/// Keeps the still `id`, a PNG, for the surfaces to read as `<id>.png` in [`stills_dir`]. Written whole or not at all
/// (a temporary file renamed over it), so a surface never reads half of one.
pub fn save_still<R: Runtime>(app: &AppHandle<R>, id: &str, png: &[u8]) -> Result<(), StillError> {
    check_still(id, png)?;
    let dir = stills_dir(app).map_err(StillError::Io)?;
    fs::create_dir_all(&dir).map_err(StillError::Io)?;
    let file = dir.join(format!("{id}.png"));
    let partial = dir.join(format!(".{id}.png.partial"));
    fs::write(&partial, png)
        .and_then(|()| fs::rename(&partial, &file))
        .map_err(StillError::Io)
}

/// Forgets every still (at the app's start: the page sends the ones it shows again).
pub fn clear_stills<R: Runtime>(app: &AppHandle<R>) -> io::Result<()> {
    match fs::remove_dir_all(stills_dir(app)?) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn check_still(id: &str, png: &[u8]) -> Result<(), StillError> {
    if !Still::valid_id(id) {
        return Err(StillError::Id);
    }
    if png.len() > STILL_LIMIT || !png.starts_with(PNG_SIGNATURE) {
        return Err(StillError::Image);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(still.k, 1.0);
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
            }),
            ..CallSurface::default()
        };
        let json = serde_json::to_value(&surface).unwrap();
        assert_eq!(json["phase"], "speaking");
        assert_eq!(json["canCancelSend"], true);
        assert_eq!(json["workingSince"], 1.5);
        assert_eq!(json["still"]["k"], 1.3);
        assert_eq!(json["words"]["skipName"], "");
    }

    #[test]
    fn a_still_s_id_is_a_plain_file_name() {
        for id in ["a", "thread-1_x", &"a".repeat(64)] {
            assert!(Still::valid_id(id), "{id}");
        }
        for id in ["", "../x", "a/b", "a.png", "ñ", &"a".repeat(65)] {
            assert!(!Still::valid_id(id), "{id}");
        }
        assert!(matches!(
            check_still("../x", PNG_SIGNATURE),
            Err(StillError::Id)
        ));
        assert!(matches!(
            check_still("a", b"GIF89a"),
            Err(StillError::Image)
        ));
        let big = [PNG_SIGNATURE, &vec![0u8; STILL_LIMIT][..]].concat();
        assert!(matches!(check_still("a", &big), Err(StillError::Image)));
        assert!(check_still("a", PNG_SIGNATURE).is_ok());
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
