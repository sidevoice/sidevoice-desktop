//! The user's preferred languages, most preferred first (BCP 47 tags), from the maintained `sys-locale` crate.
//! One body for every OS, so this is a plain module: no trait, no per-OS files.

pub fn preferred() -> Vec<String> {
    sys_locale::get_locales().collect()
}
