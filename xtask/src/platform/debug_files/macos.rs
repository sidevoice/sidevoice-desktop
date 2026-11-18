//! macOS: `dsymutil` makes the `.dSYM` from the binary as built, then the shipped binary is `strip -S`.

use std::path::Path;

use crate::util::{ensure, Cmd};
use crate::Result;

pub struct Native;

impl super::Os for Native {
    fn split(_build: &Path, binary: &Path, out: &Path) -> Result<()> {
        let dsym = out.join("sidevoice-desktop.dSYM");
        Cmd::new("dsymutil").arg(binary).arg("-o").arg(&dsym).run()?;
        let dwarf = dsym.join("Contents/Resources/DWARF").join(binary.file_name().ok_or("the binary has no name")?);
        ensure(dwarf.is_file(), "dsymutil made no DWARF: the build kept no debug information (profile.release)")?;
        Cmd::new("strip").arg("-S").arg(binary).run()?;
        // Sentry matches a crash's module to its dSYM by the UUID in the executable, which `strip -S` keeps.
        let (shipped, debug) = (uuid(binary)?, uuid(&dwarf)?);
        ensure(!shipped.is_empty() && shipped == debug, "the dSYM's UUID is not the shipped binary's")
    }
}

/// `dwarfdump --uuid`'s lines for `path`, without the path (`UUID: <uuid> (arm64)`).
fn uuid(path: &Path) -> Result<String> {
    let text = Cmd::new("dwarfdump").arg("--uuid").arg(path).stdout()?;
    Ok(text
        .lines()
        .filter_map(|line| line.strip_prefix("UUID: "))
        .filter_map(|rest| rest.split(" (").next())
        .collect::<Vec<_>>()
        .join(" "))
}
