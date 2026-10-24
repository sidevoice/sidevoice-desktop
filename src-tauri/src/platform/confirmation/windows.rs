use tauri::{AppHandle, Runtime};
use windows_sys::Win32::UI::{
    Controls::{
        TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
        TDF_SIZE_TO_CONTENT, TD_WARNING_ICON,
    },
    WindowsAndMessaging::{IDCANCEL, IDOK},
};

use super::{Confirmation, ConfirmationKind};

pub struct NativeConfirmation;

impl Confirmation for NativeConfirmation {
    async fn confirm<R: Runtime>(app: &AppHandle<R>, kind: ConfirmationKind) -> bool {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        if app
            .run_on_main_thread(move || {
                let locale = sys_locale::get_locale().unwrap_or_default();
                let (title, detail, cancel, action) = kind.words(&locale);
                let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
                let (title, detail, cancel, action) = (wide(title), wide(detail), wide(cancel), wide(action));
                let buttons = [
                    TASKDIALOG_BUTTON { nButtonID: IDCANCEL, pszButtonText: cancel.as_ptr() },
                    TASKDIALOG_BUTTON { nButtonID: IDOK, pszButtonText: action.as_ptr() },
                ];
                let config = TASKDIALOGCONFIG {
                    cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
                    dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT,
                    pszWindowTitle: title.as_ptr(),
                    pszContent: detail.as_ptr(),
                    Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: TD_WARNING_ICON },
                    pButtons: buttons.as_ptr(),
                    cButtons: buttons.len() as u32,
                    nDefaultButton: IDCANCEL,
                    ..Default::default()
                };
                let mut response = IDCANCEL;
                // SAFETY: all UTF-16 strings and button storage live until this modal call returns. Tauri's
                // default Windows app manifest enables Common Controls v6, as TaskDialogIndirect requires.
                let result =
                    unsafe { TaskDialogIndirect(&config, &mut response, std::ptr::null_mut(), std::ptr::null_mut()) };
                let _ = sender.send(result >= 0 && response == IDOK);
            })
            .is_err()
        {
            return false;
        }
        tauri::async_runtime::spawn_blocking(move || receiver.recv().unwrap_or(false)).await.unwrap_or(false)
    }
}
