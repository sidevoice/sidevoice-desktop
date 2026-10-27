//! The local host's way to the operating system: `sidevoice_platform` (the shared crate of core releases) for
//! everything that differs per OS, and here only what the local host adds on top of it, none of it per OS: the local
//! channel driven from blocking code ([`ipc`]), the package for this computer ([`target`]), and the name the pairing
//! takes ([`computer_name`]).

pub mod computer_name;
pub mod ipc;
pub mod target;
