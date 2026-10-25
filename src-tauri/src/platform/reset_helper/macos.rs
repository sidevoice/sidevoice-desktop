//! Existing macOS helper transaction, entered only after authenticated handoff.
use super::ResetHelper;
use crate::data::{
    account_home, finish_reset_at, finish_uninstall_at, move_to_trash, removable_app_bundle, remove_known,
    remove_model_store, report_path, standard_models, webkit_children, webkit_gone, write_atomic_json,
};
use crate::platform::reset_handoff::{DesktopReset, ResetHandoff};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command};

pub struct DesktopResetHelper;

impl ResetHelper for DesktopResetHelper {
    fn run() {
        let Ok(helper) = DesktopReset::receive() else { return };
        let webkit = webkit_children(helper.plan.parent_pid);
        let (initial, uninstall) = (helper.plan.initial.clone(), helper.plan.uninstall);
        let Ok(lock) = helper.await_exit() else { return };
        let Some(report) = report_path() else { return };
        let Some(initial_failed) = initial.get("failed").and_then(Value::as_array) else { return };
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
        if account_home().as_deref() != Some(home.as_path()) {
            return;
        }
        let mut completed = Vec::new();
        let mut failed = initial_failed.clone();
        let journal = report.with_file_name("journal.json");
        let webkit_safe = webkit.as_ref().is_some_and(|children| webkit_gone(children));
        let paths = [
            ("app-data", home.join("Library/Application Support/dev.sidevoice.desktop")),
            ("models", standard_models(&home)),
            ("webview", home.join("Library/WebKit/dev.sidevoice.desktop")),
            ("cache", home.join("Library/Caches/dev.sidevoice.desktop")),
            ("saved-state", home.join("Library/Saved Application State/dev.sidevoice.desktop.savedState")),
            ("logs", home.join("Library/Logs/dev.sidevoice.desktop")),
        ];
        for (name, path) in paths {
            if name == "webview" && !webkit_safe {
                failed.push(json!({"step": "webview", "key": "reset.process-busy"}));
            } else {
                let removed =
                    if name == "models" { remove_model_store(&path, &home) } else { remove_known(&path, &home) };
                match removed {
                    Ok(()) => completed.push(name),
                    Err(key) => failed.push(json!({"step": name, "key": key})),
                }
            }
            if write_atomic_json(&journal, &json!({"stage":"cleanup", "completed":completed, "failed":failed})).is_err()
            {
                failed.push(json!({"step":"report", "key":"reset.report-failed"}));
                break;
            }
        }
        let domains = Command::new("/usr/bin/defaults").arg("domains").output();
        match domains {
            Ok(output) if output.status.success() => {
                let present =
                    String::from_utf8_lossy(&output.stdout).split(',').any(|d| d.trim() == "dev.sidevoice.desktop");
                if !present
                    || Command::new("/usr/bin/defaults")
                        .args(["delete", "dev.sidevoice.desktop"])
                        .status()
                        .is_ok_and(|s| s.success())
                {
                    completed.push("preferences");
                } else {
                    failed.push(json!({"step": "preferences", "key": "remove"}));
                }
            }
            _ => failed.push(json!({"step": "preferences", "key": "inspect"})),
        }
        let agents = initial.get("agents").cloned().unwrap_or_else(|| json!({}));
        let relaunch = || {
            // Cleanup is over. A new app must acquire this lock independently.
            drop(lock);
            std::env::current_exe()
                .ok()
                .and_then(|exe| Command::new(exe).env("SIDEVOICE_RESET_RELAUNCH", "1").spawn().ok())
                .is_some()
        };
        if uninstall {
            let trash = || removable_app_bundle().and_then(|bundle| move_to_trash(&bundle));
            finish_uninstall_at(&report, &journal, &completed, failed, agents, write_atomic_json, trash, relaunch);
        } else {
            finish_reset_at(&report, &journal, &completed, failed, agents, write_atomic_json, relaunch);
        }
    }
}
