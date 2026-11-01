//! Who may call the shared commands in this app (sidevoice-app-tauri's ACL.md): the capabilities, and the answer the app
//! gives the plugin about who is the page.

use std::collections::BTreeSet;

use serde_json::Value;
use sidevoice_app_tauri::inventory;

use super::is_room_page;
use sidevoice_app_tauri::platform::origin::APP_ORIGIN;

/// Every capability file of the app, by name.
fn capabilities() -> Vec<(String, Value)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    let mut files: Vec<(String, Value)> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|file| {
            let path = file.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            (path.file_name().unwrap().to_string_lossy().into_owned(), serde_json::from_str(&text).unwrap())
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|item| item.as_str().unwrap().to_owned()).collect()
}

#[test]
fn the_room_window_is_granted_every_shared_command_once_and_no_other_window_any() {
    let shared: Vec<String> = inventory().into_iter().map(|entry| entry.permission).collect();
    let mut seen_room = false;
    for (name, capability) in capabilities() {
        let granted: Vec<String> =
            strings(&capability["permissions"]).into_iter().filter(|p| p.starts_with("sidevoice-app-tauri:")).collect();
        if name == "room.json" {
            seen_room = true;
            assert_eq!(strings(&capability["windows"]), ["room-*"]);
            let unique: BTreeSet<&String> = granted.iter().collect();
            assert_eq!(unique.len(), granted.len(), "a permission twice: {granted:?}");
            assert_eq!(unique, shared.iter().collect::<BTreeSet<_>>(), "the room window gets exactly the inventory");
        } else {
            assert!(granted.is_empty(), "{name} grants a shared command: {granted:?}");
        }
        // Never a wildcard or the plugin's default set, which would grant more than the inventory says.
        for permission in strings(&capability["permissions"]) {
            assert!(
                !permission.starts_with("sidevoice-app-tauri:") || permission.contains(":allow-"),
                "{name}: {permission}"
            );
        }
    }
    assert!(seen_room);
}

#[test]
fn the_call_controls_card_is_granted_only_its_own_commands() {
    let (_, card) = capabilities().into_iter().find(|(name, _)| name == "call-controls.json").unwrap();
    assert_eq!(strings(&card["windows"]), ["call-controls"]);
    for permission in strings(&card["permissions"]) {
        assert!(permission.starts_with("allow-call-controls-"), "{permission}");
    }
}

#[test]
fn only_the_current_room_window_on_the_apps_own_origin_is_the_page() {
    assert_eq!(is_room_page(Some("room-2"), "room-2", Some(APP_ORIGIN)), Ok(()));
    // A room window that was replaced, the card, a window made by a page, and no room at all.
    for label in ["room-1", "call-controls", "main", ""] {
        assert_eq!(is_room_page(Some("room-2"), label, Some(APP_ORIGIN)), Err("not the room window".into()), "{label}");
    }
    assert_eq!(is_room_page(None, "room-2", Some(APP_ORIGIN)), Err("not the room window".into()));
    // The current room window showing anything but the app's own page: a site, the other scheme, no URL at all.
    for origin in [Some("https://example.com"), Some("http://tauri.localhost.evil.test"), Some("null"), Some(""), None]
    {
        assert_eq!(is_room_page(Some("room-2"), "room-2", origin), Err("not the app's own page".into()), "{origin:?}");
    }
    let other = if APP_ORIGIN == "tauri://localhost" { "http://tauri.localhost" } else { "tauri://localhost" };
    assert!(
        is_room_page(Some("room-2"), "room-2", Some(other)).is_err(),
        "the other platform's origin is not this one's"
    );
}

/// The bridge script is told whether the host owns the system's media controls, as the literal it reads.
#[test]
fn the_bridge_script_receives_the_hosts_media_support() {
    let available = sidevoice_app_tauri::platform::media_controls::available();
    let script = sidevoice_desktop_core::bridge::script_for_origin(
        "tauri://localhost",
        false,
        &sidevoice_desktop_core::build_info::BuildInfo::new("dev", "0.0.0", "", ""),
        None,
        &Default::default(),
        false,
        &[],
        available,
        false,
        "linux",
        &sidevoice_desktop_core::bridge::Parts { transport: "TRANSPORT", members: "MEMBERS" },
    );
    let literal = serde_json::to_string(&available).unwrap();
    assert!(script.contains(&format!("factory(window, \"tauri://localhost\", {literal},")));
}

/// The room's script is made of the real parts, the transport the plugin ships, the members the apps share and the
/// desktop's own, and nothing of it is left to fill in.
#[test]
fn the_rooms_script_is_made_of_the_real_parts_with_no_placeholder_left() {
    let parts = sidevoice_desktop_core::bridge::Parts {
        transport: sidevoice_app_tauri::js::TRANSPORT,
        members: sidevoice_app_core::js::MEMBERS,
    };
    let script = sidevoice_desktop_core::bridge::script_for_origin(
        "tauri://localhost",
        true,
        &sidevoice_desktop_core::build_info::BuildInfo::new("nightly", "0.0.0", "", ""),
        Some("Mac de Ana"),
        &Default::default(),
        true,
        &[],
        true,
        true,
        "macos",
        &parts,
    );
    // (`win.__SIDEVOICE_TARGET__` is the web's own global; a placeholder is a string.)
    assert!(!script.contains("\"__SIDEVOICE_"), "a placeholder is left in the script");
    for (part, mark) in [
        ("transport", "plugin:sidevoice-app-tauri|"),
        ("members", "function room("),
        ("desktop", "installDesktopBridge"),
    ] {
        assert_eq!(script.matches(mark).count(), 1, "the {part} part is in the script once");
    }
}
