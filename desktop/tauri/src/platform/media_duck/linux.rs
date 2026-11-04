//! Linux: lower each playing stream's volume through the sound server (`pactl`, PulseAudio or PipeWire's Pulse server; so
//! browsers are reached, which MPRIS `Volume` does not), pause and resume through MPRIS on the session bus.
use super::{Lowering, Mode, Os, Pausing};
#[path = "ours.rs"]
mod ours;
use ours::Ours;
use serde_json::Value;
use sidevoice_desktop_core::media_duck::{Player, State};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;
use zbus::blocking::{connection, fdo::DBusProxy, proxy::Builder, Connection, Proxy};
use zbus::names::BusName;
use zbus::proxy::CacheProperties;

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const STREAM: &str = "stream:";
const MPRIS: &str = "mpris:";
/// A player that does not answer in this long is skipped: a call must not wait on one.
const ANSWER: Duration = Duration::from_millis(500);

pub(super) struct Native;

/// One connection to the session bus, kept for the worker's life.
fn connection() -> Option<Connection> {
    static CONNECTION: Mutex<Option<Connection>> = Mutex::new(None);
    let mut kept = CONNECTION.lock().ok()?;
    if kept.is_none() {
        *kept = connection::Builder::session().ok()?.method_timeout(ANSWER).build().ok();
    }
    kept.clone()
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
        state,
        volume: None,
        pausable: proxy.get_property::<bool>("CanPause").unwrap_or(false),
    })
}

/// The sound server's playback streams, as `pactl` lists them; none when it cannot be asked.
fn streams() -> Vec<Value> {
    let out = Command::new("pactl").args(["-f", "json", "list", "sink-inputs"]).output();
    out.ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<Vec<Value>>(&o.stdout).ok())
        .unwrap_or_default()
}

/// A stream as a player: its volume is the mean of its channels'.
fn stream_player(stream: &Value) -> Option<Player> {
    let index = stream.get("index")?.as_u64()?;
    let channels: Vec<f64> =
        stream.get("volume")?.as_object()?.values().filter_map(|c| c.get("value")?.as_f64()).collect();
    if channels.is_empty() {
        return None;
    }
    let corked = stream.get("corked").and_then(Value::as_bool).unwrap_or(false);
    Some(Player {
        key: format!("{STREAM}{index}"),
        state: if corked { State::Paused } else { State::Playing },
        volume: Some((channels.iter().sum::<f64>() / channels.len() as f64 / 65536.0) as f32),
        pausable: false,
    })
}

impl Os for Native {
    const LOWERING: Lowering = Lowering::PerApp;
    const PAUSING: Pausing = Pausing::MediaControls;

    fn players(mode: Mode) -> Vec<Player> {
        let ours = Ours::new();
        match mode {
            Mode::Lower => streams()
                .iter()
                .filter(|s| {
                    let pid = s.pointer("/properties/application.process.id").and_then(Value::as_str);
                    !pid.and_then(|p| p.parse().ok()).is_some_and(|pid| ours.has(pid))
                })
                .filter_map(stream_player)
                .collect(),
            Mode::Pause => {
                let Some(connection) = connection() else { return Vec::new() };
                let Ok(bus) = DBusProxy::new(&connection) else { return Vec::new() };
                let Ok(names) = bus.list_names() else { return Vec::new() };
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

    fn player(key: &str) -> Option<Player> {
        if let Some(index) = key.strip_prefix(STREAM) {
            return streams()
                .iter()
                .find(|s| s.get("index").and_then(Value::as_u64).map(|i| i.to_string()).as_deref() == Some(index))
                .and_then(stream_player);
        }
        mpris_player(&connection()?, key.strip_prefix(MPRIS)?)
    }

    fn set_volume(key: &str, volume: f32) -> bool {
        let Some(index) = key.strip_prefix(STREAM) else { return false };
        let raw = (volume.clamp(0.0, 1.5) * 65536.0).round() as u32;
        Command::new("pactl")
            .args(["set-sink-input-volume", index, &raw.to_string()])
            .output()
            .is_ok_and(|o| o.status.success())
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
