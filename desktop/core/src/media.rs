//! What the room window (the bundled interface) may navigate to. It captures no media: the app runs the voice call
//! (`host.voice`) and owns the microphone, so the room window's permission handler (src/lib.rs) denies every request
//! (wry's delegate would grant them all if the app set none). macOS still asks the person once for the app itself
//! (TCC, with `NSMicrophoneUsageDescription`); that prompt cannot and should not be skipped.

/// Which navigations the room window allows: the app's own pages, and the schemes a page's own frames and
/// objects use. wry asks for every frame. Never another site, a local file or another app scheme: the window
/// only ever shows the bundled interface.
pub fn navigation_allowed(scheme: &str, origin: &str, app_origin: &str) -> bool {
    origin == app_origin || matches!(scheme, "about" | "blob" | "data")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_stays_on_the_app_s_pages() {
        const APP: &str = "tauri://localhost";
        assert!(navigation_allowed("tauri", APP, APP));
        for (scheme, origin) in [("about", "null"), ("blob", "null"), ("data", "null")] {
            assert!(navigation_allowed(scheme, origin, APP), "{scheme}");
        }
        for (scheme, origin) in [
            ("https", "https://evil.example"),
            ("http", "http://127.0.0.1:8768"),
            ("file", "null"),
            ("tauri", "tauri://other"),
            ("ipc", "ipc://localhost"),
            ("javascript", "null"),
        ] {
            assert!(!navigation_allowed(scheme, origin, APP), "{scheme} {origin}");
        }
    }
}
