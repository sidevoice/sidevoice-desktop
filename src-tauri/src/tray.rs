//! The menu-bar (tray) icon: call state at a glance, mute, hang up, hide the call controls card, show the window,
//! settings, quit. What it shows comes from [`bridge::tray_view`], in the app's language; what it does goes through
//! the bridge.

use sidevoice_desktop_core::bridge::{self, CallSnapshot, Command, TrayIcon};
use sidevoice_desktop_core::i18n::t;
use std::sync::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

const TRAY_ID: &str = "sidevoice";

// Drawn by scripts/make-icons.mjs (docs/BRAND.md). macOS: template images (black + alpha), which it tints
// for light and dark menu bars. Windows and Linux do not tint tray icons, so the mark comes in the brand's
// two looks, berenjena for light bars and lila for dark ones, and the app picks by the bar's colour.
#[cfg(target_os = "macos")]
mod art {
    use sidevoice_desktop_core::bridge::TrayIcon;
    pub const TEMPLATE: bool = true;
    pub fn bytes(which: TrayIcon) -> &'static [u8] {
        match which {
            TrayIcon::Idle => include_bytes!("../icons/tray/idle.png"),
            TrayIcon::Live => include_bytes!("../icons/tray/live.png"),
            TrayIcon::Muted => include_bytes!("../icons/tray/muted.png"),
        }
    }
}
#[cfg(not(target_os = "macos"))]
mod art {
    use sidevoice_desktop_core::bridge::TrayIcon;
    pub const TEMPLATE: bool = false;
    pub fn bytes(which: TrayIcon) -> &'static [u8] {
        match (bar_is_light(), which) {
            (true, TrayIcon::Idle) => include_bytes!("../icons/tray/light-idle.png"),
            (true, TrayIcon::Live) => include_bytes!("../icons/tray/light-live.png"),
            (true, TrayIcon::Muted) => include_bytes!("../icons/tray/light-muted.png"),
            (false, TrayIcon::Idle) => include_bytes!("../icons/tray/dark-idle.png"),
            (false, TrayIcon::Live) => include_bytes!("../icons/tray/dark-live.png"),
            (false, TrayIcon::Muted) => include_bytes!("../icons/tray/dark-muted.png"),
        }
    }
    /// Windows says whether its taskbar is light (Settings → Personalisation → Colours → "Windows mode").
    /// Read at every icon change, so a switch shows by the next change of call state.
    #[cfg(windows)]
    fn bar_is_light() -> bool {
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
            .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
            .map(|light| light == 1)
            .unwrap_or(false)
    }
    /// Linux panels do not say; most (GNOME's top bar among them) are dark.
    #[cfg(not(windows))]
    fn bar_is_light() -> bool {
        false
    }
}

struct TrayItems {
    status: MenuItem<Wry>,
    mute: MenuItem<Wry>,
    hang_up: MenuItem<Wry>,
    call_controls: MenuItem<Wry>,
    last_icon: Mutex<Option<TrayIcon>>,
}

fn icon(which: TrayIcon) -> tauri::Result<Image<'static>> {
    Image::from_bytes(art::bytes(which))
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

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(initial.icon)?)
        .icon_as_template(art::TEMPLATE)
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
            "settings" => crate::open_settings(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    app.manage(TrayItems { status, mute, hang_up, call_controls, last_icon: Mutex::new(Some(initial.icon)) });
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

    let mut last = items.last_icon.lock().unwrap();
    if *last != Some(view.icon) {
        if let (Some(tray), Ok(image)) = (app.tray_by_id(TRAY_ID), icon(view.icon)) {
            let _ = tray.set_icon(Some(image));
            let _ = tray.set_icon_as_template(art::TEMPLATE);
            let _ = tray.set_tooltip(Some(&view.status));
        }
        *last = Some(view.icon);
    }
}
