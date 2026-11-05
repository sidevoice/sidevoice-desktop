//! Linux: lower each playing stream's volume through the sound server (`pactl`, PulseAudio or PipeWire's Pulse server; so
//! browsers are reached, which MPRIS `Volume` does not), pause and resume through MPRIS on the session bus.
use super::{Lowering, Mode, Os, Pausing};
#[path = "ours.rs"]
mod ours;
use ours::Ours;
use serde_json::Value;
use sidevoice_desktop_core::media_duck::{Player, Seen, State};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;
use zbus::blocking::{connection, fdo::DBusProxy, proxy::Builder, Connection, Proxy};
use zbus::names::BusName;
use zbus::proxy::CacheProperties;

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const STREAM: &str = "stream:";
const MPRIS: &str = "mpris:";
/// A player that does not answer in this long is skipped: a call must not wait on one.
const ANSWER: Duration = Duration::from_millis(500);
/// The sound server's tool gets this long.
const PACTL: Duration = Duration::from_secs(2);
/// The first `pactl` that can list as JSON.
const PACTL_JSON: u32 = 16;

pub(super) struct Native;

static CONNECTION: Mutex<Option<Connection>> = Mutex::new(None);

/// One connection to the session bus, kept for the worker's life, and made again after a failure (the bus restarts with a
/// login session).
fn connection() -> Option<Connection> {
    let mut kept = CONNECTION.lock().ok()?;
    if kept.is_none() {
        *kept = connection::Builder::session().ok()?.method_timeout(ANSWER).build().ok();
    }
    kept.clone()
}

fn forget_connection() {
    if let Ok(mut kept) = CONNECTION.lock() {
        *kept = None;
    }
}

fn player_proxy<'a>(connection: &Connection, name: &'a str) -> Option<Proxy<'a>> {
    Builder::new(connection)
        .destination(name)
        .ok()?
        .path("/org/mpris/MediaPlayer2")
        .ok()?
        .interface("org.mpris.MediaPlayer2.Player")
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .ok()
}

fn mpris_player(connection: &Connection, name: &str) -> Option<Player> {
    let proxy = player_proxy(connection, name)?;
    let status: String = proxy.get_property("PlaybackStatus").ok()?;
    let state = match status.as_str() {
        "Playing" => State::Playing,
        "Paused" => State::Paused,
        _ => State::Stopped,
    };
    Some(Player {
        key: format!("{MPRIS}{name}"),
        app: String::new(),
        state,
        volume: None,
        pausable: proxy.get_property::<bool>("CanPause").unwrap_or(false),
    })
}

/// `pactl` with its words in English (a comma for a decimal point breaks its JSON) and a time to answer; its output, or
/// none when it is missing, fails or is late.
fn pactl(args: &[&str]) -> Option<Vec<u8>> {
    let mut child = Command::new("pactl")
        .args(args)
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = out.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    match rx.recv_timeout(PACTL) {
        Ok(bytes) => child.wait().ok()?.success().then_some(bytes),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

/// The sound server's playback streams, as `pactl` lists them; none when it cannot be asked.
fn streams() -> Option<Vec<Value>> {
    serde_json::from_slice(&pactl(&["-f", "json", "list", "sink-inputs"])?).ok()
}

/// The app a stream belongs to: the server remembers a volume under its name.
fn app_of(stream: &Value) -> String {
    ["application.name", "application.process.binary"]
        .iter()
        .find_map(|k| stream.pointer(&format!("/properties/{k}")).and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned()
}

fn index_of(stream: &Value) -> Option<u64> {
    stream.get("index")?.as_u64()
}

/// A stream as a player: its volume is the mean of its channels'.
fn stream_player(stream: &Value) -> Option<Player> {
    let channels: Vec<f64> =
        stream.get("volume")?.as_object()?.values().filter_map(|c| c.get("value")?.as_f64()).collect();
    if channels.is_empty() {
        return None;
    }
    let corked = stream.get("corked").and_then(Value::as_bool).unwrap_or(false);
    Some(Player {
        key: format!("{STREAM}{}", index_of(stream)?),
        app: app_of(stream),
        state: if corked { State::Paused } else { State::Playing },
        volume: Some((channels.iter().sum::<f64>() / channels.len() as f64 / 65536.0) as f32),
        pausable: false,
    })
}

impl Os for Native {
    /// Per stream needs a `pactl` that lists as JSON.
    fn lowering() -> Lowering {
        static LOWERING: OnceLock<Lowering> = OnceLock::new();
        *LOWERING.get_or_init(|| {
            let major = pactl(&["--version"])
                .and_then(|out| String::from_utf8(out).ok())
                .and_then(|text| text.split_whitespace().nth(1)?.split('.').next()?.parse::<u32>().ok());
            if major.is_some_and(|m| m >= PACTL_JSON) {
                Lowering::PerApp
            } else {
                Lowering::None
            }
        })
    }

    fn pausing() -> Pausing {
        Pausing::MediaControls
    }

    fn players(mode: Mode) -> Vec<Player> {
        let ours = Ours::new();
        match mode {
            Mode::Lower => streams()
                .unwrap_or_default()
                .iter()
                .filter(|s| {
                    let pid = s.pointer("/properties/application.process.id").and_then(Value::as_str);
                    !pid.and_then(|p| p.parse().ok()).is_some_and(|pid| ours.has(pid))
                })
                .filter_map(stream_player)
                .collect(),
            Mode::Pause => {
                let Some(connection) = connection() else { return Vec::new() };
                let Some(names) = DBusProxy::new(&connection).ok().and_then(|bus| Some((bus.list_names().ok()?, bus)))
                else {
                    forget_connection();
                    return Vec::new();
                };
                let (names, bus) = names;
                names
                    .iter()
                    .map(|name| name.as_str())
                    .filter(|name| name.starts_with(PREFIX))
                    // The room's webview offers a player for its page, from a process of its own: never ours.
                    .filter(|name| {
                        let pid =
                            BusName::try_from(*name).ok().and_then(|n| bus.get_connection_unix_process_id(n).ok());
                        !pid.is_some_and(|pid| ours.has(pid))
                    })
                    .filter_map(|name| mpris_player(&connection, name))
                    .collect()
            }
            Mode::Off => Vec::new(),
        }
    }

    fn player(key: &str, app: &str) -> Seen {
        // From before a restart the stream's index means nothing: only its app finds it.
        if key.is_empty() && !app.is_empty() {
            let Some(all) = streams() else { return Seen::Unknown };
            return all.iter().find(|s| app_of(s) == app).and_then(stream_player).map_or(Seen::Gone, Seen::Is);
        }
        if let Some(index) = key.strip_prefix(STREAM) {
            let Some(all) = streams() else { return Seen::Unknown };
            let same_index = |s: &&Value| index_of(s).is_some_and(|i| i.to_string() == index);
            // The stream itself; else the app's next one, where the server's stored volume for the app lands.
            let found =
                all.iter().find(same_index).or_else(|| all.iter().find(|s| !app.is_empty() && app_of(s) == app));
            return found.and_then(stream_player).map_or(Seen::Gone, Seen::Is);
        }
        let Some(name) = key.strip_prefix(MPRIS) else { return Seen::Gone };
        let Some(connection) = connection() else { return Seen::Unknown };
        if let Some(player) = mpris_player(&connection, name) {
            return Seen::Is(player);
        }
        // Not answering is not leaving: only a name that is off the bus is gone.
        match DBusProxy::new(&connection).ok().and_then(|bus| bus.list_names().ok()) {
            Some(names) if !names.iter().any(|n| n.as_str() == name) => Seen::Gone,
            Some(_) => Seen::Unknown,
            None => {
                forget_connection();
                Seen::Unknown
            }
        }
    }

    fn set_volume(key: &str, volume: f32) -> bool {
        let Some(index) = key.strip_prefix(STREAM) else { return false };
        let raw = (volume.clamp(0.0, 1.5) * 65536.0).round() as u32;
        pactl(&["set-sink-input-volume", index, &raw.to_string()]).is_some()
    }

    fn pause(key: &str) -> bool {
        call(key, "Pause")
    }

    fn play(key: &str) -> bool {
        call(key, "Play")
    }
}

fn call(key: &str, method: &str) -> bool {
    let (Some(connection), Some(name)) = (connection(), key.strip_prefix(MPRIS)) else { return false };
    player_proxy(&connection, name).is_some_and(|p| p.call_method(method, &()).is_ok())
}
