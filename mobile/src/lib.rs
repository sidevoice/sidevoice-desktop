//! The Sidevoice mobile app: a native shell around the Sidevoice web interface the repository pins (`web.pin.json`,
//! unpacked into `ui/` by `npm run web`, which every Tauri build and dev run starts with).
//!
//! The window shows only the app's own pages. It has **no bridge**: nothing is injected, so the interface runs as it
//! does in a browser (it pairs with a machine on the local network and keeps its pairings in the webview's storage),
//! and no page captures anything: the app will own the microphone, as on the desktop.
mod platform;

pub use platform::entry::run;
use platform::origin::APP_ORIGIN;
use tauri::{webview::PermissionResponse, Url, WebviewUrl, WebviewWindowBuilder};

const BUNDLED_INTERFACE: &str = "voice/index.html";

/// An URL's origin as a page on it reports it. The WHATWG origin of a custom scheme (`tauri://localhost`) is opaque
/// (`null`), but the webviews report the app's pages as `scheme://host`, and so does this.
fn origin_of(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
        (Some(host), None) => format!("{}://{host}", url.scheme()),
        _ => "null".to_owned(),
    }
}

/// Which navigations the window allows: the app's own pages, and the schemes a page's own frames and objects use.
/// Never another site, a local file or another app scheme.
fn navigation_allowed(url: &Url, app_origin: &str) -> bool {
    origin_of(url) == app_origin || matches!(url.scheme(), "about" | "blob" | "data")
}

/// The app: one window, on the bundled interface.
fn run_app() {
    tauri::Builder::default()
        .plugin(sidevoice_mobile_plugin::init())
        .setup(|app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App(BUNDLED_INTERFACE.into()))
                // Only the app's own pages: the interface never navigates away.
                .on_navigation(|url| navigation_allowed(url, APP_ORIGIN))
                // No page captures anything (the microphone, the camera, the screen): the webview would grant every
                // request when the app sets no handler.
                .on_permission_request(|_webview, _kind| PermissionResponse::Deny)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Sidevoice mobile app");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(url: &str, app_origin: &str) -> bool {
        navigation_allowed(&Url::parse(url).unwrap(), app_origin)
    }

    #[test]
    fn navigation_stays_on_the_app_s_pages() {
        for app in ["tauri://localhost", "http://tauri.localhost"] {
            let page = format!("{app}/voice/index.html");
            assert!(allowed(&page, app), "{page}");
            for url in ["about:blank", "blob:null/1b2c", "data:text/html,x"] {
                assert!(allowed(url, app), "{url}");
            }
        }
        for url in [
            "https://evil.example/",
            "http://127.0.0.1:8768/",
            "http://192.168.1.20:8768/",
            "file:///etc/passwd",
            "tauri://other/",
            "http://tauri.localhost:8080/",
            "ipc://localhost/",
            "javascript:alert(1)",
        ] {
            assert!(
                !allowed(url, "tauri://localhost") && !allowed(url, "http://tauri.localhost"),
                "{url}"
            );
        }
        // Each OS has one app origin: the other one's pages are not its own.
        assert!(!allowed("http://tauri.localhost/", "tauri://localhost"));
        assert!(!allowed("tauri://localhost/", "http://tauri.localhost"));
    }
}
