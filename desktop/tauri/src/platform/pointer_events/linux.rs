use super::{ButtonSample, Os};
use crate::platform::desktop_geometry;
use gtk::gdk::{self, ModifierType};
use gtk::prelude::*;

pub struct Native;

/// Linux: the page owns its cursor on X11, and outside clicks are seen by polling the pointer's button state;
/// Wayland cannot observe the pointer globally, so the page's own events are used.
impl Os for Native {
    fn available() -> bool {
        desktop_geometry::positions_available()
    }

    // X11 reports only which buttons are down now, so "pressed since the last look" is always `false`: a click that
    // starts and ends between two looks can be missed. Runs on the main thread, where GDK may be used.
    fn button_samples() -> Option<[ButtonSample; 3]> {
        let display = gdk::Display::default()?;
        let pointer = display.default_seat()?.pointer()?;
        let (_, _, _, mask) = display.default_screen().root_window()?.device_position(&pointer);
        // Left, right, middle: the order of the other platforms (GDK numbers the middle button 2 and the right 3).
        Some(
            [ModifierType::BUTTON1_MASK, ModifierType::BUTTON3_MASK, ModifierType::BUTTON2_MASK]
                .map(|button| (mask.contains(button), false)),
        )
    }
}
