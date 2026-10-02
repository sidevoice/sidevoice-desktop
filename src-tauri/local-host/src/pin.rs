//! Identity pinned for the connector executable carried by a macOS arm64 app build.
//!
//! The checked-in pin stays explicitly pending until R4-b publishes a genuine SEA and its verified embedded
//! manifest. Test fixtures live under `test-fixtures/` and are never read by production code.

use crate::Refusal;
use base64::Engine;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const SHA256_LEN: usize = 64;
const CORE_RELEASE_PREFIX: &str = "https://github.com/sidevoice/sidevoice-core/releases/download/";
const CORE_TARGETS: [(&str, &str, &str); 3] =
    [("macos", "aarch64", "macos-aarch64"), ("linux", "x86_64", "linux-x86_64"), ("linux", "aarch64", "linux-aarch64")];

fn invalid_pin(why: &str) -> Refusal {
    Refusal::new("install.pin-invalid", format!("The connector build pin {why}."))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ConnectorPin {
    pub schema: u32,
    pub status: String,
    pub target: String,
    pub connector_sha: Option<String>,
    pub connector_version: Option<String>,
    pub channel: Option<String>,
    pub build_seq: Option<u64>,
    pub core_version: Option<String>,
    pub core_manifest_sha256: Option<String>,
    pub core_manifest_size: Option<u64>,
    pub core_manifest_bytes_base64: Option<String>,
    pub core_assets: Vec<CoreAssetPin>,
    pub core_manifest_sidecars: Vec<SidecarPin>,
    pub executable_sha256: Option<String>,
    pub executable_size: Option<u64>,
    pub asset_url: Option<String>,
    pub metadata_protocol: Option<String>,
    pub progress_protocol: Option<String>,
    pub core_api: Option<i64>,
    pub core_link: Option<i64>,
    pub link_min: Option<i64>,
    pub link_max: Option<i64>,
    pub provenance: Option<ProvenancePin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenancePin {
    pub repository: Option<String>,
    pub repository_id: Option<String>,
    pub workflow: Option<String>,
    pub run_id: Option<u64>,
    pub artifact_name: Option<String>,
    pub sidecars: Vec<SidecarPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarPin {
    pub name: String,
    pub url: Option<String>,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreAssetPin {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledBuild {
    pub connector_version: Option<String>,
    pub connector_sha: Option<String>,
    pub core_version: Option<String>,
    pub core_build: Option<String>,
    pub core_manifest_sha256: Option<String>,
    pub channel: Option<String>,
    pub build_seq: Option<u64>,
    pub core_api: Option<i64>,
    pub core_link: Option<i64>,
}

fn exact_keys(value: &Value, expected: &[&str]) -> bool {
    let Some(object) = value.as_object() else { return false };
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn encoded_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn core_release_asset(url: &str, filename: &str, version: &str, tag: Option<&str>) -> Option<String> {
    if url.contains('?') || url.contains('#') {
        return None;
    }
    let remainder = url.strip_prefix(CORE_RELEASE_PREFIX)?;
    let (release_tag, name) = remainder.split_once('/')?;
    let version_tag = format!("v{version}");
    if name.contains('/')
        || name != filename
        || (release_tag != "nightly" && release_tag != version_tag.as_str())
        || tag.is_some_and(|expected| expected != release_tag)
    {
        return None;
    }
    let expected = format!("{CORE_RELEASE_PREFIX}{}/{}", encoded_component(release_tag), encoded_component(filename));
    (expected == url).then(|| release_tag.to_string())
}

/// The signed core producer's exact `{bundles, wheel}` shape. Version identity comes from versioned asset filenames
/// and their release tag; accepting a synthetic `manifest.version` field would reject real producer bytes.
fn core_manifest_assets(manifest: &Value, version: &str) -> Option<Vec<CoreAssetPin>> {
    if !exact_keys(manifest, &["bundles", "wheel"]) {
        return None;
    }
    let bundles = manifest.get("bundles")?.as_array()?;
    if bundles.len() != CORE_TARGETS.len() {
        return None;
    }
    let mut release_tag = None;
    let mut assets = Vec::with_capacity(bundles.len());
    for (entry, (os, arch, target)) in bundles.iter().zip(CORE_TARGETS) {
        if !exact_keys(entry, &["os", "arch", "url", "sha256", "size"])
            || entry.get("os")?.as_str()? != os
            || entry.get("arch")?.as_str()? != arch
        {
            return None;
        }
        let name = format!("sidevoice-core-{version}-{target}.tar.zst");
        let url = entry.get("url")?.as_str()?;
        let tag = core_release_asset(url, &name, version, release_tag.as_deref())?;
        let sha256 = entry.get("sha256")?.as_str()?;
        let size = entry.get("size")?.as_u64()?;
        if !valid_sha(sha256) || size == 0 {
            return None;
        }
        release_tag = Some(tag);
        assets.push(CoreAssetPin { name, url: url.to_string(), sha256: sha256.to_string(), size });
    }
    let wheel = manifest.get("wheel")?;
    let wheel_name = format!("sidevoice_core-{version}-py3-none-any.whl");
    if !exact_keys(wheel, &["url", "sha256"])
        || core_release_asset(wheel.get("url")?.as_str()?, &wheel_name, version, release_tag.as_deref()).is_none()
        || !valid_sha(wheel.get("sha256")?.as_str()?)
    {
        return None;
    }
    Some(assets)
}

impl ConnectorPin {
    pub fn from_json(text: &str) -> Result<Self, Refusal> {
        serde_json::from_str(text).map_err(|error| {
            Refusal::new("install.pin-invalid", format!("The connector build pin is not valid JSON: {error}"))
        })
    }

    /// Production package inputs must be complete and tied to an immutable, public connector build.
    pub fn validate_ready(&self) -> Result<(), Refusal> {
        if self.schema != 1 {
            return Err(invalid_pin("uses an unsupported schema"));
        }
        if self.status != "ready" {
            return Err(invalid_pin("is pending R4-b's signed macOS arm64 artifact"));
        }
        if self.target != "macos-aarch64" {
            return Err(invalid_pin("does not target macOS arm64"));
        }
        let connector_sha = self.connector_sha.as_deref().ok_or_else(|| invalid_pin("has no connector SHA"))?;
        if connector_sha.len() != 40 || !connector_sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid_pin("has an invalid connector SHA"));
        }
        let connector_version =
            self.connector_version.as_deref().ok_or_else(|| invalid_pin("has no connector version"))?;
        Version::parse(connector_version).map_err(|_| invalid_pin("has an invalid connector version"))?;
        if !matches!(self.channel.as_deref(), Some("release" | "nightly")) {
            return Err(invalid_pin("has no supported release channel"));
        }
        if self.build_seq.unwrap_or_default() == 0 {
            return Err(invalid_pin("has no build sequence"));
        }
        let core_version = self.core_version.as_deref().ok_or_else(|| invalid_pin("has no core version"))?;
        Version::parse(core_version).map_err(|_| invalid_pin("has an invalid core version"))?;
        validate_sha(self.core_manifest_sha256.as_deref(), "core manifest")?;
        let manifest_b64 = self
            .core_manifest_bytes_base64
            .as_deref()
            .ok_or_else(|| invalid_pin("has no pinned core manifest bytes"))?;
        let manifest = base64::engine::general_purpose::STANDARD
            .decode(manifest_b64)
            .map_err(|_| invalid_pin("has invalid base64 core manifest bytes"))?;
        if Some(manifest.len() as u64) != self.core_manifest_size
            || Some(sha256_bytes(&manifest).as_str()) != self.core_manifest_sha256.as_deref()
        {
            return Err(invalid_pin("has core manifest bytes that do not match the pinned digest and size"));
        }
        let manifest_json: Value =
            serde_json::from_slice(&manifest).map_err(|_| invalid_pin("has invalid pinned core manifest JSON"))?;
        let expected_assets = core_manifest_assets(
            &manifest_json,
            self.core_version.as_deref().ok_or_else(|| invalid_pin("has no core version"))?,
        )
        .ok_or_else(|| invalid_pin("does not match the signed producer's versioned bundles/wheel schema"))?;
        if self.core_assets.is_empty() || self.core_manifest_sidecars.is_empty() {
            return Err(invalid_pin("has no pinned core assets or manifest attestations"));
        }
        if self.core_assets != expected_assets {
            return Err(invalid_pin("has core assets that do not match the pinned manifest bundles"));
        }
        for asset in &self.core_assets {
            if asset.name.is_empty()
                || asset.name.contains('/')
                || !asset.url.starts_with("https://")
                || !valid_sha(&asset.sha256)
                || asset.size == 0
            {
                return Err(invalid_pin("has an invalid pinned core asset"));
            }
        }
        for sidecar in &self.core_manifest_sidecars {
            validate_sidecar(sidecar).map_err(invalid_pin)?;
        }
        validate_sha(self.executable_sha256.as_deref(), "executable")?;
        if self.executable_size.unwrap_or_default() == 0 {
            return Err(invalid_pin("has no executable size"));
        }
        let url = self.asset_url.as_deref().ok_or_else(|| invalid_pin("has no immutable asset URL"))?;
        if !url.starts_with("https://api.github.com/repos/sidevoice/sidevoice-connector/actions/runs/") {
            return Err(invalid_pin("does not name a public connector asset"));
        }
        if self.metadata_protocol.as_deref() != Some("sidevoice-metadata-v1")
            || self.progress_protocol.as_deref() != Some("sidevoice-progress-jsonl-v1")
        {
            return Err(invalid_pin("does not pin the required R4-b metadata and progress protocols"));
        }
        if self.core_api.unwrap_or_default() <= 0 || self.core_link.unwrap_or_default() <= 0 {
            return Err(invalid_pin("has no embedded core API or link id"));
        }
        if !matches!((self.link_min, self.link_max), (Some(min), Some(max)) if min > 0 && min <= max)
            || !matches!((self.link_min, self.link_max, self.core_link), (Some(min), Some(max), Some(link)) if min <= link && link <= max)
        {
            return Err(invalid_pin("has an invalid connector/core link compatibility range"));
        }
        let provenance = self.provenance.as_ref().ok_or_else(|| invalid_pin("has no provenance record"))?;
        if provenance.repository.as_deref() != Some("sidevoice/sidevoice-connector")
            || provenance.repository_id.as_deref().map_or(true, str::is_empty)
            || provenance.workflow.as_deref().map_or(true, str::is_empty)
            || provenance.run_id.unwrap_or_default() == 0
            || provenance.artifact_name.as_deref().map_or(true, str::is_empty)
            || provenance.sidecars.is_empty()
        {
            return Err(invalid_pin("has incomplete R4-b provenance"));
        }
        let run_asset = format!(
            "https://api.github.com/repos/sidevoice/sidevoice-connector/actions/runs/{}/artifacts/",
            provenance.run_id.unwrap_or_default()
        );
        let artifact_id = url.strip_prefix(&run_asset).and_then(|path| path.strip_suffix("/zip"));
        if !matches!(artifact_id, Some(id) if !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())) {
            return Err(invalid_pin("does not name the immutable artifact ZIP from its pinned CI run"));
        }
        for sidecar in &provenance.sidecars {
            validate_sidecar(sidecar).map_err(invalid_pin)?;
        }
        Ok(())
    }

    /// Checks both version output and the machine-readable embedded-manifest report from the executable.
    pub fn verify_metadata(&self, version: &Value, metadata: &Value) -> Result<(), Refusal> {
        self.validate_ready()?;
        let mismatch = |field: &str| {
            Refusal::new(
                "install.pin-mismatch",
                format!("The bundled connector's {field} does not match its build pin."),
            )
        };
        let expected = [
            ("version", self.connector_version.as_deref()),
            ("target", Some("macos-aarch64")),
            ("channel", self.channel.as_deref()),
            ("connector_sha", self.connector_sha.as_deref()),
        ];
        for (field, value) in expected {
            if version.get(field).and_then(Value::as_str) != value {
                return Err(mismatch(field));
            }
        }
        if version.get("build_seq").and_then(Value::as_u64) != self.build_seq {
            return Err(mismatch("build_seq"));
        }
        let connector = metadata.get("connector").ok_or_else(|| mismatch("connector metadata"))?;
        let core = metadata.get("embedded_core").ok_or_else(|| mismatch("embedded core manifest"))?;
        let protocol = metadata.get("protocols").ok_or_else(|| mismatch("CLI protocols"))?;
        for (field, value) in [
            ("sha", self.connector_sha.as_deref()),
            ("version", self.connector_version.as_deref()),
            ("channel", self.channel.as_deref()),
        ] {
            if connector.get(field).and_then(Value::as_str) != value {
                return Err(mismatch(field));
            }
        }
        if connector.get("build_seq").and_then(Value::as_u64) != self.build_seq {
            return Err(mismatch("build_seq"));
        }
        for (field, value) in
            [("version", self.core_version.as_deref()), ("manifest_sha256", self.core_manifest_sha256.as_deref())]
        {
            if core.get(field).and_then(Value::as_str) != value {
                return Err(mismatch(field));
            }
        }
        let pinned_assets = serde_json::to_value(&self.core_assets).unwrap_or(Value::Null);
        if core.get("assets") != Some(&pinned_assets) {
            return Err(mismatch("embedded core assets"));
        }
        if protocol.get("metadata").and_then(Value::as_str) != self.metadata_protocol.as_deref()
            || protocol.get("progress").and_then(Value::as_str) != self.progress_protocol.as_deref()
        {
            return Err(mismatch("progress/metadata protocol"));
        }
        for (field, expected) in [("api", self.core_api), ("link", self.core_link)] {
            if core.get(field).and_then(Value::as_i64) != expected {
                return Err(mismatch(field));
            }
        }
        for (field, expected) in [("link_min", self.link_min), ("link_max", self.link_max)] {
            if connector.get(field).and_then(Value::as_i64) != expected {
                return Err(mismatch(field));
            }
        }
        Ok(())
    }

    pub fn as_public_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

fn validate_sha(value: Option<&str>, label: &str) -> Result<(), Refusal> {
    if value.is_some_and(valid_sha) {
        Ok(())
    } else {
        Err(Refusal::new("install.pin-invalid", format!("The connector build pin has an invalid {label} SHA-256.")))
    }
}

fn valid_sha(value: &str) -> bool {
    value.len() == SHA256_LEN && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_sidecar(sidecar: &SidecarPin) -> Result<(), &'static str> {
    if sidecar.name.is_empty()
        || !sidecar.name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || sidecar.url.as_deref().map_or(true, |url| !url.starts_with("https://"))
        || !valid_sha(&sidecar.sha256)
        || sidecar.size == 0
    {
        Err("has an invalid pinned sidecar")
    } else {
        Ok(())
    }
}

impl InstalledBuild {
    /// Projects public metadata from the trusted selected `R/current/release.json`; the install command never reaches
    /// the page. `core_build` identifies the runtime selected by R1/R4 without treating it as a protocol link id.
    pub fn from_release_record(record: &Value, core_api: Option<i64>, core_link: Option<i64>) -> Self {
        let text = |key: &str| record.get(key).and_then(Value::as_str).map(str::to_string);
        let number = |key: &str| record.get(key).and_then(Value::as_u64);
        InstalledBuild {
            connector_version: text("connector"),
            connector_sha: text("connector_sha"),
            core_version: text("core"),
            core_build: text("core_build"),
            core_manifest_sha256: text("core_manifest_sha256"),
            channel: text("channel"),
            build_seq: number("build_seq"),
            core_api: core_api.or_else(|| record.get("core_api").and_then(Value::as_i64)),
            core_link: core_link.or_else(|| record.get("core_link").and_then(Value::as_i64)),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn fixture_pin() -> ConnectorPin {
        let manifest = include_bytes!("../test-fixtures/core-manifest-core34.json");
        let manifest_json: Value = serde_json::from_slice(manifest).unwrap();
        let core_assets = core_manifest_assets(&manifest_json, "0.1.0").unwrap();
        let manifest_sha = sha256_bytes(manifest);
        ConnectorPin {
            schema: 1,
            status: "ready".into(),
            target: "macos-aarch64".into(),
            connector_sha: Some("a".repeat(40)),
            connector_version: Some("1.2.3".into()),
            channel: Some("nightly".into()),
            build_seq: Some(42),
            core_version: Some("0.1.0".into()),
            core_manifest_sha256: Some(manifest_sha),
            core_manifest_size: Some(manifest.len() as u64),
            core_manifest_bytes_base64: Some(base64::engine::general_purpose::STANDARD.encode(manifest)),
            core_assets,
            core_manifest_sidecars: vec![SidecarPin {
                name: "core-manifest.sigstore.json".into(),
                url: Some("https://github.com/sidevoice/sidevoice-core/releases/download/v0.1.0/core-manifest.sigstore.json".into()),
                sha256: "d".repeat(64),
                size: 10,
            }],
            executable_sha256: Some("c".repeat(64)),
            executable_size: Some(12),
            asset_url: Some("https://api.github.com/repos/sidevoice/sidevoice-connector/actions/runs/3/artifacts/4/zip".into()),
            metadata_protocol: Some("sidevoice-metadata-v1".into()),
            progress_protocol: Some("sidevoice-progress-jsonl-v1".into()),
            core_api: Some(1),
            core_link: Some(1),
            link_min: Some(1),
            link_max: Some(1),
            provenance: Some(ProvenancePin {
                repository: Some("sidevoice/sidevoice-connector".into()),
                repository_id: Some("12345".into()),
                workflow: Some(".github/workflows/build.yml@refs/heads/main".into()),
                run_id: Some(3),
                artifact_name: Some("sidevoice-macos-aarch64".into()),
                sidecars: vec![SidecarPin { name: "sidevoice.sigstore.json".into(),
                    url: Some("https://github.com/sidevoice/sidevoice-connector/releases/download/v1.2.3/sidevoice.sigstore.json".into()),
                    sha256: "d".repeat(64), size: 10 }],
            }),
        }
    }

    #[test]
    fn pending_or_incomplete_pin_is_rejected() {
        let pin = fixture_pin();
        assert!(pin.validate_ready().is_ok(), "core PR #34's `{bundles,wheel}` bytes are accepted as produced");
        let mut pin = fixture_pin();
        pin.status = "pending".into();
        assert_eq!(pin.validate_ready().unwrap_err().key, "install.pin-invalid");
        let mut pin = fixture_pin();
        pin.executable_sha256 = Some("not-a-digest".into());
        assert_eq!(pin.validate_ready().unwrap_err().key, "install.pin-invalid");
        let mut pin = fixture_pin();
        pin.core_assets[0].size += 1;
        assert_eq!(pin.validate_ready().unwrap_err().key, "install.pin-invalid");
    }

    #[test]
    fn metadata_must_match_both_the_executable_and_embedded_manifest() {
        let pin = fixture_pin();
        let version = serde_json::json!({"version":"1.2.3", "target":"macos-aarch64", "channel":"nightly",
            "connector_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "build_seq":42});
        let metadata = serde_json::json!({"connector":{"version":"1.2.3", "sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "channel":"nightly", "build_seq":42, "link_min":1, "link_max":1}, "embedded_core":{"version":"0.1.0",
            "manifest_sha256":pin.core_manifest_sha256, "assets":pin.core_assets,
            "api":1, "link":1},
            "protocols":{"metadata":"sidevoice-metadata-v1", "progress":"sidevoice-progress-jsonl-v1"}});
        pin.verify_metadata(&version, &metadata).unwrap();
        let mut wrong = metadata;
        wrong["embedded_core"]["manifest_sha256"] = Value::String("0".repeat(64));
        assert_eq!(pin.verify_metadata(&version, &wrong).unwrap_err().key, "install.pin-mismatch");
    }

    #[test]
    fn release_record_projection_does_not_include_the_install_command() {
        let installed = InstalledBuild::from_release_record(
            &serde_json::json!({"id":"1.0.0", "connector":"1.0.0", "connector_sha":"e".repeat(40), "core":"0.8.0",
                "core_build":"build-17", "channel":"release", "build_seq":12, "command":["/private/path"]}),
            Some(1),
            Some(1),
        );
        let value = serde_json::to_value(installed).unwrap();
        assert!(value.get("command").is_none());
        assert_eq!(value["connectorVersion"], "1.0.0");
        assert_eq!(value["connectorSha"], "e".repeat(40));
        assert_eq!(value["coreBuild"], "build-17");
    }
}
