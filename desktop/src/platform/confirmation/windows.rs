use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::{
    Controls::{
        TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
        TDF_SIZE_TO_CONTENT, TDM_CLICK_BUTTON, TDN_CREATED, TD_WARNING_ICON,
    },
    WindowsAndMessaging::{SendMessageW, IDCANCEL, IDOK},
};

use super::{DialogStart, Os};

pub(super) struct Native;

unsafe extern "system" fn dialog_event(hwnd: HWND, event: u32, _: WPARAM, _: LPARAM, data: isize) -> i32 {
    if event == TDN_CREATED as u32 {
        // SAFETY: TaskDialogIndirect calls this synchronously. Its callback data points to the live Option
        // in show(), and only this notification takes the START token.
        let start = unsafe { &mut *(data as *mut Option<DialogStart>) }.take();
        if start.is_some_and(|start| !start.opened()) {
            // SAFETY: hwnd is this newly created task dialog. IDCANCEL closes it through its own button path.
            unsafe { SendMessageW(hwnd, TDM_CLICK_BUTTON as u32, IDCANCEL as usize, 0) };
        }
    }
    0
}

impl Os for Native {
    fn show(words: (&'static str, &'static str, &'static str, &'static str), start: DialogStart) -> bool {
        let (title, detail, cancel, action) = words;
        let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let (title, detail, cancel, action) = (wide(title), wide(detail), wide(cancel), wide(action));
        let buttons = [
            TASKDIALOG_BUTTON { nButtonID: IDCANCEL, pszButtonText: cancel.as_ptr() },
            TASKDIALOG_BUTTON { nButtonID: IDOK, pszButtonText: action.as_ptr() },
        ];
        let mut start = Some(start);
        let config = TASKDIALOGCONFIG {
            cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
            dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT,
            pszWindowTitle: title.as_ptr(),
            pszContent: detail.as_ptr(),
            Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: TD_WARNING_ICON },
            pButtons: buttons.as_ptr(),
            cButtons: buttons.len() as u32,
            nDefaultButton: IDCANCEL,
            pfCallback: Some(dialog_event),
            lpCallbackData: &mut start as *mut Option<DialogStart> as isize,
            ..Default::default()
        };
        let mut response = IDCANCEL;
        // SAFETY: all UTF-16 strings and button storage live until this modal call returns. Tauri's
        // default Windows app manifest enables Common Controls v6, as TaskDialogIndirect requires.
        let result = unsafe { TaskDialogIndirect(&config, &mut response, std::ptr::null_mut(), std::ptr::null_mut()) };
        result >= 0 && response == IDOK
    }
}
