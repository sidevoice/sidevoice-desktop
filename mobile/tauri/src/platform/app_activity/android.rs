//! Android: the activity's lifecycle reaches the app as the window's `Suspended` and `Resumed`.
use std::sync::Arc;

use super::Os;

pub struct Native;

impl Os for Native {
    fn observe(_listener: Arc<dyn Fn(bool) + Send + Sync>) {}

    fn shown_by_event(event: &tauri::RunEvent) -> Option<bool> {
        match event {
            tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::Suspended,
                ..
            } => Some(false),
            tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::Resumed,
                ..
            } => Some(true),
            _ => None,
        }
    }
}
