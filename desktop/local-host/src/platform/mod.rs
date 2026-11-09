//! The local host's way to the operating system: `sidevoice_platform` (the shared crate of core releases) for
//! everything that differs per OS, and here only what the local host adds on top of it: the package for this computer
//! ([`target`]), the name the pairing takes ([`computer_name`]), and whether this OS's temporary directory is often moved
//! somewhere shared ([`staging`], one value per OS, no behaviour).

pub mod computer_name;
pub mod staging;
pub mod target;
