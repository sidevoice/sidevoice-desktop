//! `dmgbuild` (pinned, in a virtualenv of its own) with a `tiffutil` background, as scripts/dmg-settings.py expects.

use std::path::Path;

use super::DiskImage;
use crate::util::Cmd;
use crate::Result;

pub struct Host;

impl DiskImage for Host {
    fn assemble(
        &self,
        work: &Path,
        app: &Path,
        background: [&Path; 2],
        layout: &Path,
        icon: &Path,
        output: &Path,
    ) -> Result<()> {
        let venv = work.join("dmgbuild");
        Cmd::new("python3").args(["-m", "venv"]).arg(&venv).run()?;
        Cmd::new(venv.join("bin/pip")).args(["install", "--quiet", "dmgbuild==1.6.7"]).run()?;
        // One TIFF with the 1x and 2x drawings: Finder picks the one for the screen.
        let tiff = work.join("background.tiff");
        Cmd::new("tiffutil").arg("-cathidpicheck").args(background).arg("-out").arg(&tiff).run()?;
        Cmd::new(venv.join("bin/dmgbuild"))
            .args(["-s", "scripts/dmg-settings.py"])
            .arg("-D")
            .arg(format!("app={}", app.display()))
            .arg("-D")
            .arg(format!("background={}", tiff.display()))
            .arg("-D")
            .arg(format!("layout={}", layout.display()))
            .arg("-D")
            .arg(format!("icon={}", icon.display()))
            .arg("Sidevoice")
            .arg(output)
            .run()
    }
}
