//! The account running the app. Destructive work (deleting the app's data, the connector's uninstall) happens only
//! in [`home`], and only when `HOME` names the home the OS account record has for this user.

use std::path::PathBuf;

#[cfg(unix)]
#[path = "account/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "account/windows.rs"]
mod imp;

/// The account home: `dirs`' home (which trusts `HOME`, as Tauri does) only when it is the home the OS account
/// record has for this user. Anything else, or no record, is `unsafe-home`: callers do no destructive work in
/// another home.
pub fn home() -> Result<PathBuf, &'static str> {
    accept(dirs::home_dir(), imp::recorded_home())
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
        assert!(imp::recorded_home().is_some_and(|home| home.is_absolute()));
    }
}
