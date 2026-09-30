//! The menu-bar (tray) icon: call state at a glance, mute, hang up, show the window, settings, quit.
//! What it shows comes from [`bridge::tray_view`]; what it does goes through the bridge.

use sidevoice_desktop_core::bridge::{self, CallSnapshot, Command, TrayIcon};
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

const TRAY_ID: &str = "sidevoice";

// Template images (black + alpha): macOS tints them for light/dark menu bars.
const ICON_IDLE: &[u8] = include_bytes!("../icons/tray/idle.png");
const ICON_LIVE: &[u8] = include_bytes!("../icons/tray/live.png");
const ICON_MUTED: &[u8] = include_bytes!("../icons/tray/muted.png");

struct TrayItems {
    status: MenuItem<Wry>,
    mute: MenuItem<Wry>,
    hang_up: MenuItem<Wry>,
    last_icon: Mutex<Option<TrayIcon>>,
}

fn icon(which: TrayIcon) -> tauri::Result<Image<'static>> {
    Image::from_bytes(match which {
        TrayIcon::Idle => ICON_IDLE,
        TrayIcon::Live => ICON_LIVE,
        TrayIcon::Muted => ICON_MUTED,
    })
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let initial = bridge::tray_view(&CallSnapshot::default());
    let status = MenuItem::with_id(app, "status", &initial.status, false, None::<&str>)?;
    let mute = MenuItem::with_id(app, "mute", initial.mute_label, initial.mute_enabled, None::<&str>)?;
    let hang_up = MenuItem::with_id(app, "hang-up", "Colgar", initial.hang_up_enabled, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Mostrar Sidevoice", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Ajustes…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Salir de Sidevoice", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &mute,
            &hang_up,
            &PredefinedMenuItem::separator(app)?,
            &show,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(initial.icon)?)
        .icon_as_template(true)
        .tooltip("Sidevoice")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "mute" => crate::send(app, Command::ToggleMute),
            "hang-up" => crate::send(app, Command::HangUp),
            "show" => crate::show_room(app),
            "settings" => crate::open_settings(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    app.manage(TrayItems { status, mute, hang_up, last_icon: Mutex::new(Some(initial.icon)) });
    Ok(())
}

pub fn update(app: &AppHandle, snapshot: &CallSnapshot) {
    let Some(items) = app.try_state::<TrayItems>() else { return };
    let view = bridge::tray_view(snapshot);
    let _ = items.status.set_text(&view.status);
    let _ = items.mute.set_text(view.mute_label);
    let _ = items.mute.set_enabled(view.mute_enabled);
    let _ = items.hang_up.set_enabled(view.hang_up_enabled);

    let mut last = items.last_icon.lock().unwrap();
    if *last != Some(view.icon) {
        if let (Some(tray), Ok(image)) = (app.tray_by_id(TRAY_ID), icon(view.icon)) {
            let _ = tray.set_icon(Some(image));
            let _ = tray.set_icon_as_template(true);
            let _ = tray.set_tooltip(Some(&view.status));
        }
        *last = Some(view.icon);
    }
}
