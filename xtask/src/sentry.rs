//! Crash reports' side of a release build: the release and its debug files (dSYM, PDB, ELF, `.dwp`) to Sentry, so that a
//! crash's addresses become function names and lines (docs/CRASH_REPORTS.md). `sentry-cli` does the upload; this only says
//! what and where.
//!
//! Reads `SENTRY_AUTH_TOKEN` (the org secret; never printed, never on a command line). The organisation, the project and the
//! EU region are fixed here: org `twentynine-tech-sl`, project `sidevoice`, `https://de.sentry.io`.

use crate::util::{self, Cmd};
use crate::Result;

const URL: &str = "https://de.sentry.io";
const ORG: &str = "twentynine-tech-sl";
const PROJECT: &str = "sidevoice";
/// The apps' `service.name`: the name a release carries, as the reporter in the app names it.
const SERVICES: [&str; 2] = ["sidevoice-desktop", "sidevoice-mobile"];

/// The release `service` reports itself as: `<service.name>@<version>`.
pub fn release_name(service: &str, version: &str) -> Result<String> {
    SERVICES
        .contains(&service)
        .then(|| format!("{service}@{version}"))
        .ok_or_else(|| format!("{service} is not one of {SERVICES:?}"))
}

fn sentry() -> Result<Cmd> {
    util::required_env("SENTRY_AUTH_TOKEN")?;
    Ok(Cmd::new("sentry-cli")
        .env("SENTRY_URL", URL)
        .env("SENTRY_ORG", ORG)
        .env("SENTRY_PROJECT", PROJECT))
}

/// `sentry upload SERVICE PATH...`: the release `SERVICE@<version>` created, the debug files under each PATH uploaded, the
/// release finalised. Only a release or nightly build runs it (its workflow step is not run for a dev build).
pub fn upload(service: &str, paths: &[&str]) -> Result<()> {
    let release = release_name(service, &util::version()?)?;
    if paths.is_empty() {
        return Err("sentry upload: no path to look for debug files in".into());
    }
    sentry()?.args(["releases", "new", &release]).run()?;
    sentry()?
        .args(["debug-files", "upload", "--wait"])
        .args(paths)
        .run()?;
    sentry()?.args(["releases", "finalize", &release]).run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_is_named_after_its_service() {
        assert_eq!(
            release_name("sidevoice-desktop", "0.9.1").unwrap(),
            "sidevoice-desktop@0.9.1"
        );
        assert!(
            release_name("sidevoice-core", "0.9.1").is_err(),
            "the machines' releases are theirs to upload"
        );
    }
}
