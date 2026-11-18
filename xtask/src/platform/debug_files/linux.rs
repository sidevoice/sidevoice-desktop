//! Linux: the unstripped binary is the debug file, the shipped one is `objcopy --strip-debug` of it.

use std::fs;
use std::path::Path;

use object::read::elf::ElfFile64;
use object::Object;

use crate::util::{ensure, read_bytes, Cmd};
use crate::Result;

pub struct Native;

impl super::Os for Native {
    fn split(_build: &Path, binary: &Path, out: &Path) -> Result<()> {
        let debug = out.join(binary.file_name().ok_or("the binary has no name")?).with_extension("debug");
        fs::copy(binary, &debug).map_err(|e| format!("{}: {e}", binary.display()))?;
        let (lines, id) = facts(&debug)?;
        ensure(
            lines,
            "the release binary has no line tables: profile.release is `debug = \"line-tables-only\"`, `strip = \"none\"`",
        )?;
        let id = id.ok_or(
            "the release binary has no build id: desktop/.cargo/config.toml links Linux with `--build-id=sha1`",
        )?;
        // `objcopy IN OUT`, which GNU's and LLVM's both take.
        let stripped = binary.with_extension("stripped");
        Cmd::new("objcopy").arg("--strip-debug").arg(binary).arg(&stripped).run()?;
        fs::rename(&stripped, binary).map_err(|e| format!("{}: {e}", binary.display()))?;
        let (lines, shipped) = facts(binary)?;
        ensure(
            !lines && shipped.as_ref() == Some(&id),
            "the shipped binary must have no line tables and the debug file's build id",
        )
    }
}

/// Whether an ELF file has line tables, and its build id.
fn facts(path: &Path) -> Result<(bool, Option<Vec<u8>>)> {
    let data = read_bytes(path)?;
    let file =
        ElfFile64::<object::Endianness>::parse(data.as_slice()).map_err(|e| format!("{}: {e}", path.display()))?;
    let id = file.build_id().map_err(|e| format!("{}: {e}", path.display()))?.map(<[u8]>::to_vec);
    Ok((file.section_by_name(".debug_line").is_some(), id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::debug_files::Os;

    /// With a binary that has line tables and a build id (what the app's build links), the split keeps both in the debug file
    /// and leaves the shipped binary with the id and without the line tables.
    #[test]
    fn the_debug_file_keeps_the_line_tables_and_the_shipped_binary_loses_them_and_keeps_the_id() {
        let exe = std::env::current_exe().unwrap();
        let objcopy = Cmd::new("objcopy").arg("--version").attempt().is_ok_and(|(ok, _)| ok);
        let (lines, id) = facts(&exe).unwrap();
        if !(objcopy && lines && id.is_some()) {
            return;
        }
        let work = tempfile::tempdir().unwrap();
        let (binary, out) = (work.path().join("sidevoice-desktop"), work.path().join("out"));
        fs::create_dir(&out).unwrap();
        fs::copy(&exe, &binary).unwrap();
        Native::split(work.path(), &binary, &out).unwrap();
        let debug = out.join("sidevoice-desktop.debug");
        assert_eq!(facts(&debug).unwrap(), (true, id.clone()));
        assert_eq!(facts(&binary).unwrap(), (false, id));
        assert!(fs::metadata(&binary).unwrap().len() < fs::metadata(&debug).unwrap().len(), "smaller");
    }

    /// The test binary itself is an ELF with line tables and, with the link flags of `desktop/.cargo/config.toml` (which the
    /// xtask's own build does not get), possibly no build id: a binary without one is refused.
    #[test]
    fn a_binary_without_debug_information_or_a_build_id_is_refused_not_uploaded_empty() {
        let work = tempfile::tempdir().unwrap();
        let (binary, out) = (work.path().join("sidevoice-desktop"), work.path().join("out"));
        fs::create_dir(&out).unwrap();
        // Not an ELF at all.
        fs::write(&binary, b"not a binary").unwrap();
        assert!(Native::split(work.path(), &binary, &out).is_err());
        // An ELF the profile stripped: nothing to upload.
        let exe = std::env::current_exe().unwrap();
        fs::copy(&exe, &binary).unwrap();
        if Cmd::new("objcopy").arg("--version").attempt().is_ok_and(|(ok, _)| ok) {
            let bare = work.path().join("bare");
            Cmd::new("objcopy").arg("--strip-debug").arg(&binary).arg(&bare).run().unwrap();
            assert!(Native::split(work.path(), &bare, &out).is_err(), "no line tables: refused");
        }
    }
}
