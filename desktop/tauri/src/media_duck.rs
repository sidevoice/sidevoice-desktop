//! Other apps' media while the microphone is open in a call (docs/MEDIA_DUCK.md). One worker thread follows the call and the
//! person's choice, because every system call it makes may block; it is the only place that remembers what was taken, and
//! it writes that down so a crash does not leave other apps lowered or paused for good.

use crate::platform::media_duck as platform;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sidevoice_desktop_core::media_duck::{wanted, Ducker, Mode, Status, Touch};
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, Webview};

/// Where what is owed is kept, in the app's data folder.
const OWED: &str = "media-duck.json";

enum Msg {
    Mode(Mode),
    Call {
        joined: bool,
        mic_enabled: bool,
    },
    /// Give everything back now, and say so on the channel.
    Release(mpsc::Sender<()>),
}

/// The worker's mailbox, kept as the app's state.
pub struct MediaDuck(Mutex<mpsc::Sender<Msg>>);

/// What is kept: what is owed, and when and on which start of the machine it was written.
#[derive(Serialize, Deserialize)]
struct Kept {
    boot: u64,
    at: u64,
    owed: Vec<Touch>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The boot times of one start of the machine differ by a second or so between reads.
fn same_boot(a: u64, b: u64) -> bool {
    a.abs_diff(b) <= 5
}

/// What an earlier run left that is still worth giving back (core `Touch::outlives`).
fn read(file: &Option<PathBuf>) -> Vec<Touch> {
    let kept: Option<Kept> =
        file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok());
    let Some(kept) = kept else { return Vec::new() };
    let (same, age) = (same_boot(kept.boot, platform::boot_time()), now().saturating_sub(kept.at));
    kept.owed.into_iter().filter(|t| t.outlives(same, age)).collect()
}

/// Writes down what is owed, or forgets the file when nothing is: written whole beside it and renamed over it, so a crash
/// in the middle leaves the old file, never half of one.
fn write(file: &Option<PathBuf>, owed: &[Touch]) {
    let Some(file) = file else { return };
    if owed.is_empty() {
        let _ = std::fs::remove_file(file);
        return;
    }
    let kept = Kept { boot: platform::boot_time(), at: now(), owed: owed.to_vec() };
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
            let (mut mode, mut joined, mut mic_enabled) = (mode, false, false);
            let mut ducker = Ducker::owing(read(&file));
            let mut last_written = Vec::<Touch>::new();
            // What an earlier run left is given back at once, not at the first call.
            ducker.settle(platform::player, platform::act);
            write(&file, ducker.owed());
            while let Ok(first) = rx.recv() {
                // What queued up while the last look was blocked is folded into one: only the latest state matters.
                let mut acks = Vec::new();
                for msg in std::iter::once(first).chain(rx.try_iter()) {
                    match msg {
                        Msg::Mode(m) => mode = m,
                        Msg::Call { joined: j, mic_enabled: m } => (joined, mic_enabled) = (j, m),
                        Msg::Release(ack) => {
                            (joined, mic_enabled) = (false, false);
                            acks.push(ack);
                        }
                    }
                }
                // A look only when the call really asks something else of the media: every report of the page comes through
                // here, and a debt that cannot be paid is tried again at a change, not at each of them.
                ducker.follow(wanted(mode, joined, mic_enabled), platform::players, platform::player, platform::act);
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

/// The call changed: whether it is joined, and whether the microphone is open in it.
pub fn call(app: &AppHandle, joined: bool, mic_enabled: bool) {
    if let Some(duck) = app.try_state::<MediaDuck>() {
        duck.send(Msg::Call { joined, mic_enabled });
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
    Status { mode, lower, pause, access: platform::access() }
}

/// `host.mediaDuck`'s one command: `action` `get`, `set` with `mode` (`lower`, `pause` or `off`), or `allow`: the
/// person's own request to let the app control the apps it needs to see or pause, which is when the system may ask.
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
            Ok(status(mode))
        }
        // The person pressed "Allow": the system may ask now (never in the middle of a call, never at startup).
        "allow" => {
            let _ = tauri::async_runtime::spawn_blocking(platform::ask).await;
            Ok(status(crate::stored_media_duck(&app)))
        }
        _ => Err(refuse("bad_request", format!("not a media duck action: {action}"))),
    }
}
