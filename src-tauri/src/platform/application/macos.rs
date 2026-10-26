use super::Os;
use tauri::AppHandle;

pub(super) struct Native;

impl Os for Native {
    fn prepare(app: &AppHandle) -> tauri::Result<()> {
        // Ask for a regular Dock application before creating any window or panel (#117).
        app.set_activation_policy(tauri::ActivationPolicy::Regular)?;
        app.set_dock_visibility(true)
    }

    fn activate(_app: &AppHandle) {
        use objc2::{class, msg_send, runtime::AnyObject, sel};
        // SAFETY: main thread after setup; older macOS versions do not implement activate.
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let responds: bool = msg_send![app, respondsToSelector: sel!(activate)];
            if responds {
                let _: () = msg_send![app, activate];
            }
        }
    }

    fn handle_event(app: &AppHandle, event: &tauri::RunEvent) {
        if let tauri::RunEvent::Reopen { has_visible_windows: false, .. } = event {
            crate::show_room(app);
        }
    }

    fn report(app: &AppHandle) -> Option<String> {
        use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
        use objc2_foundation::NSString;
        // NSApplicationActivationPolicy: 0 Regular (Dock tile, Cmd+Tab), 1 Accessory, 2 Prohibited.
        // SAFETY: main thread (setup and `run_on_main_thread`); public AppKit getters.
        let (active, policy, running_policy, hidden, owns_menu_bar, bundle) = unsafe {
            let ns_app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let running: *mut AnyObject = msg_send![class!(NSRunningApplication), currentApplication];
            let active: bool = msg_send![ns_app, isActive];
            let policy: isize = msg_send![ns_app, activationPolicy];
            let running_policy: isize = msg_send![running, activationPolicy];
            let hidden: bool = msg_send![ns_app, isHidden];
            let owns_menu_bar: bool = msg_send![running, ownsMenuBar];
            let bundle: Option<Retained<NSString>> = msg_send![running, bundleIdentifier];
            (active, policy, running_policy, hidden, owns_menu_bar, bundle.map(|b| b.to_string()))
        };
        let room = crate::room_window(app).map_or_else(
            || "none".to_owned(),
            |w| {
                // NSWindowOcclusionStateVisible: set while any part of the window is on screen.
                let onscreen = w.ns_window().is_ok_and(|ns| unsafe {
                    let state: usize = msg_send![ns as *mut AnyObject, occlusionState];
                    state & (1 << 1) != 0
                });
                format!("visible={},onscreen={onscreen}", w.is_visible().unwrap_or(false))
            },
        );
        let regular = policy == 0;
        let bundle = bundle.as_deref().unwrap_or("none");
        Some(format!(
            "app_active={active} regular={regular} policy={policy} running_policy={running_policy} hidden={hidden} \
             owns_menu_bar={owns_menu_bar} bundle={bundle} room={room}"
        ))
    }
}
