//! Where the app keeps its own files, inside the directory Tauri calls the app data directory. On iOS that is the app's
//! folder in Application Support, already the place for them. On Android it is the app's whole data directory
//! (`Context.getDataDir()`, with the preferences, databases and caches the system keeps there), whose `files` is the
//! one for an app's own files (`Context.getFilesDir()`). Only this value differs, so there is no per-OS code.

/// The folder of the app's own files in Tauri's app data directory, where it is not that directory itself.
pub const FILES: Option<&str> = if cfg!(target_os = "android") {
    Some("files")
} else {
    None
};
