//! Other apps' media while the microphone is open in a call (docs/MEDIA_DUCK.md). One worker thread follows the call and the
//! person's choice, because every system call it makes may block; it is the only place that remembers what was taken.

use crate::platform::media_duck as platform;
use serde_json::{json, Value};
use sidevoice_desktop_core::media_duck::{wanted, Ducker, Mode, Status};
use std::sync::{mpsc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, Webview};

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

impl MediaDuck {
    pub fn start(mode: Mode) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        std::thread::spawn(move || {
            let (mut mode, mut joined, mut mic_enabled) = (mode, false, false);
            let mut ducker = Ducker::default();
            while let Ok(msg) = rx.recv() {
                let mut done = None;
                match msg {
                    Msg::Mode(m) => mode = m,
                    Msg::Call { joined: j, mic_enabled: m } => (joined, mic_enabled) = (j, m),
                    Msg::Release(ack) => {
                        (joined, mic_enabled) = (false, false);
                        done = Some(ack);
                    }
                }
                ducker.follow(wanted(mode, joined, mic_enabled), platform::players, platform::act);
                if let Some(ack) = done {
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

/// The app is going: what it took is given back before the process ends (a force quit cannot).
pub fn release(app: &AppHandle) {
    if let Some(duck) = app.try_state::<MediaDuck>() {
        let (ack, done) = mpsc::channel();
        duck.send(Msg::Release(ack));
        let _ = done.recv_timeout(Duration::from_secs(3));
    }
}

fn status(mode: Mode) -> Status {
    let (lower, pause) = platform::reach();
    Status { mode, lower, pause }
}

/// `host.mediaDuck`'s one command: `action` `get`, or `set` with `mode` (`lower`, `pause` or `off`).
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
        _ => Err(refuse("bad_request", format!("not a media duck action: {action}"))),
    }
}
