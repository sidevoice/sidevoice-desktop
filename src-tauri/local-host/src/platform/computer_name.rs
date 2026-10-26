//! The kernel hostname used to name the core's local pairing.

use std::ffi::OsStr;

/// The current computer's kernel hostname source.
pub struct SystemComputerName;

/// The name used when pairing the core on this computer.
pub trait ComputerName {
    /// Read the kernel hostname, replacing invalid UTF-8, removing only an exact `.local` suffix,
    /// and using `Sidevoice` when the result is empty. Other whitespace is preserved.
    fn read() -> String {
        normalize(&gethostname::gethostname())
    }
}

impl ComputerName for SystemComputerName {}

fn normalize(raw: &OsStr) -> String {
    let name = raw.to_string_lossy();
    let name = name.strip_suffix(".local").unwrap_or(&name);
    if name.is_empty() {
        "Sidevoice".into()
    } else {
        name.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_computer_name_is_usable_for_a_pairing() {
        let name = SystemComputerName::read();
        assert!(!name.is_empty());
    }

    #[test]
    fn pairing_name_strips_only_the_exact_suffix_and_uses_empty_fallback() {
        assert_eq!(normalize(OsStr::new("desk.local")), "desk");
        assert_eq!(normalize(OsStr::new("desk.local.local")), "desk.local");
        assert_eq!(normalize(OsStr::new("desk.local.")), "desk.local.");
        assert_eq!(normalize(OsStr::new(".local")), "Sidevoice");
        assert_eq!(normalize(OsStr::new("")), "Sidevoice");
        assert_eq!(normalize(OsStr::new(" desk ")), " desk ");
    }

    #[cfg(unix)]
    #[test]
    fn pairing_name_replaces_invalid_hostname_bytes() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(normalize(OsStr::from_bytes(b"desk-\xff.local")), "desk-\u{fffd}");
    }
}
