//! Making the files this process and its children create private to the user.

/// While it lives, files this process and its children create are private to this user (umask 077), whatever the
/// person's umask: Tauri's bundler writes the certificate it decodes with plain file creation. The previous
/// creation mask comes back on drop. A host without a creation mask changes nothing.
pub struct PrivateFiles {
    _held: imp::Held,
}

impl PrivateFiles {
    pub fn start() -> PrivateFiles {
        PrivateFiles { _held: imp::Held::start() }
    }
}

#[cfg(unix)]
#[path = "private_files/unix.rs"]
mod imp;
#[cfg(not(unix))]
#[path = "private_files/other.rs"]
mod imp;
