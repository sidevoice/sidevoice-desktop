//! The origin Tauri serves an app's own pages from: what a page reports as its `location`, and what a node sees as the
//! interface's `Origin`. Only this value differs by OS (Android and Windows serve `http://tauri.localhost`; macOS, Linux and
//! iOS `tauri://localhost`), so there is no per-OS code.

/// `scheme://host` of the app's own pages.
pub const APP_ORIGIN: &str =
    if cfg!(any(windows, target_os = "android")) { "http://tauri.localhost" } else { "tauri://localhost" };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_origin_is_a_scheme_and_a_host_only() {
        let (scheme, host) = APP_ORIGIN.split_once("://").unwrap();
        assert!(matches!(scheme, "tauri" | "http") && !host.contains(['/', ':']) && host.ends_with("localhost"));
    }
}
