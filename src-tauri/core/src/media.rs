//! Who may capture media in, and navigate, the room window (the bundled interface).
//!
//! wry's WKWebView delegate grants every camera and microphone request from any page when the app
//! sets no handler. The room window sets one. Where the app runs the voice call itself (`host.voice`, macOS), the
//! microphone is the app's and every page is denied it. Elsewhere the page runs the call, and the microphone is granted
//! without a WebKit prompt to the app's own pages only (the bundled interface). The camera, and anything else, are
//! always denied. macOS still asks the person once for the app itself (TCC, with
//! `NSMicrophoneUsageDescription`); that prompt cannot and should not be skipped.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    Microphone,
    Camera,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

/// `page_origin` is the origin of the page currently loaded in the room window, if any; `page_captures` whether the
/// page runs the voice call (no native `host.voice`).
pub fn decide(capture: Capture, page_origin: Option<&str>, room_origin: &str, page_captures: bool) -> Decision {
    match capture {
        Capture::Microphone if page_captures && page_origin == Some(room_origin) => Decision::Allow,
        _ => Decision::Deny,
    }
}

/// Which navigations the room window allows: the app's own pages, and the schemes a page's own frames and
/// objects use. wry asks for every frame. Never another site, a local file or another app scheme: the window
/// only ever shows the bundled interface.
pub fn navigation_allowed(scheme: &str, origin: &str, app_origin: &str) -> bool {
    origin == app_origin || matches!(scheme, "about" | "blob" | "data")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: &str = "tauri://localhost";

    #[test]
    fn microphone_only_for_the_room_and_only_where_the_page_runs_the_call() {
        assert_eq!(decide(Capture::Microphone, Some(ROOM), ROOM, false), Decision::Deny);
        assert_eq!(decide(Capture::Microphone, Some(ROOM), ROOM, true), Decision::Allow);
        assert_eq!(decide(Capture::Microphone, Some("https://accounts.google.com"), ROOM, true), Decision::Deny);
        assert_eq!(decide(Capture::Microphone, Some("https://voice.example.com:8443"), ROOM, true), Decision::Deny);
        assert_eq!(decide(Capture::Microphone, Some("http://voice.example.com"), ROOM, true), Decision::Deny);
        assert_eq!(decide(Capture::Microphone, None, ROOM, true), Decision::Deny);
    }

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

    #[test]
    fn never_the_camera_or_anything_else() {
        assert_eq!(decide(Capture::Camera, Some(ROOM), ROOM, true), Decision::Deny);
        assert_eq!(decide(Capture::Other, Some(ROOM), ROOM, true), Decision::Deny);
    }
}
