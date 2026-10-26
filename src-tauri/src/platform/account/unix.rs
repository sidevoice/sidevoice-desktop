//! Unix (macOS, Linux): the user database entry of the effective user (`getpwuid_r`), never `HOME`.

use nix::unistd::{Uid, User};
use std::path::PathBuf;

pub(super) fn recorded_home() -> Option<PathBuf> {
    User::from_uid(Uid::effective()).ok().flatten().map(|user| user.dir)
}
