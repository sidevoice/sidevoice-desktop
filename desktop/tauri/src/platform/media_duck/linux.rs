//! MPRIS over the session bus: every player that offers `org.mpris.MediaPlayer2.Player` (browsers, music players,
//! video players) pauses and resumes, and lowers where it takes `Volume`.
use super::{Lowering, Mode, Os, Pausing, Player};
use zbus::blocking::{fdo::DBusProxy, proxy::Builder, Connection, Proxy};
use zbus::names::BusName;
use zbus::proxy::CacheProperties;

const PREFIX: &str = "org.mpris.MediaPlayer2.";

pub(super) struct Native;

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

impl Os for Native {
    const LOWERING: Lowering = Lowering::PerApp;
    const PAUSING: Pausing = Pausing::MediaControls;

    fn players(_: Mode) -> Vec<Player> {
        let Ok(connection) = Connection::session() else { return Vec::new() };
        let Ok(bus) = DBusProxy::new(&connection) else { return Vec::new() };
        let Ok(names) = bus.list_names() else { return Vec::new() };
        let ours = std::process::id();
        names
            .iter()
            .map(|name| name.as_str())
            .filter(|name| name.starts_with(PREFIX))
            // The room's own webview may offer a player for its page: never the app's own sound.
            .filter(|name| {
                BusName::try_from(*name).ok().and_then(|n| bus.get_connection_unix_process_id(n).ok()) != Some(ours)
            })
            .filter_map(|name| {
                let proxy = player_proxy(&connection, name)?;
                let status: String = proxy.get_property("PlaybackStatus").ok()?;
                Some(Player {
                    key: name.to_owned(),
                    playing: status == "Playing",
                    volume: proxy.get_property::<f64>("Volume").ok().map(|v| v as f32),
                    pausable: proxy.get_property::<bool>("CanPause").unwrap_or(false),
                })
            })
            .collect()
    }

    fn set_volume(key: &str, volume: f32) -> bool {
        let Ok(connection) = Connection::session() else { return false };
        player_proxy(&connection, key).is_some_and(|p| p.set_property("Volume", f64::from(volume)).is_ok())
    }

    fn pause(key: &str) -> bool {
        call(key, "Pause")
    }

    fn play(key: &str) -> bool {
        call(key, "Play")
    }
}

fn call(key: &str, method: &str) -> bool {
    let Ok(connection) = Connection::session() else { return false };
    player_proxy(&connection, key).is_some_and(|p| p.call_method(method, &()).is_ok())
}
