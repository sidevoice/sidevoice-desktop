//! Making the files this process and its children create private to the user.

/// A host's way to keep new files private. While the value lives, files this process and its children create are
/// private to this user (umask 077), whatever the person's umask: Tauri's bundler writes the certificate it decodes
/// with plain file creation. The previous creation mask comes back on drop. A host without a creation mask changes
/// nothing.
pub trait PrivateCreation {
    /// Starts holding private creation; dropping the value ends it.
    fn start() -> Self;
}

#[cfg(unix)]
#[path = "private_files/unix.rs"]
mod imp;
#[cfg(not(unix))]
#[path = "private_files/other.rs"]
mod imp;

pub use imp::PrivateFiles;
