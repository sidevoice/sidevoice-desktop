//! Who may capture media in, and navigate, the room window.
//!
//! wry's WKWebView delegate grants every camera and microphone request from any page when the app
//! sets no handler. The room window sets one: the microphone is granted without a WebKit prompt to
//! the configured room origin only; the camera, and every other origin (sign-in pages, anything a
//! link leads to), are denied. macOS still asks the person once for the app itself (TCC, with
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

/// `page_origin` is the origin of the page currently loaded in the room window, if any.
pub fn decide(capture: Capture, page_origin: Option<&str>, room_origin: &str) -> Decision {
    match capture {
        Capture::Microphone if page_origin == Some(room_origin) => Decision::Allow,
        _ => Decision::Deny,
    }
}

/// Which navigations the room window allows, by URL scheme. wry asks for every frame, so the
/// schemes iframes use (sign-in widgets, the room's own blobs) must pass; `file:` and the app's
/// own schemes must not, so a remote page cannot open local files or the app's bundled pages.
pub fn navigation_allowed(scheme: &str) -> bool {
    matches!(scheme, "https" | "http" | "about" | "blob" | "data")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: &str = "https://voice.example.com";

    #[test]
    fn microphone_only_for_the_room() {
        assert_eq!(decide(Capture::Microphone, Some(ROOM), ROOM), Decision::Allow);
        assert_eq!(decide(Capture::Microphone, Some("https://accounts.google.com"), ROOM), Decision::Deny);
        assert_eq!(decide(Capture::Microphone, Some("https://voice.example.com:8443"), ROOM), Decision::Deny);
        assert_eq!(decide(Capture::Microphone, Some("http://voice.example.com"), ROOM), Decision::Deny);
        assert_eq!(decide(Capture::Microphone, None, ROOM), Decision::Deny);
    }

    #[test]
    fn navigation_schemes() {
        for ok in ["https", "http", "about", "blob", "data"] {
            assert!(navigation_allowed(ok), "{ok}");
        }
        for no in ["file", "tauri", "ipc", "asset", "javascript", "ftp", ""] {
            assert!(!navigation_allowed(no), "{no}");
        }
    }

    #[test]
    fn never_the_camera_or_anything_else() {
        assert_eq!(decide(Capture::Camera, Some(ROOM), ROOM), Decision::Deny);
        assert_eq!(decide(Capture::Other, Some(ROOM), ROOM), Decision::Deny);
    }
}
