use super::{polling, ButtonEdges, PointerEvents, Sink, State, Subscription};
use tauri::AppHandle;
pub struct NativePointerEvents;
impl PointerEvents for NativePointerEvents {
    fn watch(app: &AppHandle, label: &'static str, margin: f64, state: State, sink: Sink) -> Option<Subscription> {
        let mut buttons = ButtonEdges::default();
        Some(polling(app, label, margin, state, sink, move |inside, state| {
            let _ = state.hand; // The page owns the cursor on Windows.
            buttons.pressed(button_samples()) && inside == Some(false)
        }))
    }
}
// Poll every button, including after an earlier one was pressed. GetAsyncKeyState's low bit is only a hint:
// another process may consume it. Fast clicks between samples cannot be guaranteed by this backend.
fn button_samples() -> [u16; 3] {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].map(|key| unsafe { GetAsyncKeyState(key as i32) } as u16)
}
