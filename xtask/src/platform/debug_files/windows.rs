//! Windows: the linker's PDB, kept out of the installer (the NSIS bundle holds the executable and its resources only).

use std::fs;
use std::path::Path;

use crate::Result;

pub struct Native;

impl super::Os for Native {
    fn split(build: &Path, binary: &Path, out: &Path) -> Result<()> {
        // The PDB is named after the executable's crate, with an underscore, and sits beside it.
        let stem = binary.file_stem().and_then(|stem| stem.to_str()).ok_or("the binary has no name")?;
        let pdb = build.join(format!("{}.pdb", stem.replace('-', "_")));
        fs::copy(&pdb, out.join(pdb.file_name().ok_or("the PDB has no name")?)).map(drop).map_err(|e| {
            format!("{}: {e} (a build that strips has no PDB: profile.release is `strip = \"none\"`)", pdb.display())
        })
    }
}
