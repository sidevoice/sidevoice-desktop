//! The levels the room page reports during a call, for whoever draws them (the desktop's call controls card): the
//! microphone's and the voice's output. The plugin only checks who reports and bounds the numbers; what is done with them
//! is the app's, through [`CallObserver`].

use std::sync::Arc;

use tauri::{Runtime, State, Webview};

use crate::Callers;

/// What the app does with the levels the room page reports.
pub trait CallObserver: Send + Sync + 'static {
    /// The microphone's level, 0 to 100.
    fn level(&self, level: u8);

    /// The voice's output level, 0 to 1 (0 when it ends), and whose reply it is.
    fn playback(&self, level: f64, thread_id: Option<String>);
}

/// The app's observer, as the plugin's state: the app manages one.
pub struct Observers(pub Arc<dyn CallObserver>);

/// The microphone's level during a call (the bridge script, at most every 80 ms). Same caller rule as the voice commands.
#[tauri::command]
pub fn bridge_level<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    observers: State<'_, Observers>,
    level: f64,
) -> Result<(), String> {
    callers.check(&webview)?;
    observers.0.level(level.clamp(0.0, 100.0).round() as u8);
    Ok(())
}

/// The voice's output level during a call (the bridge script, about every 40 ms while a reply sounds, 0 when it ends) and
/// whose reply it is. Same caller rule as the voice commands.
#[tauri::command]
pub fn bridge_playback<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    observers: State<'_, Observers>,
    level: f64,
    thread_id: Option<String>,
) -> Result<(), String> {
    callers.check(&webview)?;
    let level = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
    observers.0.playback(level, thread_id.filter(|id| id.len() <= 200));
    Ok(())
}
