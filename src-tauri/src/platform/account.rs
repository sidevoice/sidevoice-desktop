//! The account running the app. Destructive work (deleting the app's data, the connector's uninstall) happens only
//! in [`home`], and only when `HOME` names the home the OS account record has for this user.

use std::io;
use std::path::{Path, PathBuf};

#[cfg_attr(unix, path = "account/unix.rs")]
#[cfg_attr(windows, path = "account/windows.rs")]
mod imp;
use imp::Native;

/// The account home: `dirs`' home (which trusts `HOME`, as Tauri does) only when it is the home the OS account
/// record has for this user. Anything else, or no record, is `unsafe-home`: callers do no destructive work in
/// another home.
pub fn home() -> Result<PathBuf, &'static str> {
    accept(dirs::home_dir(), Native::recorded_home())
}

/// Whether this account could take the entry `child` out of the directory `dir` and put something else under its
/// name, now or after first changing `dir`'s access (`dir` a directory, `child` a name directly in it). Asked of every
/// directory from the filesystem root down to the home before destructive work there: where the answer is `false`
/// all the way down, the account cannot swap its home or anything above it for a link. A failed inspection is an
/// error, never `false`.
pub fn may_replace(dir: &Path, child: &Path) -> io::Result<bool> {
    Native::may_replace(dir, child)
}

/// What differs per OS: where the account record puts the home, and how this account's rights on a directory are
/// read.
trait Os {
    /// The home the OS account record has for this user (never `HOME`), if any.
    fn recorded_home() -> Option<PathBuf>;

    /// As [`may_replace`].
    fn may_replace(dir: &Path, child: &Path) -> io::Result<bool>;
}

fn accept(home: Option<PathBuf>, recorded: Option<PathBuf>) -> Result<PathBuf, &'static str> {
    match (home, recorded) {
        (Some(home), Some(recorded)) if home == recorded => Ok(home),
        _ => Err("unsafe-home"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_recorded_home_is_accepted() {
        let (home, alternate) = (PathBuf::from("/accounts/ana"), PathBuf::from("/accounts/ana/restored"));
        assert_eq!(accept(Some(home.clone()), Some(home.clone())), Ok(home.clone()));
        assert_eq!(accept(Some(alternate), Some(home.clone())), Err("unsafe-home"));
        assert_eq!(accept(None, Some(home.clone())), Err("unsafe-home"));
        assert_eq!(accept(Some(home), None), Err("unsafe-home"));
    }

    #[test]
    fn every_host_records_an_absolute_home() {
        assert!(Native::recorded_home().is_some_and(|home| home.is_absolute()));
    }
}
