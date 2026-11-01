/// The origin Tauri serves the app's own pages from, and so what a node sees as the interface's `Origin`: Android serves
/// them over http (`http://tauri.localhost`, which is also why a page can reach the node's plain-http address without
/// mixed content), iOS from the `tauri` scheme. Both are origins the node accepts without configuration. Only this
/// value differs, so there is no per-OS code.
pub const APP_ORIGIN: &str = if cfg!(target_os = "android") {
    "http://tauri.localhost"
} else {
    "tauri://localhost"
};
