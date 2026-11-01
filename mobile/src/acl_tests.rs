//! Who may call the shared commands in this app (sidevoice-app-tauri's ACL.md): its one capability, against the plugin's
//! inventory.

use std::collections::BTreeSet;

use serde_json::Value;
use sidevoice_app_tauri::inventory;

/// The levels of a call (`bridge_level`, `bridge_playback`) feed the desktop's call controls card; this app has no card
/// and manages no observer of them, so its page is not granted them.
const NOT_GRANTED: [&str; 2] = ["bridge_level", "bridge_playback"];

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_window_is_granted_every_shared_command_it_uses_once_and_nothing_else() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|file| file.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "one capability: {files:?}");
    let capability: Value =
        serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
    assert_eq!(strings(&capability["windows"]), [crate::WINDOW]);
    let (granted, own): (Vec<String>, Vec<String>) = strings(&capability["permissions"])
        .into_iter()
        .partition(|permission| permission.starts_with("sidevoice-app-tauri:"));
    assert_eq!(
        own,
        ["allow-call-state"],
        "the app's own: its report of the call"
    );
    let unique: BTreeSet<&String> = granted.iter().collect();
    assert_eq!(
        unique.len(),
        granted.len(),
        "a permission twice: {granted:?}"
    );
    let expected: Vec<String> = inventory()
        .into_iter()
        .filter(|entry| !NOT_GRANTED.contains(&entry.command.as_str()))
        .map(|entry| entry.permission)
        .collect();
    assert_eq!(
        unique,
        expected.iter().collect::<BTreeSet<_>>(),
        "the window gets the inventory, but the levels"
    );
}
