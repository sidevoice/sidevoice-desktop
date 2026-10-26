use super::{ButtonSample, Native as Observations};

pub struct Native;

/// Windows: outside clicks are seen by polling the buttons; the page owns the cursor.
impl Observations for Native {
    // Every button is sampled, including after an earlier one was pressed. GetAsyncKeyState's high bit is "down now"
    // and its low bit "pressed since the last call", which is only a hint.
    fn button_samples() -> Option<[ButtonSample; 3]> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
        Some([VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].map(|key| {
            // SAFETY: a plain query of the key state, with no pointers.
            let state = unsafe { GetAsyncKeyState(key as i32) } as u16;
            (state & 0x8000 != 0, state & 1 != 0)
        }))
    }
}
