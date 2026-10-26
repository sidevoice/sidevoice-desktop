//! The menu-bar (tray) icon: call state at a glance, mute, hang up, hide the call controls card, show the window,
//! the room's Settings, quit. What it shows comes from [`bridge::tray_view`], in the app's language; what it does goes through
//! the bridge.

use crate::platform;
use sidevoice_desktop_core::bridge::{self, CallSnapshot, Command, TrayIcon, TrayIconSlot};
use sidevoice_desktop_core::i18n::t;
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

const TRAY_ID: &str = "sidevoice";

struct TrayItems {
    status: MenuItem<Wry>,
    mute: MenuItem<Wry>,
    hang_up: MenuItem<Wry>,
    call_controls: MenuItem<Wry>,
    /// Never held across a tray call (see [`TrayIconSlot`]).
    icon: Mutex<TrayIconSlot>,
}

fn icon(which: TrayIcon) -> tauri::Result<(Image<'static>, bool)> {
    let image = platform::tray::image(which);
    Ok((Image::from_bytes(image.bytes)?, image.template))
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let language = app.state::<crate::AppState>().language;
    let initial = bridge::tray_view(&CallSnapshot::default(), language);
    let status = MenuItem::with_id(app, "status", &initial.status, false, None::<&str>)?;
    let mute = MenuItem::with_id(app, "mute", initial.mute_label, initial.mute_enabled, None::<&str>)?;
    let hang_up =
        MenuItem::with_id(app, "hang-up", t(language, "tray.hang_up"), initial.hang_up_enabled, None::<&str>)?;
    let call_controls =
        MenuItem::with_id(app, "call-controls", t(language, "tray.hide_call_controls"), false, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", t(language, "tray.show"), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", t(language, "tray.settings"), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", t(language, "tray.quit"), true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &mute,
            &hang_up,
            &call_controls,
            &PredefinedMenuItem::separator(app)?,
            &show,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let (image, template) = icon(initial.icon)?;
    let created = TrayIconBuilder::with_id(TRAY_ID)
        .icon(image)
        .icon_as_template(template)
        .tooltip("Sidevoice")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "mute" => crate::send(app, Command::ToggleMute),
            "hang-up" => crate::send(app, Command::HangUp),
            "call-controls" => {
                crate::call_controls::toggle_hidden(app);
                let call = app.state::<crate::AppState>().call.lock().unwrap().clone();
                update(app, &call);
            }
            "show" => crate::show_room(app),
            "settings" => crate::show_settings(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app);
    if let Err(error) = created {
        platform::tray::creation_failed(error)?;
        return Ok(());
    }

    app.manage(TrayItems {
        status,
        mute,
        hang_up,
        call_controls,
        icon: Mutex::new(TrayIconSlot::showing(initial.icon)),
    });
    Ok(())
}

pub fn update(app: &AppHandle, snapshot: &CallSnapshot) {
    let Some(items) = app.try_state::<TrayItems>() else { return };
    let language = app.state::<crate::AppState>().language;
    let view = bridge::tray_view(snapshot, language);
    let _ = items.status.set_text(&view.status);
    let _ = items.mute.set_text(view.mute_label);
    let _ = items.mute.set_enabled(view.mute_enabled);
    let _ = items.hang_up.set_enabled(view.hang_up_enabled);
    // Hiding the card lasts for the call that is on: only offered during one.
    let hidden = crate::call_controls::hidden_for_call(app);
    let label = if hidden { "tray.show_call_controls" } else { "tray.hide_call_controls" };
    let _ = items.call_controls.set_text(t(language, label));
    let _ = items.call_controls.set_enabled(snapshot.joined);

    // Decided under the lock, applied without it, on the main thread: a tray call from another thread waits for the main
    // thread, which takes this lock (`bridge_state`), so holding it across the call could deadlock (#170). A decision
    // overtaken by a later one is skipped, so the last icon decided is the one shown.
    let Some(decision) = items.icon.lock().unwrap().decide(view.icon) else { return };
    let (a, which, status) = (app.clone(), view.icon, view.status);
    let _ = app.run_on_main_thread(move || {
        let Some(items) = a.try_state::<TrayItems>() else { return };
        if !items.icon.lock().unwrap().is_latest(decision) {
            return;
        }
        if let (Some(tray), Ok((image, template))) = (a.tray_by_id(TRAY_ID), icon(which)) {
            let _ = tray.set_icon(Some(image));
            let _ = tray.set_icon_as_template(template);
            let _ = tray.set_tooltip(Some(&status));
        }
    });
}
