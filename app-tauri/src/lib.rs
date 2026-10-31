//! The commands every Sidevoice app shares, as one Tauri plugin over sidevoice-app-core's application host
//! (`AppHost`): the voice call's (`voice_*`) and the engine's (`engine_*`). The page invokes them as
//! `plugin:sidevoice-app-tauri|<command>`, and an app grants each to a window with the permission
//! `sidevoice-app-tauri:allow-<command>` (ACL.md says which, to whom, with what arguments and what each tells the page).
//!
//! A command is the caller check, the arguments and one call of the host, which does the work; the host is the app's
//! to build and manage (it brings the hooks only the app has), and the plugin only forwards to it. The events the host
//! tells the page are not an answer of any command: they go to the host's `EventSink`, which for this plugin is [`Pages`],
//! and over the channel the page opened for itself (`voice_events`), to that page and to no other. A page is numbered by
//! the host ([`sidevoice_app_core::app::PageGeneration`]); `voice_events` tells it its number, every voice command carries
//! it back, and the host refuses a number that is not the current page's.
//!
//! Who may call is the app's to say: it hands [`init`] a function that answers, for a webview, whether it is the page the
//! app owns the calls for. The voice commands ask it before they do anything; the engine commands are held by the
//! capability alone (a window the app grants them to), as they were before they were a plugin (ACL.md).

mod acl;
mod engine;
pub mod headset;
mod microphone;
mod observers;
mod pages;
mod pairings;
pub mod platform;
mod voice;

use std::sync::Arc;

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime, Webview};

pub use acl::{inventory, Entry};
pub use observers::{CallObserver, Observers};
pub use pages::Pages;
pub use pairings::{PairingStore, Pairings};

include!("../commands.rs");

/// The plugin's name: what the page invokes it by, and what its permissions are named after.
pub const PLUGIN: &str = "sidevoice-app-tauri";

/// The app's answer to who may call, for a webview and its app.
type Caller<R> = Arc<dyn Fn(&AppHandle<R>, &Webview<R>) -> Result<(), String> + Send + Sync>;

pub(crate) struct Callers<R: Runtime>(Caller<R>);

impl<R: Runtime> Callers<R> {
    /// Whether `webview` may call, as the app says: `Err` with why not.
    pub(crate) fn check(&self, webview: &Webview<R>) -> Result<(), String> {
        (self.0)(webview.app_handle(), webview)
    }
}

/// The plugin. `caller` answers whether a webview is the page the app owns the calls for (its current room window, on
/// its own origin): the voice commands refuse any other with `bad_request`. `pages` is where the page's channel is kept
/// (`voice_events`); the app hands a clone of it to its `AppHost` as the `EventSink`. The app manages its `AppHost` itself.
pub fn init<R: Runtime>(
    caller: impl Fn(&AppHandle<R>, &Webview<R>) -> Result<(), String> + Send + Sync + 'static,
    pages: Pages,
) -> TauriPlugin<R> {
    Builder::new(PLUGIN)
        .invoke_handler(tauri::generate_handler![
            engine::engine_capabilities,
            engine::engine_installed,
            engine::engine_install,
            engine::engine_progress,
            engine::engine_cancel,
            engine::engine_uninstall,
            engine::engine_memory,
            engine::engine_catalogs,
            engine::engine_set_credential,
            engine::engine_has_credential,
            engine::engine_test_credential,
            engine::engine_refresh,
            voice::voice_events,
            voice::voice_create_call,
            voice::voice_route_input,
            voice::voice_dispose_call,
            voice::voice_ready,
            voice::voice_update,
            voice::voice_preload,
            voice::voice_start,
            voice::voice_stop,
            voice::voice_say,
            voice::voice_cancel_say,
            voice::voice_mute,
            voice::voice_cancel_input,
            voice::voice_end_input,
            voice::voice_audio_devices,
            voice::voice_set_audio_devices,
            microphone::microphone,
            observers::bridge_level,
            observers::bridge_playback,
            pairings::pairings_read,
            pairings::pairings_write,
        ])
        .setup(move |app, _| {
            app.manage(Callers::<R>(Arc::new(caller)));
            app.manage(pages);
            Ok(())
        })
        .build()
}

#[cfg(all(test, debug_assertions))]
mod ipc_tests;

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    use super::*;

    /// The app's answer: the window called `room-1` is the page. Counts how often it is asked.
    fn callers(asked: &Arc<AtomicUsize>) -> Callers<MockRuntime> {
        let counted = Arc::clone(asked);
        Callers(Arc::new(move |_app, webview| {
            counted.fetch_add(1, Ordering::SeqCst);
            if webview.label() == "room-1" {
                Ok(())
            } else {
                Err(format!("{} is not the room window", webview.label()))
            }
        }))
    }

    #[test]
    fn the_app_decides_who_the_page_is_and_is_asked_every_time() {
        let app = mock_builder().build(mock_context(noop_assets())).unwrap();
        let asked = Arc::new(AtomicUsize::new(0));
        let callers = callers(&asked);
        let window = |label: &str| {
            WebviewWindowBuilder::new(&app, label, WebviewUrl::default()).build().unwrap().as_ref().clone()
        };
        let (room, card, stale) = (window("room-1"), window("call-controls"), window("room-0"));
        assert_eq!(callers.check(&room), Ok(()));
        assert_eq!(callers.check(&card), Err("call-controls is not the room window".into()));
        assert_eq!(callers.check(&stale), Err("room-0 is not the room window".into()));
        assert_eq!(callers.check(&room), Ok(()), "an answer is not remembered");
        assert_eq!(asked.load(Ordering::SeqCst), 4);
    }

    /// The body of the command called `name` in `source`: from its signature to the next command.
    fn body<'a>(source: &'a str, name: &str) -> &'a str {
        let from = source.find(&format!("fn {name}<")).unwrap_or_else(|| panic!("no command {name}"));
        let rest = &source[from..];
        let open = rest.find("{\n").unwrap();
        let end = rest[open..].find("\n}\n").map_or(rest.len(), |end| open + end);
        &rest[open..end]
    }

    #[test]
    fn every_command_but_the_engines_asks_the_caller_first_and_no_engine_command_does() {
        // A checkout on Windows may have CRLF line ends: the sources are read with LF.
        let lf = |source: &str| source.replace("\r\n", "\n");
        let asking = [
            lf(include_str!("voice.rs")),
            lf(include_str!("microphone.rs")),
            lf(include_str!("observers.rs")),
            lf(include_str!("pairings.rs")),
        ];
        let engine = lf(include_str!("engine.rs"));
        for command in COMMANDS {
            if !command.starts_with("engine_") {
                let source = asking.iter().find(|source| source.contains(&format!("fn {command}<"))).expect(command);
                let first = body(source, command).lines().nth(1).unwrap_or_default().trim();
                assert!(
                    first.starts_with("allowed(&callers, &webview)") || first.starts_with("callers.check(&webview)"),
                    "{command} asks the caller before anything else: {first}"
                );
            } else {
                let at = engine.find(&format!("fn {command}(")).unwrap_or_else(|| panic!("no command {command}"));
                let signature = &engine[at..engine[at..].find('{').map_or(engine.len(), |end| at + end)];
                assert!(!signature.contains("Webview") && !signature.contains("Callers"), "{command}: {signature}");
            }
        }
    }
}
