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
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    pub core_assets: Vec<CoreAssetPin>,
    #[serde(default)]
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
    #[serde(default)]
    pub native_pair: Option<NativePairPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePairPin {
    pub runtime_kind: String,
    pub runtime_build_sha: String,
    pub runtime_sha256: String,
    pub runtime_size: u64,
    pub runtime_target: String,
    pub core_kind: String,
    pub core_source_sha: String,
    pub core_cargo_lock_sha256: String,
    pub core_manifest_sha256: String,
    pub core_manifest_size: u64,
    pub core_manifest_bytes_base64: String,
    pub core_archive_sha256: String,
    pub core_archive_size: u64,
    pub core_target: String,
    pub core_entrypoint: String,
    pub core_build: String,
    pub pair_id: String,
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
    pub format: Option<String>,
    pub core_version: Option<String>,
    pub core_build: Option<String>,
    pub core_manifest_sha256: Option<String>,
    pub channel: Option<String>,
    pub build_seq: Option<u64>,
    pub core_api: Option<i64>,
    pub core_link: Option<i64>,
    pub runtime_kind: Option<String>,
    pub pair_id: Option<String>,
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
        if self.schema == 2 {
            return self.validate_native_pair_ready();
        }
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
        if !url.starts_with("https://api.github.com/repos/sidevoice/sidevoice-connector/actions/artifacts/") {
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
        // Run/repository/SHA provenance is checked by the authenticated build-time downloader.
        // GitHub's download endpoint is artifact-scoped, never nested beneath actions/runs.
        let artifact_prefix = "https://api.github.com/repos/sidevoice/sidevoice-connector/actions/artifacts/";
        let artifact_id = url.strip_prefix(artifact_prefix).and_then(|path| path.strip_suffix("/zip"));
        if !matches!(artifact_id, Some(id) if !id.is_empty() && !id.starts_with('0') && id.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(invalid_pin("does not name the immutable artifact ZIP from its pinned CI run"));
        }
        for sidecar in &provenance.sidecars {
            validate_sidecar(sidecar).map_err(invalid_pin)?;
        }
        Ok(())
    }

    fn validate_native_pair_ready(&self) -> Result<(), Refusal> {
        if self.status != "ready" || self.target != "macos-aarch64" {
            return Err(invalid_pin("is not a ready macOS arm64 native pair"));
        }
        let connector_sha = self.connector_sha.as_deref().ok_or_else(|| invalid_pin("has no Connector source SHA"))?;
        if !valid_git_sha(connector_sha) {
            return Err(invalid_pin("has an invalid Connector source SHA"));
        }
        let connector_version =
            self.connector_version.as_deref().ok_or_else(|| invalid_pin("has no Connector version"))?;
        Version::parse(connector_version).map_err(|_| invalid_pin("has an invalid Connector version"))?;
        if !matches!(self.channel.as_deref(), Some("release" | "nightly")) || self.build_seq.is_none() {
            return Err(invalid_pin("has invalid native-pair channel metadata"));
        }
        let core_version = self.core_version.as_deref().ok_or_else(|| invalid_pin("has no Core version"))?;
        Version::parse(core_version).map_err(|_| invalid_pin("has an invalid Core version"))?;
        if self.metadata_protocol.as_deref() != Some("sidevoice-metadata-v1")
            || self.progress_protocol.as_deref() != Some("sidevoice-progress-jsonl-v1")
        {
            return Err(invalid_pin("does not pin the required R4 metadata and progress protocols"));
        }
        if self.core_api.unwrap_or_default() <= 0
            || self.core_link.unwrap_or_default() <= 0
            || !matches!((self.link_min, self.link_max, self.core_link), (Some(min), Some(max), Some(link))
                if min > 0 && min <= link && link <= max)
        {
            return Err(invalid_pin("has invalid native-pair Core API or link metadata"));
        }
        validate_sha(self.executable_sha256.as_deref(), "native-pair executable")?;
        if !matches!(self.executable_size, Some(1..=536_870_912)) {
            return Err(invalid_pin("has no bounded native-pair executable size"));
        }
        if self.core_manifest_sha256.is_some()
            || self.core_manifest_size.is_some()
            || self.core_manifest_bytes_base64.is_some()
            || !self.core_assets.is_empty()
            || !self.core_manifest_sidecars.is_empty()
            || self.asset_url.is_some()
            || self.provenance.is_some()
        {
            return Err(invalid_pin("mixes the Rust-native pair with Python bundle or downloaded-artifact fields"));
        }
        let pair = self.native_pair.as_ref().ok_or_else(|| invalid_pin("has no Rust-native pair identity"))?;
        if pair.runtime_kind != "rust-native-v1"
            || pair.core_kind != "rust-native-v1"
            || pair.runtime_build_sha != connector_sha
            || !valid_git_sha(&pair.core_source_sha)
            || pair.runtime_target != self.target
            || pair.core_target != self.target
            || pair.core_entrypoint != "bin/sidevoice-core-rust"
            || !valid_sha_lower(&pair.runtime_sha256)
            || !(1..=100_000_000).contains(&pair.runtime_size)
            || !valid_sha_lower(&pair.core_cargo_lock_sha256)
            || !valid_sha_lower(&pair.core_manifest_sha256)
            || !valid_sha_lower(&pair.core_archive_sha256)
            || !(1..=250_000_000).contains(&pair.core_archive_size)
            || !(1..=4_000_000).contains(&pair.core_manifest_size)
        {
            return Err(invalid_pin("has an invalid Rust runtime or Core identity"));
        }
        let manifest_bytes = base64::engine::general_purpose::STANDARD
            .decode(&pair.core_manifest_bytes_base64)
            .map_err(|_| invalid_pin("has invalid native Core manifest base64"))?;
        if base64::engine::general_purpose::STANDARD.encode(&manifest_bytes) != pair.core_manifest_bytes_base64
            || manifest_bytes.len() as u64 != pair.core_manifest_size
            || sha256_bytes(&manifest_bytes) != pair.core_manifest_sha256
        {
            return Err(invalid_pin("has native Core manifest bytes that do not match their pinned digest and size"));
        }
        let manifest: Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| invalid_pin("has invalid native Core manifest JSON"))?;
        let mut canonical =
            serde_json::to_vec(&manifest).map_err(|_| invalid_pin("has invalid native Core manifest JSON"))?;
        canonical.push(b'\n');
        if canonical != manifest_bytes {
            return Err(invalid_pin("has noncanonical native Core manifest bytes"));
        }
        let targets = ["macos-aarch64", "linux-x86_64", "linux-aarch64"];
        if !exact_keys(&manifest, &["schema", "kind", "source_sha", "cargo_lock_sha256", "entrypoint", "bundles"])
            || manifest.get("schema").and_then(Value::as_u64) != Some(1)
            || manifest.get("kind").and_then(Value::as_str) != Some("rust-native-v1")
            || manifest.get("source_sha").and_then(Value::as_str) != Some(pair.core_source_sha.as_str())
            || manifest.get("cargo_lock_sha256").and_then(Value::as_str) != Some(pair.core_cargo_lock_sha256.as_str())
            || manifest.get("entrypoint").and_then(Value::as_str) != Some(pair.core_entrypoint.as_str())
        {
            return Err(invalid_pin("does not match the Rust-native closed manifest schema"));
        }
        let bundles = manifest.get("bundles").ok_or_else(|| invalid_pin("has no native Core bundles"))?;
        if !exact_keys(bundles, &targets) {
            return Err(invalid_pin("does not contain the exact closed Rust-native target set"));
        }
        for target in targets {
            let entry = bundles.get(target).ok_or_else(|| invalid_pin("is missing a native Core target"))?;
            let name = format!("sidevoice-core-rust-{}-{target}.tar.zst", pair.core_source_sha);
            if !exact_keys(entry, &["name", "size", "sha256"])
                || entry.get("name").and_then(Value::as_str) != Some(name.as_str())
                || !matches!(entry.get("size").and_then(Value::as_u64), Some(1..=250_000_000))
                || !entry.get("sha256").and_then(Value::as_str).is_some_and(valid_sha_lower)
            {
                return Err(invalid_pin("has an invalid Rust-native archive record"));
            }
        }
        let selected =
            bundles.get(pair.core_target.as_str()).ok_or_else(|| invalid_pin("has no macOS Core archive"))?;
        let expected_core_build =
            format!("rust-native-v1-{}-{}-{}", pair.core_target, pair.core_source_sha, pair.core_archive_sha256);
        if selected.get("sha256").and_then(Value::as_str) != Some(pair.core_archive_sha256.as_str())
            || selected.get("size").and_then(Value::as_u64) != Some(pair.core_archive_size)
            || pair.core_build != expected_core_build
            || pair.pair_id != format!("pair-v1:rust-native-v1:{}:core:{}", pair.runtime_sha256, pair.core_build)
        {
            return Err(invalid_pin("does not bind the pinned Rust runtime to the selected Core archive"));
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
        if self.schema == 2 {
            return self.verify_native_pair_metadata(version, metadata, &mismatch);
        }
        if version.get("format").and_then(Value::as_str) != Some("sea")
            || version.get("sea").and_then(Value::as_bool) != Some(true)
        {
            return Err(mismatch("SEA executable identity"));
        }
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
        if connector.get("format").and_then(Value::as_str) != Some("sea")
            || connector.get("sea").and_then(Value::as_bool) != Some(true)
        {
            return Err(mismatch("SEA metadata identity"));
        }
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

    fn verify_native_pair_metadata(
        &self,
        version: &Value,
        metadata: &Value,
        mismatch: &impl Fn(&str) -> Refusal,
    ) -> Result<(), Refusal> {
        if version.get("ok").and_then(Value::as_bool) != Some(true)
            || metadata.get("ok").and_then(Value::as_bool) != Some(true)
            || version.get("format").and_then(Value::as_str) != Some("sea")
            || version.get("sea").and_then(Value::as_bool) != Some(true)
        {
            return Err(mismatch("SEA executable identity"));
        }
        for (field, value) in [
            ("version", self.connector_version.as_deref()),
            ("target", Some("macos-aarch64")),
            ("channel", self.channel.as_deref()),
            ("connector_sha", self.connector_sha.as_deref()),
        ] {
            if version.get(field).and_then(Value::as_str) != value {
                return Err(mismatch(field));
            }
        }
        if version.get("build_seq").and_then(Value::as_u64) != self.build_seq {
            return Err(mismatch("build_seq"));
        }
        let connector = metadata.get("connector").ok_or_else(|| mismatch("connector metadata"))?;
        let core = metadata.get("embedded_core").ok_or_else(|| mismatch("Rust-native Core metadata"))?;
        let protocol = metadata.get("protocols").ok_or_else(|| mismatch("CLI protocols"))?;
        if connector.get("format").and_then(Value::as_str) != Some("sea")
            || connector.get("sea").and_then(Value::as_bool) != Some(true)
            || connector.get("sha").and_then(Value::as_str) != self.connector_sha.as_deref()
            || connector.get("version").and_then(Value::as_str) != self.connector_version.as_deref()
            || connector.get("target").and_then(Value::as_str) != Some("macos-aarch64")
            || connector.get("channel").and_then(Value::as_str) != self.channel.as_deref()
            || connector.get("build_seq").and_then(Value::as_u64) != self.build_seq
        {
            return Err(mismatch("Connector source-build metadata"));
        }
        if core.get("version").and_then(Value::as_str) != self.core_version.as_deref()
            || !core.get("manifest_sha256").is_some_and(Value::is_null)
            || core.get("assets").and_then(Value::as_array).is_none_or(|assets| !assets.is_empty())
            || core.get("api").and_then(Value::as_i64) != self.core_api
            || core.get("link").and_then(Value::as_i64) != self.core_link
            || connector.get("link_min").and_then(Value::as_i64) != self.link_min
            || connector.get("link_max").and_then(Value::as_i64) != self.link_max
        {
            return Err(mismatch("Rust-native Core source-build metadata"));
        }
        if protocol.get("metadata").and_then(Value::as_str) != self.metadata_protocol.as_deref()
            || protocol.get("progress").and_then(Value::as_str) != self.progress_protocol.as_deref()
        {
            return Err(mismatch("progress/metadata protocol"));
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

fn valid_sha_lower(value: &str) -> bool {
    value.len() == SHA256_LEN && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_git_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
            format: text("format"),
            core_version: text("core"),
            core_build: text("core_build"),
            core_manifest_sha256: text("core_manifest_sha256"),
            channel: text("channel"),
            build_seq: number("build_seq"),
            core_api: core_api.or_else(|| record.get("core_api").and_then(Value::as_i64)),
            core_link: core_link.or_else(|| record.get("core_link").and_then(Value::as_i64)),
            runtime_kind: text("runtime_kind"),
            pair_id: text("pair_id"),
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
            asset_url: Some("https://api.github.com/repos/sidevoice/sidevoice-connector/actions/artifacts/4/zip".into()),
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
            native_pair: None,
        }
    }

    pub fn fixture_native_pair_pin() -> ConnectorPin {
        let mut pin = fixture_pin();
        pin.schema = 2;
        pin.core_manifest_sha256 = None;
        pin.core_manifest_size = None;
        pin.core_manifest_bytes_base64 = None;
        pin.core_assets.clear();
        pin.core_manifest_sidecars.clear();
        pin.asset_url = None;
        pin.provenance = None;
        let core_source_sha = "e".repeat(40);
        let core_cargo_lock_sha256 = "f".repeat(64);
        let bundles = serde_json::json!({
            "macos-aarch64": {"name":format!("sidevoice-core-rust-{core_source_sha}-macos-aarch64.tar.zst"),
                "size":1,"sha256":"a".repeat(64)},
            "linux-x86_64": {"name":format!("sidevoice-core-rust-{core_source_sha}-linux-x86_64.tar.zst"),
                "size":2,"sha256":"b".repeat(64)},
            "linux-aarch64": {"name":format!("sidevoice-core-rust-{core_source_sha}-linux-aarch64.tar.zst"),
                "size":3,"sha256":"c".repeat(64)},
        });
        let manifest = serde_json::json!({
            "schema":1,"kind":"rust-native-v1","source_sha":core_source_sha,
            "cargo_lock_sha256":core_cargo_lock_sha256,"entrypoint":"bin/sidevoice-core-rust","bundles":bundles,
        });
        let mut manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        manifest_bytes.push(b'\n');
        let manifest_sha = sha256_bytes(&manifest_bytes);
        let archive_sha = manifest["bundles"]["macos-aarch64"]["sha256"].as_str().unwrap().to_string();
        let core_build = format!("rust-native-v1-macos-aarch64-{core_source_sha}-{archive_sha}");
        pin.native_pair = Some(NativePairPin {
            runtime_kind: "rust-native-v1".into(),
            runtime_build_sha: pin.connector_sha.clone().unwrap(),
            runtime_sha256: "d".repeat(64),
            runtime_size: 100,
            runtime_target: "macos-aarch64".into(),
            core_kind: "rust-native-v1".into(),
            core_source_sha,
            core_cargo_lock_sha256,
            core_manifest_sha256: manifest_sha,
            core_manifest_size: manifest_bytes.len() as u64,
            core_manifest_bytes_base64: base64::engine::general_purpose::STANDARD.encode(&manifest_bytes),
            core_archive_sha256: archive_sha,
            core_archive_size: 1,
            core_target: "macos-aarch64".into(),
            core_entrypoint: "bin/sidevoice-core-rust".into(),
            core_build: core_build.clone(),
            pair_id: format!("pair-v1:rust-native-v1:{}:core:{core_build}", "d".repeat(64)),
        });
        pin
    }

    #[test]
    fn pending_or_incomplete_pin_is_rejected() {
        let pin = fixture_pin();
        assert!(pin.validate_ready().is_ok(), "core PR #34's `{{bundles,wheel}}` bytes are accepted as produced");
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
    fn native_pair_pin_uses_a_distinct_closed_rust_core_manifest() {
        let pin = fixture_native_pair_pin();
        assert!(pin.validate_ready().is_ok());
        let mut mixed = pin.clone();
        mixed.core_assets.push(CoreAssetPin {
            name: "python-wheel.whl".into(),
            url: "https://example.invalid".into(),
            sha256: "a".repeat(64),
            size: 1,
        });
        assert_eq!(mixed.validate_ready().unwrap_err().key, "install.pin-invalid");

        let mut changed = pin.clone();
        changed.native_pair.as_mut().unwrap().core_archive_sha256 = "0".repeat(64);
        assert_eq!(changed.validate_ready().unwrap_err().key, "install.pin-invalid");

        let mut noncanonical = pin;
        noncanonical.native_pair.as_mut().unwrap().core_manifest_bytes_base64 =
            base64::engine::general_purpose::STANDARD.encode(b"{}\n");
        assert_eq!(noncanonical.validate_ready().unwrap_err().key, "install.pin-invalid");
    }

    #[test]
    fn native_pair_metadata_requires_the_rust_native_no_python_assets_shape() {
        let pin = fixture_native_pair_pin();
        let version = serde_json::json!({"ok":true,"version":"1.2.3","target":"macos-aarch64","channel":"nightly",
            "connector_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","build_seq":42,"format":"sea","sea":true});
        let metadata = serde_json::json!({"ok":true,"connector":{"version":"1.2.3","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "target":"macos-aarch64","channel":"nightly","build_seq":42,"format":"sea","sea":true,"link_min":1,"link_max":1},
            "embedded_core":{"version":"0.1.0","manifest_sha256":null,"assets":[],"api":1,"link":1},
            "protocols":{"metadata":"sidevoice-metadata-v1","progress":"sidevoice-progress-jsonl-v1"}});
        pin.verify_metadata(&version, &metadata).unwrap();
        let mut python_shape = metadata;
        python_shape["embedded_core"]["assets"] = serde_json::json!([{"name":"python-core.whl"}]);
        assert_eq!(pin.verify_metadata(&version, &python_shape).unwrap_err().key, "install.pin-mismatch");
    }

    #[test]
    fn artifact_url_is_canonical_and_not_nested_under_a_run() {
        for url in [
            "https://api.github.com/repos/sidevoice/sidevoice-connector/actions/runs/3/artifacts/4/zip",
            "https://api.github.com/repos/sidevoice/sidevoice-connector/actions/artifacts/0/zip",
            "https://api.github.com/repos/sidevoice/sidevoice-connector/actions/artifacts/4/zip?x=1",
        ] {
            let mut pin = fixture_pin();
            pin.asset_url = Some(url.into());
            assert_eq!(pin.validate_ready().unwrap_err().key, "install.pin-invalid");
        }
    }

    #[test]
    fn r4b_pin_requires_both_connector_and_core_manifest_attestations() {
        let mut pin = fixture_pin();
        pin.provenance.as_mut().unwrap().sidecars.clear();
        assert_eq!(pin.validate_ready().unwrap_err().key, "install.pin-invalid");

        let mut pin = fixture_pin();
        pin.core_manifest_sidecars.clear();
        assert_eq!(pin.validate_ready().unwrap_err().key, "install.pin-invalid");
    }

    #[test]
    fn metadata_must_match_both_the_executable_and_embedded_manifest() {
        let pin = fixture_pin();
        let version = serde_json::json!({"version":"1.2.3", "target":"macos-aarch64", "channel":"nightly",
            "connector_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "build_seq":42, "format":"sea", "sea":true});
        let metadata = serde_json::json!({"connector":{"version":"1.2.3", "sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "channel":"nightly", "build_seq":42, "format":"sea", "sea":true, "link_min":1, "link_max":1}, "embedded_core":{"version":"0.1.0",
            "manifest_sha256":pin.core_manifest_sha256, "assets":pin.core_assets,
            "api":1, "link":1},
            "protocols":{"metadata":"sidevoice-metadata-v1", "progress":"sidevoice-progress-jsonl-v1"}});
        pin.verify_metadata(&version, &metadata).unwrap();
        let mut wrong_format = version.clone();
        wrong_format["format"] = Value::String("esm".into());
        assert_eq!(pin.verify_metadata(&wrong_format, &metadata).unwrap_err().key, "install.pin-mismatch");
        let mut wrong_sea_flag = metadata.clone();
        wrong_sea_flag["connector"]["sea"] = Value::Bool(false);
        assert_eq!(pin.verify_metadata(&version, &wrong_sea_flag).unwrap_err().key, "install.pin-mismatch");
        let mut wrong = metadata;
        wrong["embedded_core"]["manifest_sha256"] = Value::String("0".repeat(64));
        assert_eq!(pin.verify_metadata(&version, &wrong).unwrap_err().key, "install.pin-mismatch");
    }

    #[test]
    fn release_record_projection_does_not_include_the_install_command() {
        let installed = InstalledBuild::from_release_record(
            &serde_json::json!({"id":"1.0.0", "connector":"1.0.0", "connector_sha":"e".repeat(40), "core":"0.8.0",
                "core_build":"build-17", "runtime_kind":"rust-native-v1", "pair_id":"pair-v1:rust-native-v1:test",
                "channel":"release", "build_seq":12, "command":["/private/path"]}),
            Some(1),
            Some(1),
        );
        let value = serde_json::to_value(installed).unwrap();
        assert!(value.get("command").is_none());
        assert_eq!(value["connectorVersion"], "1.0.0");
        assert_eq!(value["connectorSha"], "e".repeat(40));
        assert_eq!(value["coreBuild"], "build-17");
        assert_eq!(value["runtimeKind"], "rust-native-v1");
        assert_eq!(value["pairId"], "pair-v1:rust-native-v1:test");
    }
}
