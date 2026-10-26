use super::{polling, ButtonSample, PointerEvents, Sink, State, Subscription};
use tauri::AppHandle;
pub struct NativePointerEvents;
impl PointerEvents for NativePointerEvents {
    fn watch(app: &AppHandle, label: &'static str, margin: f64, state: State, sink: Sink) -> Option<Subscription> {
        // The page owns the cursor on Windows.
        Some(polling(app, label, margin, state, sink, Some(button_samples), None))
    }
}
// Every button is sampled, including after an earlier one was pressed. GetAsyncKeyState's high bit is "down now" and
// its low bit "pressed since the last call", which is only a hint.
fn button_samples() -> [ButtonSample; 3] {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].map(|key| {
        // SAFETY: a plain query of the key state, with no pointers.
        let state = unsafe { GetAsyncKeyState(key as i32) } as u16;
        (state & 0x8000 != 0, state & 1 != 0)
    })
}
