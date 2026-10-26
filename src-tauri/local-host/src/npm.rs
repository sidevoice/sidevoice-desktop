//! The `sidevoice` package on the npm registry, as the app installs it without Node (docs/LOCAL_HOST.md → Install):
//! which release is the latest, which of its platform packages is this computer's, where that package's tarball is
//! and what its digest must be.
//!
//! 1. `GET https://registry.npmjs.org/sidevoice/latest` → `version`, and `optionalDependencies`: one package per
//!    platform, `@sidevoice/sidevoice-<os>-<cpu>` as npm names the platform (`darwin`, `linux`; `arm64`, `x64`), each at
//!    that same version — what `npx sidevoice@latest` would pick here.
//! 2. `GET https://registry.npmjs.org/@sidevoice/sidevoice-<os>-<cpu>/<version>` → `dist.tarball` and `dist.integrity`
//!    (its SHA-512, base64).
//!
//! Every URL is HTTPS on `registry.npmjs.org`, the tarball's too (at the package's own path there); no redirect is
//! followed, and nothing a document names elsewhere is fetched. The trust is the registry's, over TLS, as for `npx`:
//! the integrity binds the tarball to the document that named it.

use crate::install::{Failure, Step};
use base64::Engine as _;
use serde_json::Value;
use std::io::Read;

/// The registry, the only origin the install talks to.
pub const REGISTRY: &str = "https://registry.npmjs.org";
/// The package `npx sidevoice@latest install` runs.
pub const PACKAGE: &str = "sidevoice";
/// What every platform package's name starts with; the rest is npm's `<os>-<cpu>`.
pub const PLATFORM_PREFIX: &str = "@sidevoice/sidevoice-";
/// A registry document larger than this is not one (the real ones are a few KiB).
const DOCUMENT_LIMIT: u64 = 1024 * 1024;

/// What a GET answered: its status, its `Content-Length` when it said one, and its body.
pub struct Response {
    pub status: u16,
    pub length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

/// The HTTP client the install uses: `GET url`, following no redirect. A transport failure is `install.network`.
/// [`Https`] in the app; the stand-in registry in tests.
pub trait Fetch: Send + Sync {
    fn get(&self, url: &str) -> Result<Response, Failure>;
}

/// The platform package for npm's names of an OS and CPU, if npm has names for them.
pub fn platform_package(os: &str, arch: &str) -> Option<String> {
    let os = match os {
        "macos" => "darwin",
        "linux" => "linux",
        _ => return None,
    };
    let cpu = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => return None,
    };
    Some(format!("{PLATFORM_PREFIX}{os}-{cpu}"))
}

/// The release to install: the latest `sidevoice`'s version, and the platform package to get it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub package: String,
}

/// Where the platform package's tarball is, and the SHA-512 it must have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dist {
    pub tarball: String,
    pub sha512: [u8; 64],
}

pub fn latest_url() -> String {
    format!("{REGISTRY}/{PACKAGE}/latest")
}

pub fn version_url(release: &Release) -> String {
    format!("{REGISTRY}/{}/{}", release.package, release.version)
}

fn manifest(message: impl Into<String>) -> Failure {
    Failure::new("install.authenticity", Step::Download, message).check("manifest")
}

fn no_bundle(message: impl Into<String>) -> Failure {
    Failure::new("install.no-bundle", Step::Download, message)
}

/// The release `GET /sidevoice/latest` names for `package` (this computer's platform package): its `version`, which
/// `optionalDependencies` must list `package` at, exactly. A platform it does not list has no release
/// (`install.no-bundle`); a document that is not one is `install.authenticity` (`manifest`).
pub fn release(latest: &[u8], package: &str) -> Result<Release, Failure> {
    let document: Value = serde_json::from_slice(latest)
        .map_err(|e| manifest(format!("The registry's latest {PACKAGE} is not JSON: {e}")))?;
    if document.get("name").and_then(Value::as_str) != Some(PACKAGE) {
        return Err(manifest(format!("The registry's latest {PACKAGE} names another package.")));
    }
    let version = document
        .get("version")
        .and_then(Value::as_str)
        .filter(|v| crate::host::semver(v))
        .ok_or_else(|| manifest(format!("The registry's latest {PACKAGE} has no version.")))?;
    let listed = document.get("optionalDependencies").and_then(|d| d.get(package));
    match listed.and_then(Value::as_str) {
        Some(at) if at == version => Ok(Release { version: version.to_string(), package: package.to_string() }),
        Some(at) => Err(no_bundle(format!("{PACKAGE} {version} lists {package} at {at:?}, not at {version}."))),
        None => Err(no_bundle(format!("{PACKAGE} {version} has no package for this computer ({package})."))),
    }
}

/// `dist` of `GET /<package>/<version>`: the tarball (HTTPS on the registry, at the package's own path) and the
/// SHA-512 of its integrity. A document for another package or version, or without either, is
/// `install.authenticity` (`manifest`); a tarball anywhere else is `install.authenticity` (`download`).
pub fn dist(document: &[u8], release: &Release) -> Result<Dist, Failure> {
    let document: Value = serde_json::from_slice(document)
        .map_err(|e| manifest(format!("The registry's {} {} is not JSON: {e}", release.package, release.version)))?;
    if document.get("name").and_then(Value::as_str) != Some(release.package.as_str())
        || document.get("version").and_then(Value::as_str) != Some(release.version.as_str())
    {
        return Err(manifest(format!(
            "The registry answered for another package than {} {}.",
            release.package, release.version
        )));
    }
    let dist = document.get("dist");
    let tarball = dist
        .and_then(|d| d.get("tarball"))
        .and_then(Value::as_str)
        .ok_or_else(|| manifest(format!("{} {} names no tarball.", release.package, release.version)))?;
    if !tarball_allowed(tarball, release) {
        return Err(Failure::new(
            "install.authenticity",
            Step::Download,
            format!("{} {} names a tarball outside the registry: {tarball:?}", release.package, release.version),
        )
        .check("download"));
    }
    let sha512 =
        dist.and_then(|d| d.get("integrity")).and_then(Value::as_str).and_then(sha512_of).ok_or_else(|| {
            manifest(format!(
                "{} {} has no SHA-512 integrity to check its tarball by.",
                release.package, release.version
            ))
        })?;
    Ok(Dist { tarball: tarball.to_string(), sha512 })
}

/// Whether `url` is the package's tarball on the registry: `https://registry.npmjs.org/<package>/-/<file>.tgz`, with
/// nothing but plain path characters — no other host, port, user, query, fragment, escape or dot segment.
pub fn tarball_allowed(url: &str, release: &Release) -> bool {
    let Some(file) = url.strip_prefix(&format!("{REGISTRY}/{}/-/", release.package)) else { return false };
    !file.is_empty()
        && file.len() <= 255
        && file.ends_with(".tgz")
        && !file.starts_with('.')
        && file.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

/// The SHA-512 in an SRI `integrity` (`sha512-<base64>`, among others separated by blanks): exactly one, 64 bytes.
pub fn sha512_of(integrity: &str) -> Option<[u8; 64]> {
    let mut found = integrity.split_ascii_whitespace().filter_map(|hash| hash.strip_prefix("sha512-"));
    let encoded = found.next()?;
    if found.next().is_some() {
        return None;
    }
    // An SRI hash may carry options after `?`; npm writes none.
    let encoded = encoded.split('?').next()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    bytes.try_into().ok()
}

/// What a registry status means for the install: 200 goes on; 404 is nothing for this computer; a redirect is not
/// followed (the registry does not send one); anything else is the network's.
pub fn expect_ok(response: &Response, url: &str) -> Result<(), Failure> {
    match response.status {
        200 => Ok(()),
        404 => Err(no_bundle(format!("The registry has nothing at {url}."))),
        300..=399 => Err(Failure::new(
            "install.authenticity",
            Step::Download,
            format!("The registry redirected {url}; the app follows no redirect."),
        )
        .check("download")),
        status => {
            Err(Failure::new("install.network", Step::Download, format!("The registry answered {status} for {url}.")))
        }
    }
}

/// A registry document (at most [`DOCUMENT_LIMIT`]).
fn document(fetch: &dyn Fetch, url: &str) -> Result<Vec<u8>, Failure> {
    debug_assert!(url.starts_with(&format!("{REGISTRY}/")));
    let response = fetch.get(url)?;
    expect_ok(&response, url)?;
    let mut bytes = Vec::new();
    response
        .body
        .take(DOCUMENT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Failure::new("install.network", Step::Download, format!("{url}: {e}")))?;
    if bytes.len() as u64 > DOCUMENT_LIMIT {
        return Err(manifest(format!("{url} is larger than a registry document.")));
    }
    Ok(bytes)
}

/// The latest release for `package` and where its tarball is: the two documents above.
pub fn resolve(fetch: &dyn Fetch, package: &str) -> Result<(Release, Dist), Failure> {
    let release = release(&document(fetch, &latest_url())?, package)?;
    let dist = dist(&document(fetch, &version_url(&release))?, &release)?;
    Ok((release, dist))
}

/// The registry over HTTPS (reqwest's blocking client, rustls with the platform's verifier, the system's proxy): HTTPS
/// only, no redirect, 15 s to connect and 30 s for any one read or write.
#[cfg(feature = "fetch")]
pub struct Https {
    client: reqwest::blocking::Client,
}

#[cfg(feature = "fetch")]
impl Https {
    /// Not on an async runtime's thread: the blocking client runs one of its own.
    pub fn new() -> Result<Https, Failure> {
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("sidevoice-desktop")
            .build()
            .map_err(|e| Failure::new("install.network", Step::Download, format!("No HTTPS client: {e}")))?;
        Ok(Https { client })
    }
}

#[cfg(feature = "fetch")]
impl Fetch for Https {
    fn get(&self, url: &str) -> Result<Response, Failure> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|e| Failure::new("install.network", Step::Download, format!("{url}: {e}")))?;
        Ok(Response { status: response.status().as_u16(), length: response.content_length(), body: Box::new(response) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `GET https://registry.npmjs.org/sidevoice/latest` as it answered for 0.8.1 (trimmed of fields nothing reads).
    const LATEST_0_8_1: &str = r#"{"bin":{"sidevoice":"bin/sidevoice.js"},"dist":{"shasum":"d4de06e888258e505d727aa8ebe624407defcb78","tarball":"https://registry.npmjs.org/sidevoice/-/sidevoice-0.8.1.tgz","fileCount":4,"integrity":"sha512-yqub7slq8o+kxprBoRhrlfZMXuPtFWJ6imw6geKXp+a8fADbt+A+ov7VEUim3+pEglmIn/X4pZoYswZS7WceIg==","unpackedSize":15557},"name":"sidevoice","engines":{"node":">=18"},"license":"Apache-2.0","optionalDependencies":{"@sidevoice/sidevoice-linux-x64":"0.8.1","@sidevoice/sidevoice-linux-arm64":"0.8.1","@sidevoice/sidevoice-darwin-arm64":"0.8.1"},"_id":"sidevoice@0.8.1","version":"0.8.1"}"#;

    /// `GET https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/0.8.1` (trimmed likewise).
    const DARWIN_0_8_1: &str = r#"{"os":["darwin"],"cpu":["arm64"],"dist":{"shasum":"c927bf74debe92ab09771fe36ba29c86a6c981d7","tarball":"https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/sidevoice-darwin-arm64-0.8.1.tgz","fileCount":104,"integrity":"sha512-SURTSpLHq1aNxsdyNxZpsrUE/lan6QWkbYrMfgcizM0WtYxegWuH/Id3NE/UX51L3dNayiMpZV1ADf5TFMx+mw==","signatures":[{"sig":"MEUCIHecH4lUFACZ7oHlne7r1sEPpgiFDGhHfK4TzcjH+BBAAiEAqehJNbEG7aYCKjhqlqyuqU3oR88Ca+zrGGVO5KDcCHc=","keyid":"SHA256:DhQ8wR5APBvFHLF/+Tc+AYvPOdTpcIDqOhxsBHRwC7U"}],"unpackedSize":15454847},"name":"@sidevoice/sidevoice-darwin-arm64","license":"Apache-2.0","_id":"@sidevoice/sidevoice-darwin-arm64@0.8.1","version":"0.8.1"}"#;

    const DARWIN: &str = "@sidevoice/sidevoice-darwin-arm64";

    fn darwin() -> Release {
        Release { version: "0.8.1".into(), package: DARWIN.into() }
    }

    #[test]
    fn platforms_are_named_as_npm_names_them() {
        assert_eq!(platform_package("macos", "aarch64").as_deref(), Some(DARWIN));
        assert_eq!(platform_package("linux", "x86_64").as_deref(), Some("@sidevoice/sidevoice-linux-x64"));
        assert_eq!(platform_package("linux", "aarch64").as_deref(), Some("@sidevoice/sidevoice-linux-arm64"));
        // An Intel Mac has a name, and no package (`install.no-bundle`, below); a system npm has no name for, neither.
        assert_eq!(platform_package("macos", "x86_64").as_deref(), Some("@sidevoice/sidevoice-darwin-x64"));
        assert_eq!(platform_package("windows", "x86_64"), None);
        assert_eq!(platform_package("linux", "riscv64"), None);
    }

    #[test]
    fn the_real_documents_of_0_8_1_resolve() {
        let release = release(LATEST_0_8_1.as_bytes(), DARWIN).unwrap();
        assert_eq!(release, darwin());
        assert_eq!(version_url(&release), "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/0.8.1");
        assert_eq!(latest_url(), "https://registry.npmjs.org/sidevoice/latest");
        let dist = dist(DARWIN_0_8_1.as_bytes(), &release).unwrap();
        assert_eq!(
            dist.tarball,
            "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/sidevoice-darwin-arm64-0.8.1.tgz"
        );
        // `sha512sum` of that tarball as the registry serves it.
        let hex: String = dist.sha512.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "4944534a92c7ab568dc6c772371669b2b504fe56a7e905a46d8acc7e0722cccd\
             16b58c5e816b87fc8777344fd45f9d4bddd35aca2329655d400dfe5314cc7e9b"
        );
    }

    #[test]
    fn a_platform_the_release_does_not_list_has_no_bundle() {
        for package in ["@sidevoice/sidevoice-darwin-x64", "@sidevoice/sidevoice-win32-x64"] {
            let refused = release(LATEST_0_8_1.as_bytes(), package).unwrap_err();
            assert_eq!((refused.key.as_str(), refused.step), ("install.no-bundle", Some(Step::Download)), "{package}");
        }
        let skewed = LATEST_0_8_1.replace(
            r#""@sidevoice/sidevoice-darwin-arm64":"0.8.1""#,
            r#""@sidevoice/sidevoice-darwin-arm64":"^0.8.0""#,
        );
        assert_eq!(release(skewed.as_bytes(), DARWIN).unwrap_err().key, "install.no-bundle", "only the same version");
    }

    #[test]
    fn a_latest_document_that_is_not_one_is_refused() {
        let manifest = |doc: &str| {
            let refused = release(doc.as_bytes(), DARWIN).unwrap_err();
            assert_eq!(refused.key, "install.authenticity", "{doc}");
            assert_eq!(refused.params, Some(json!({"check": "manifest"})), "{doc}");
        };
        manifest("nope");
        manifest("[]");
        manifest(&LATEST_0_8_1.replace(r#""name":"sidevoice""#, r#""name":"sidevoice-evil""#));
        manifest(&LATEST_0_8_1.replace(r#""version":"0.8.1""#, r#""version":"../../x""#));
        manifest(&LATEST_0_8_1.replace(r#""version":"0.8.1""#, r#""version":"0.8.1/../evil""#));
    }

    #[test]
    fn a_version_document_for_another_package_or_without_dist_is_refused() {
        let check = |doc: &str, expected: &str| {
            let refused = dist(doc.as_bytes(), &darwin()).unwrap_err();
            assert_eq!(refused.key, "install.authenticity", "{doc}");
            assert_eq!(refused.params, Some(json!({"check": expected})), "{doc}");
        };
        check(&DARWIN_0_8_1.replace(r#""version":"0.8.1""#, r#""version":"0.8.0""#), "manifest");
        check(
            &DARWIN_0_8_1.replace(r#""name":"@sidevoice/sidevoice-darwin-arm64""#, r#""name":"@evil/x""#),
            "manifest",
        );
        check(&DARWIN_0_8_1.replace(r#""integrity":"#, r#""integrity_":"#), "manifest");
        check(&DARWIN_0_8_1.replace("sha512-SURT", "sha1-SURT"), "manifest");
        check(&DARWIN_0_8_1.replace(r#""tarball":"#, r#""tarball_":"#), "manifest");
        check(
            &DARWIN_0_8_1.replace("https://registry.npmjs.org/@sidevoice", "https://evil.example/@sidevoice"),
            "download",
        );
    }

    #[test]
    fn only_the_packages_own_tarball_on_the_registry_is_fetched() {
        let release = darwin();
        let good = "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/sidevoice-darwin-arm64-0.8.1.tgz";
        assert!(tarball_allowed(good, &release));
        for bad in [
            "http://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/sidevoice-darwin-arm64-0.8.1.tgz",
            "https://registry.npmjs.org.evil.example/@sidevoice/sidevoice-darwin-arm64/-/x.tgz",
            "https://registry.npmjs.org@evil.example/@sidevoice/sidevoice-darwin-arm64/-/x.tgz",
            "https://user@registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/x.tgz",
            "https://registry.npmjs.org:8443/@sidevoice/sidevoice-darwin-arm64/-/x.tgz",
            "https://registry.yarnpkg.com/@sidevoice/sidevoice-darwin-arm64/-/x.tgz",
            "https://registry.npmjs.org/@sidevoice/sidevoice-linux-x64/-/sidevoice-linux-x64-0.8.1.tgz",
            "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/../../evil/-/x.tgz",
            "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/x.tgz?redirect=https://evil.example",
            "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/x.tgz#frag",
            "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/%2e%2e%2fx.tgz",
            "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/sub/x.tgz",
            "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/x.zip",
            "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/.tgz",
            "https://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/x\\y.tgz",
            "HTTPS://registry.npmjs.org/@sidevoice/sidevoice-darwin-arm64/-/x.tgz",
        ] {
            assert!(!tarball_allowed(bad, &release), "{bad}");
        }
    }

    #[test]
    fn the_integrity_is_exactly_one_sha512() {
        let digest = [7u8; 64];
        let encoded = base64::engine::general_purpose::STANDARD.encode(digest);
        assert_eq!(sha512_of(&format!("sha512-{encoded}")), Some(digest));
        assert_eq!(sha512_of(&format!("sha1-AAAA sha512-{encoded}")), Some(digest), "beside another algorithm");
        assert_eq!(sha512_of(&format!("sha512-{encoded}?opt")), Some(digest));
        assert_eq!(sha512_of(&format!("sha512-{encoded} sha512-{encoded}")), None, "two: which one?");
        assert_eq!(sha512_of("sha256-AAAA"), None);
        assert_eq!(sha512_of("sha512-not base64!"), None);
        let short = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        assert_eq!(sha512_of(&format!("sha512-{short}")), None, "64 bytes or nothing");
        assert_eq!(sha512_of(""), None);
    }

    #[test]
    fn registry_statuses_map_to_keys() {
        let response = |status| Response { status, length: None, body: Box::new(std::io::empty()) };
        assert!(expect_ok(&response(200), "u").is_ok());
        assert_eq!(expect_ok(&response(404), "u").unwrap_err().key, "install.no-bundle");
        let redirect = expect_ok(&response(302), "u").unwrap_err();
        assert_eq!(
            (redirect.key.as_str(), redirect.params),
            ("install.authenticity", Some(json!({"check": "download"})))
        );
        assert_eq!(expect_ok(&response(503), "u").unwrap_err().key, "install.network");
        assert_eq!(expect_ok(&response(403), "u").unwrap_err().key, "install.network");
    }
}
