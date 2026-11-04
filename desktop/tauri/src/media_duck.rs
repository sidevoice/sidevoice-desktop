//! Other apps' media while the microphone is open in a call or the agent is speaking (docs/MEDIA_DUCK.md). One worker
//! thread follows the call and the person's choice, because every system call it makes may block; it is the only place that remembers what was taken, and
//! it writes that down so a crash does not leave other apps lowered or paused for good.

use crate::platform::media_duck as platform;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sidevoice_desktop_core::media_duck::{wanted, Access, Ducker, Hold, Lowering, Mode, Pausing, Status, Touch};
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_opener::OpenerExt;

/// Where what is owed is kept, in the app's data folder.
const OWED: &str = "media-duck.json";

enum Msg {
    Mode(Mode),
    Call {
        joined: bool,
        mic_enabled: bool,
        agent_speaking: bool,
    },
    /// Give everything back now, and say so on the channel.
    Release(mpsc::Sender<()>),
}

/// The worker's mailbox, kept as the app's state.
pub struct MediaDuck(Mutex<mpsc::Sender<Msg>>);

/// What is kept: what is owed, and on which start of the machine it was written.
#[derive(Serialize, Deserialize)]
struct Kept {
    boot: u64,
    owed: Vec<Touch>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The boot times of one start of the machine differ by a second or so between reads.
fn same_boot(a: u64, b: u64) -> bool {
    a.abs_diff(b) <= 5
}

/// What an earlier run left that is still worth giving back (core `Touch::after_restart`).
fn read(file: &Option<PathBuf>) -> Vec<Touch> {
    let kept: Option<Kept> =
        file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok());
    let Some(kept) = kept else { return Vec::new() };
    let (same, now) = (same_boot(kept.boot, platform::boot_time()), now());
    kept.owed.into_iter().filter_map(|t| t.after_restart(same, now)).collect()
}

/// Writes down what is owed, or forgets the file when nothing is: written whole beside it and renamed over it, so a crash
/// in the middle leaves the old file, never half of one.
fn write(file: &Option<PathBuf>, owed: &[Touch]) {
    let Some(file) = file else { return };
    if owed.is_empty() {
        let _ = std::fs::remove_file(file);
        return;
    }
    let kept = Kept { boot: platform::boot_time(), owed: owed.to_vec() };
    let Ok(bytes) = serde_json::to_vec(&kept) else { return };
    let temp = file.with_extension("json.tmp");
    if std::fs::write(&temp, bytes).is_ok() && std::fs::rename(&temp, file).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

impl MediaDuck {
    /// Starts the worker with the person's choice; what an earlier run took and did not give back is given back first.
    pub fn start(mode: Mode, dir: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let file = dir.map(|d| d.join(OWED));
        std::thread::spawn(move || {
            let (mut mode, mut joined, mut mic_enabled, mut agent_speaking) = (mode, false, false, false);
            let mut ducker = Ducker::owing(read(&file));
            let mut hold = Hold::default();
            let mut last_written = Vec::<Touch>::new();
            let started = Instant::now();
            let millis = || started.elapsed().as_millis() as u64;
            // What a run that died left in the system goes first (macOS: its taps), then what it owed is given back, at
            // once rather than at the first call.
            platform::clean();
            ducker.tick(now());
            ducker.settle(platform::player, platform::act);
            write(&file, ducker.owed());
            let mut until: Option<u64> = None;
            loop {
                // Wait for news, or for a held give-back to fall due.
                let first = match until {
                    Some(at) => match rx.recv_timeout(Duration::from_millis(at.saturating_sub(millis()))) {
                        Ok(msg) => Some(msg),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    },
                    None => match rx.recv() {
                        Ok(msg) => Some(msg),
                        Err(_) => break,
                    },
                };
                // What queued up while the last look was blocked is folded into one: only the latest state matters.
                let mut acks = Vec::new();
                for msg in first.into_iter().chain(rx.try_iter()) {
                    match msg {
                        Msg::Mode(m) => mode = m,
                        Msg::Call { joined: j, mic_enabled: m, agent_speaking: s } => {
                            (joined, mic_enabled, agent_speaking) = (j, m, s)
                        }
                        Msg::Release(ack) => {
                            (joined, mic_enabled, agent_speaking) = (false, false, false);
                            acks.push(ack);
                        }
                    }
                }
                // Only the gap between turns is held over: hanging up, quitting or choosing to do nothing gives back now.
                if !joined || mode == Mode::Off {
                    hold.drop_now();
                }
                let want;
                (want, until) = hold.follow(wanted(mode, joined, mic_enabled, agent_speaking), millis());
                // A look only when the call really asks something else of the media: every report of the page comes through
                // here, and a debt that cannot be paid is tried again at a change, not at each of them.
                ducker.tick(now());
                ducker.follow(want, platform::players, platform::player, platform::act);
                if ducker.owed() != last_written.as_slice() {
                    write(&file, ducker.owed());
                    last_written = ducker.owed().to_vec();
                }
                for ack in acks {
                    let _ = ack.send(());
                }
            }
        });
        MediaDuck(Mutex::new(tx))
    }

    fn send(&self, msg: Msg) {
        let _ = self.0.lock().unwrap().send(msg);
    }
}

/// The call changed: whether it is joined, whether the microphone is open in it, and whether the agent is speaking.
pub fn call(app: &AppHandle, joined: bool, mic_enabled: bool, agent_speaking: bool) {
    if let Some(duck) = app.try_state::<MediaDuck>() {
        duck.send(Msg::Call { joined, mic_enabled, agent_speaking });
    }
}

/// The app is going: what it took is given back before the process ends (a force quit cannot; the next start does it).
pub fn release(app: &AppHandle) {
    if let Some(duck) = app.try_state::<MediaDuck>() {
        let (ack, done) = mpsc::channel();
        duck.send(Msg::Release(ack));
        let _ = done.recv_timeout(Duration::from_secs(3));
    }
}

fn status(mode: Mode) -> Status {
    let (lower, pause) = platform::reach();
    Status { mode, lower, pause, access: platform::access(), capture: platform::capture() }
}

fn in_call(app: &AppHandle) -> bool {
    app.state::<crate::AppState>().call.lock().unwrap().joined
}

/// What a choice needs asked of the system: the permission to lower any app (asked first: with it, lowering needs nothing
/// else), then the permission to control the named apps, if the choice still goes through them.
fn ask_for(mode: Mode) {
    if mode == Mode::Off {
        return;
    }
    if platform::capture() == Some(Access::Undetermined) {
        platform::ask_capture();
    }
    let (lower, pause) = platform::reach();
    let through_named = match mode {
        Mode::Lower => lower == Lowering::SomeApps,
        _ => pause == Pausing::SomeApps,
    };
    if through_named && platform::access() == Access::Undetermined {
        platform::ask();
    }
}

/// `host.mediaDuck`'s one command: `action` `get`; `set` with `mode` (`lower`, `pause` or `off`); `allow`, the person's
/// own request to let the app do what the choice needs, which is when the system may ask; or `settings`, the system's page
/// where the person turns back on what they once refused.
#[tauri::command]
pub async fn media_duck(
    app: AppHandle,
    webview: Webview,
    action: String,
    mode: Option<String>,
) -> Result<Status, Value> {
    let refuse = |key: &str, why: String| json!({ "key": key, "message": why });
    crate::room_page(&app, &webview).map_err(|why| refuse("bad_request", why))?;
    match action.as_str() {
        "get" => Ok(status(crate::stored_media_duck(&app))),
        "set" => {
            let mode: Mode = serde_json::from_value(json!(mode))
                .map_err(|_| refuse("bad_request", "mode is lower, pause or off".into()))?;
            crate::change_settings(&app, |s| s.media_duck = mode).map_err(|e| refuse("internal", e))?;
            if let Some(duck) = app.try_state::<MediaDuck>() {
                duck.send(Msg::Mode(mode));
            }
            // Choosing Lower is when the system is asked to let the app lower any app, never in the middle of a call (the
            // row's Allow is the person's own way to ask then).
            if mode == Mode::Lower && !in_call(&app) && platform::capture() == Some(Access::Undetermined) {
                let _ = tauri::async_runtime::spawn_blocking(platform::ask_capture).await;
            }
            Ok(status(mode))
        }
        // The person pressed "Allow": the system may ask now (never on its own in the middle of a call, never at startup).
        "allow" => {
            let mode = crate::stored_media_duck(&app);
            let _ = tauri::async_runtime::spawn_blocking(move || ask_for(mode)).await;
            Ok(status(mode))
        }
        "settings" => {
            let url = platform::settings_url().ok_or_else(|| refuse("unsupported", "no settings page here".into()))?;
            app.opener().open_url(url, None::<&str>).map_err(|e| refuse("internal", e.to_string()))?;
            Ok(status(crate::stored_media_duck(&app)))
        }
        _ => Err(refuse("bad_request", format!("not a media duck action: {action}"))),
    }
}
