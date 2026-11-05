//! macOS 14.4 and later, with System Audio Recording allowed, lowers any app that plays, one by one, through Core Audio
//! process taps (`macos_taps.rs`): the output volume and this app's own voice are never touched. Without them (macOS 13 to
//! 14.3, or the permission refused or not yet asked), lowering is Music's and Spotify's own `sound volume`, never the
//! device's. Pausing is Music and Spotify, through Apple events (the person allows each once: Privacy & Security ->
//! Automation); Pause reaches nothing else, and lowers nothing instead (operator, 2026-11-04). No media
//! key is ever sent: it toggles, and during a call it goes to this app (the Now Playing one). See docs/MEDIA_DUCK.md.
use super::{Lowering, Mode, Os, Pausing};
use objc2_core_services::{
    typeApplicationBundleID, typeWildCard, AECreateDesc, AEDesc, AEDeterminePermissionToAutomateTarget, AEDisposeDesc,
};
use sidevoice_desktop_core::media_duck::{with_fresh, Access, Player, Seen, State};
use std::process::Command;
use std::sync::Mutex;

#[path = "ours.rs"]
mod ours;
#[path = "macos_taps.rs"]
mod taps;

pub(super) struct Native;

/// The key prefix of a process lowered through a tap, then its pid.
const TAP: &str = "tap:";
/// The key prefix of an app that takes Apple events, then its name.
const APP: &str = "app:";
/// The apps whose playback can be asked, lowered and paused through Apple events, with their bundle ids (a helper of theirs
/// carries a bundle id that starts with it).
const APPS: [(&str, &str); 2] = [("Music", "com.apple.Music"), ("Spotify", "com.spotify.client")];
/// `errAEEventNotPermitted`: the person refused.
const DENIED: i32 = -1743;
/// `procNotFound`: the app is not open.
const NOT_OPEN: i32 = -600;
/// `errAEEventWouldRequireUserConsent`: not asked yet.
const UNDETERMINED: i32 = -1744;

/// What the system says of this app sending Apple events to `bundle`: 0 yes, `DENIED`, or another code (-1744: not asked
/// yet, -600: the app is not open). Asks the person only when `ask` is set.
fn permission(bundle: &str, ask: bool) -> i32 {
    let mut target = AEDesc { descriptorType: 0, dataHandle: std::ptr::null_mut() };
    // SAFETY: an address descriptor made from the bundle id's bytes, used for one call and disposed.
    unsafe {
        if AECreateDesc(typeApplicationBundleID, bundle.as_ptr().cast(), bundle.len() as _, &mut target) != 0 {
            return -1;
        }
        let status = AEDeterminePermissionToAutomateTarget(&target, typeWildCard, typeWildCard, ask);
        AEDisposeDesc(&mut target);
        status
    }
}

/// Runs an AppleScript line against `app` only if it is open (telling an app that is closed would open it): its output.
fn script(app: &str, command: &str) -> Option<String> {
    let script = format!("if application \"{app}\" is running then tell application \"{app}\" to {command}");
    Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// What the app named `app` says of its playback and its own volume (0 to 1): `Gone` when it is not open, `Unknown` when
/// the person has not allowed it or it does not answer (a script that asked anyway would put a dialog in the middle of a
/// call).
fn app_player(app: &str, bundle: &str) -> Result<Player, Seen> {
    match permission(bundle, false) {
        0 => {}
        NOT_OPEN => return Err(Seen::Gone),
        _ => return Err(Seen::Unknown),
    }
    let out = script(app, "return (player state as string) & \",\" & (sound volume as string)").ok_or(Seen::Unknown)?;
    if out.is_empty() {
        return Err(Seen::Gone);
    }
    let (state, volume) = out.split_once(',').unwrap_or((&out, ""));
    let state = match state {
        "playing" => State::Playing,
        "paused" => State::Paused,
        _ => State::Stopped,
    };
    Ok(Player {
        key: format!("{APP}{app}"),
        // The app keeps its volume across a restart: a debt is looked for by the app's name then.
        app: app.to_owned(),
        state,
        volume: volume.trim().parse::<f32>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0)),
        pausable: true,
    })
}

/// Whether taps lower here: a macOS that has them, and System Audio Recording allowed.
fn tapping() -> bool {
    taps::supported() && taps::permission() == Access::Granted
}

/// A process that plays, as a player a tap lowers: its volume is the gain it is played at (1 untapped).
fn tap_player(pid: i32) -> Player {
    Player {
        key: format!("{TAP}{pid}"),
        // A tap goes with the process that made it: nothing of it outlives a restart.
        app: String::new(),
        state: State::Playing,
        volume: Some(taps::gain(pid).unwrap_or(1.0)),
        pausable: false,
    }
}

/// The processes that play, each lowered by a tap.
fn tapped_players() -> Vec<Player> {
    taps::playing().into_iter().map(tap_player).collect()
}

impl Os for Native {
    fn lowering() -> Lowering {
        if tapping() {
            Lowering::PerApp
        } else {
            Lowering::SomeApps
        }
    }

    fn pausing() -> Pausing {
        Pausing::SomeApps
    }

    fn access() -> Option<Access> {
        // Each app is allowed on its own, and the system answers only about one that is open (-600 otherwise): each app's
        // last yes or no stands while it is closed. One still to be asked comes first (Allow asks it), then one refused
        // (it is not controlled), then one allowed; none known: `unknown`.
        static LAST: Mutex<[Option<Access>; APPS.len()]> = Mutex::new([None; APPS.len()]);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        for (known, (_, bundle)) in last.iter_mut().zip(APPS) {
            match permission(bundle, false) {
                0 => *known = Some(Access::Granted),
                DENIED => *known = Some(Access::Denied),
                UNDETERMINED => *known = Some(Access::Undetermined),
                // Closed, it cannot be asked: only a yes or a no is kept for it.
                NOT_OPEN if *known == Some(Access::Undetermined) => *known = None,
                _ => {}
            }
        }
        let has = |a: Access| last.contains(&Some(a));
        Some(
            [Access::Undetermined, Access::Denied, Access::Granted]
                .into_iter()
                .find(|&a| has(a))
                .unwrap_or(Access::Unknown),
        )
    }

    fn ask() {
        // The system asks about an app that is open; one that is closed cannot be asked.
        for (_, bundle) in APPS {
            permission(bundle, true);
        }
    }

    fn capture() -> Option<Access> {
        if !taps::supported() {
            return None;
        }
        let own = taps::permission();
        // This process answers as it was when it started: where it says no or not yet, a fresh one says whether the person
        // has allowed it since, and the app then has to be restarted (never a tap from that answer: `tapping`).
        Some(match own {
            Access::Undetermined | Access::Denied => with_fresh(own, taps::fresh(taps::FRESH_READ)),
            _ => own,
        })
    }

    fn ask_capture() {
        if taps::supported() {
            taps::ask();
        }
    }

    /// Privacy & Security -> Screen & System Audio Recording (verify the anchor on 14.x and 15.x).
    const CAPTURE_SETTINGS_URL: Option<&'static str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");

    /// Privacy & Security -> Automation.
    const AUTOMATION_SETTINGS_URL: Option<&'static str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Automation");

    fn clean() {
        if taps::supported() {
            taps::clean();
        }
    }

    fn players(mode: Mode) -> Vec<Player> {
        match mode {
            Mode::Lower if tapping() => tapped_players(),
            // Without taps, Music's and Spotify's own volume; in Pause, those two alone.
            Mode::Lower | Mode::Pause => APPS.iter().filter_map(|(app, bundle)| app_player(app, bundle).ok()).collect(),
            Mode::Off => Vec::new(),
        }
    }

    fn player(key: &str, app: &str) -> Seen {
        if let Some(pid) = key.strip_prefix(TAP).and_then(|pid| pid.parse::<i32>().ok()) {
            return if taps::gain(pid).is_some() || taps::running(pid) {
                Seen::Is(tap_player(pid))
            } else {
                // Gone: what was taken from it goes with it.
                taps::set(pid, 1.0);
                Seen::Gone
            };
        }
        // Its key, or after a restart its app's name.
        let name = key.strip_prefix(APP).filter(|n| !n.is_empty()).unwrap_or(app);
        let Some((name, bundle)) = APPS.iter().find(|(n, _)| *n == name) else { return Seen::Gone };
        app_player(name, bundle).map_or_else(|seen| seen, Seen::Is)
    }

    fn set_volume(key: &str, volume: f32) -> bool {
        if let Some(pid) = key.strip_prefix(TAP).and_then(|pid| pid.parse::<i32>().ok()) {
            return taps::set(pid, volume);
        }
        let Some(app) = key.strip_prefix(APP) else { return false };
        let level = (volume.clamp(0.0, 1.0) * 100.0).round() as u32;
        script(app, &format!("set sound volume to {level}")).is_some()
    }

    fn pause(key: &str) -> bool {
        key.strip_prefix(APP).is_some_and(|app| script(app, "pause").is_some())
    }

    fn play(key: &str) -> bool {
        key.strip_prefix(APP).is_some_and(|app| script(app, "play").is_some())
    }
}
