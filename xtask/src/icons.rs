//! `icons`: what the app shows of the mark is what the brand files draw (docs/BRAND.md).

use crate::util::{output, repo, run};
use crate::Result;
use std::process::Command;

/// Everything `npm run icons` writes.
const DRAWN: &[&str] = &["src-tauri/icons", "src-tauri/dmg", "ui/brand"];

pub fn icons() -> Result<()> {
    run(Command::new(npm()).args(["run", "icons"]).current_dir(repo()))?;
    let changed = output(Command::new("git").arg("-C").arg(repo()).args(["status", "--porcelain", "--"]).args(DRAWN))?;
    if changed.trim().is_empty() {
        return Ok(());
    }
    Err(format!("the committed icons are not what the brand files draw; run `npm run icons` and commit:\n{changed}"))
}

/// npm's launcher: a `.cmd` on Windows.
fn npm() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}
