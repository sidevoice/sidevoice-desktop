//! The model catalog's shape, and whether one of its native builds runs on this device (docs/ENGINES.md).
//!
//! One catalog for every client (browser, desktop app, host, later mobile), owned by `sidevoice/sidevoice-core`
//! (rubasace/sidevoice#124 §3): `catalog/engines.json` is a copy generated from it, never edited here. An
//! **engine** loads a build and runs it: in a page (`runs: page`, transformers.js) or as a package downloaded at
//! run time for one OS/architecture (`runs: native`, sherpa-onnx). A **family** (whisper, kokoro) says the task
//! and the options; a **model** (Whisper tiny, Kokoro 82M) has one **build** per engine it runs on: its files in
//! that engine's format and the engine's configuration. Choosing among models is the resolver's (§4): the web's
//! TypeScript for a client, the core's Python for a host. This app has no resolver; it only checks, at its trust
//! boundary, that the build a page asks for runs here (`accelerators_for`).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What a device can offer an engine. Serialized lowercase (`"webgpu"`, `"coreml"`…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    /// Plain CPU execution (every native device).
    Cpu,
    /// WebAssembly in a page.
    Wasm,
    /// WebGPU in a page.
    Webgpu,
    /// WebGPU with `shader-f16`.
    #[serde(rename = "webgpu-f16")]
    WebgpuF16,
    /// Apple GPU (Apple Silicon Macs, iPhones).
    Metal,
    /// Core ML (and with it the Neural Engine on Apple Silicon).
    Coreml,
    /// Apple's MLX (Apple Silicon, macOS 13.5+).
    Mlx,
    /// NVIDIA CUDA.
    Cuda,
    /// One a newer catalog names and this app does not know: no device of ours has it, so nothing needing it fits.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Task {
    Stt,
    Tts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runs {
    /// In a web page (the web interface's workers): nothing to download but the model.
    Page,
    /// A package for this OS/architecture, downloaded at run time (or bundled in a mobile app) and run natively.
    Native,
}

/// A file to download: where, how big, its SHA-256, and how it is packed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// `tar.bz2` today.
    pub archive: String,
    /// The directory the archive unpacks into (everything else in it is ignored).
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    /// `macos`, `linux`, `windows`, `ios`, `android` (Rust's `std::env::consts::OS`).
    pub os: String,
    /// `aarch64`, `x86_64` (Rust's `std::env::consts::ARCH`). Absent on a bundled package: every architecture.
    #[serde(default)]
    pub arch: Option<String>,
    /// Capabilities the package cannot run without (e.g. `cuda` for a CUDA build).
    #[serde(default)]
    pub requires: Vec<Capability>,
    /// What it can use when present, best first.
    #[serde(default)]
    pub accelerators: Vec<Capability>,
    /// Shipped inside the app (iOS, Android: the stores forbid downloading code); nothing to download.
    #[serde(default)]
    pub bundled: bool,
    #[serde(default)]
    pub download: Option<Download>,
    /// Shared libraries to load, in order, relative to the unpacked root, each with its own SHA-256: a library is
    /// hashed again right before it is loaded, so a file swapped on disk after the download is never run.
    #[serde(default)]
    pub libraries: Vec<Library>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Engine {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub version: String,
    pub runs: Runs,
    /// Families it has the pipeline for (features, decoding, phonemizer): `whisper`, `kokoro`…
    pub families: Vec<String>,
    /// Model formats it reads: `onnx`, `mlx`…
    pub formats: Vec<String>,
    /// For `runs: page`, what a page may offer it, best first.
    #[serde(default)]
    pub accelerators: Vec<Capability>,
    /// For `runs: native`, one per platform build.
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub homepage: String,
}

/// How one model runs on one engine: its files in that engine's format, and the engine's configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Build {
    pub engine: String,
    pub format: String,
    /// The accelerators this build is limited to, best first; the engine's (or its package's) when absent.
    #[serde(default)]
    pub accelerators: Option<Vec<Capability>>,
    /// Capabilities it needs beyond an accelerator (e.g. `webgpu-f16` for a half-precision quantization).
    #[serde(default)]
    pub needs: Vec<Capability>,
    /// Files a native engine downloads; a page engine fetches its own (`config` says from where).
    #[serde(default)]
    pub download: Option<Download>,
    /// Engine-specific: file names inside the download, a repository id, options.
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voice {
    pub id: String,
    /// BCP-47-ish language (`es`, `en-us`…), as the engine's phonemizer names it.
    pub language: String,
    /// The engine's speaker index.
    pub sid: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requires {
    #[serde(default)]
    pub memory_mb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub family: String,
    #[serde(default)]
    pub label: String,
    /// Languages it handles; `multi` for "many" (Whisper).
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub requires: Requires,
    #[serde(default)]
    pub voices: Vec<Voice>,
    pub builds: Vec<Build>,
    #[serde(default)]
    pub license: String,
}

impl Model {
    /// Its build on `engine`, if it has one.
    pub fn build(&self, engine: &str) -> Option<&Build> {
        self.builds.iter().find(|b| b.engine == engine)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Family {
    pub task: Task,
    /// The options every model of the family takes, as the schema the web interface renders.
    #[serde(default)]
    pub options: Vec<serde_json::Value>,
}

/// An external service a host calls with its own key. The app never calls one; it only knows the place exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u32,
    pub engines: Vec<Engine>,
    pub families: BTreeMap<String, Family>,
    #[serde(default)]
    pub providers: Vec<Provider>,
    pub models: Vec<Model>,
}

impl Catalog {
    pub fn engine(&self, id: &str) -> Option<&Engine> {
        self.engines.iter().find(|e| e.id == id)
    }

    pub fn model(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }

    /// The task of a model, from its family.
    pub fn task_of(&self, model: &Model) -> Option<Task> {
        self.families.get(&model.family).map(|f| f.task)
    }
}

/// What a place reports about itself: a page or a native runtime, its OS and architecture (native), what it
/// has (accelerators and features alike: `webgpu-f16` is a feature a build may need) and, when known, memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub runs: Runs,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    #[serde(rename = "has")]
    pub capabilities: Vec<Capability>,
    /// Total memory; `null` when unknown.
    #[serde(default)]
    pub memory_mb: Option<u64>,
}

impl Device {
    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// This native process's device: OS and architecture as compiled, and what they imply. Apple Silicon always has
/// Metal, Core ML (with the Neural Engine) and can run MLX; nothing here probes for CUDA. Memory is the OS's to
/// report (the engine crate asks it); this pure logic leaves it unknown.
pub fn native_device() -> Device {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let mut capabilities = vec![Capability::Cpu];
    if os == "macos" {
        capabilities.push(Capability::Coreml);
        if arch == "aarch64" {
            capabilities.extend([Capability::Metal, Capability::Mlx]);
        }
    }
    Device { runs: Runs::Native, os, arch, capabilities, memory_mb: None }
}

/// Places a model runs in besides a provider: this client, or the host it is paired with. A provider id must not
/// be one (`check`).
pub const PLACES: &[&str] = &["device", "host"];

/// The engine package for this device, if the engine has one it can run.
pub fn package_for<'a>(engine: &'a Engine, device: &Device) -> Option<&'a Package> {
    engine.packages.iter().find(|p| {
        p.os == device.os
            && p.arch.as_deref().map_or(true, |arch| arch == device.arch)
            && p.requires.iter().all(|c| device.has(*c))
    })
}

/// The accelerators a native `build` can use on this native device, best first — the first is the one the
/// clients' resolvers pick; `None` when the build does not run here: not a native engine's, no package of it
/// for this OS/architecture, a feature it needs missing, or no accelerator both usable and present. The narrow
/// check the app makes at its trust boundary before it installs or runs what a page asks for; choosing among
/// models is the client's resolver (rubasace/sidevoice#124 §4), never the app's.
pub fn accelerators_for(catalog: &Catalog, build: &Build, device: &Device) -> Option<Vec<Capability>> {
    let engine = catalog.engine(&build.engine)?;
    if engine.runs != Runs::Native || device.runs != Runs::Native {
        return None;
    }
    let package = package_for(engine, device)?;
    if !build.needs.iter().all(|c| device.has(*c)) {
        return None;
    }
    let accelerators: Vec<Capability> = build
        .accelerators
        .as_ref()
        .unwrap_or(&package.accelerators)
        .iter()
        .copied()
        .filter(|c| package.accelerators.contains(c) && device.has(*c))
        .collect();
    (!accelerators.is_empty()).then_some(accelerators)
}

/// A capability as the catalog writes it.
pub fn capability_name(capability: Capability) -> String {
    serde_json::to_value(capability).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// The capability the catalog names `name`; `Unknown` (which no device has) for a name this app does not know.
pub fn capability_named(name: &str) -> Capability {
    serde_json::from_value(serde_json::Value::String(name.to_string())).unwrap_or(Capability::Unknown)
}

/// Whisper's language codes (openai/whisper `tokenizer.LANGUAGES`). sherpa-onnx ends the whole process on a code
/// it does not know, so nothing else is ever handed to it.
pub const WHISPER_LANGUAGES: &[&str] = &[
    "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi",
    "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml",
    "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs",
    "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am",
    "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln",
    "ha", "ba", "jw", "su", "yue",
];

/// A language to hand Whisper: its own code, or empty for "detect it". Anything else is refused here.
pub fn whisper_language(language: &str) -> Option<&str> {
    let language = language.trim();
    (language.is_empty() || WHISPER_LANGUAGES.contains(&language)).then_some(language)
}

/// The catalog this app ships: sidevoice-core's, copied by `scripts/copy-core-catalog.mjs` (docs/ENGINES.md).
pub const BUNDLED_CATALOG: &str = include_str!("../../../catalog/engines.json");

pub fn bundled_catalog() -> Catalog {
    serde_json::from_str(BUNDLED_CATALOG).expect("the bundled catalog parses (tested)")
}

/// Option kinds the web interface knows how to render; our catalog refuses any other.
pub const OPTION_KINDS: &[&str] = &["language", "text", "voice", "range"];

/// Problems a catalog must not have: unknown engines or families, builds an engine cannot run, missing downloads
/// or hashes for native builds, unknown option kinds. The core's validator is the reference; this is its port.
pub fn check(catalog: &Catalog) -> Vec<String> {
    let mut problems = Vec::new();
    let hex = |s: &str| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit());
    let download_ok = |d: &Option<Download>| {
        d.as_ref().is_some_and(|d| hex(&d.sha256) && d.url.starts_with("https://") && d.size > 0)
    };
    if catalog.version != 2 {
        problems.push("version must be 2".to_string());
    }
    let mut engines = BTreeSet::new();
    for engine in &catalog.engines {
        if !engines.insert(engine.id.as_str()) {
            problems.push(format!("{}: engine listed twice", engine.id));
        }
        for p in &engine.packages {
            let arch = p.arch.as_deref().unwrap_or("*");
            if p.accelerators.is_empty() {
                problems.push(format!("{} {}/{arch}: a package lists no accelerators", engine.id, p.os));
            }
            if p.bundled {
                continue;
            }
            if p.arch.is_none() {
                problems.push(format!("{} {}/{arch}: a downloaded package names its architecture", engine.id, p.os));
            }
            if !download_ok(&p.download) {
                problems.push(format!("{} {}/{arch}: download needs https and a sha256", engine.id, p.os));
            }
            for library in &p.libraries {
                if !hex(&library.sha256) || library.path.contains("..") || library.path.starts_with('/') {
                    problems.push(format!(
                        "{} {}/{arch}: library {} needs a sha256 and a relative path",
                        engine.id, p.os, library.path
                    ));
                }
            }
        }
        match engine.runs {
            Runs::Native if engine.packages.is_empty() => {
                problems.push(format!("{}: native engine without packages", engine.id))
            }
            Runs::Page if engine.accelerators.is_empty() => {
                problems.push(format!("{}: a page engine lists no accelerators", engine.id))
            }
            _ => {}
        }
    }
    for (name, family) in &catalog.families {
        for option in &family.options {
            let kind = option.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            if !OPTION_KINDS.contains(&kind) {
                problems.push(format!("family {name}: option has an unknown kind {kind:?}"));
            }
        }
    }
    for provider in &catalog.providers {
        if PLACES.contains(&provider.id.as_str()) {
            problems.push(format!("{}: provider id must be unique and not a place", provider.id));
        }
    }
    let mut models = BTreeSet::new();
    for model in &catalog.models {
        if !models.insert(model.id.as_str()) {
            problems.push(format!("{}: model listed twice", model.id));
        }
        let Some(family) = catalog.families.get(&model.family) else {
            problems.push(format!("{}: unknown family {}", model.id, model.family));
            continue;
        };
        let mut on = BTreeSet::new();
        for build in &model.builds {
            let Some(engine) = catalog.engine(&build.engine) else {
                problems.push(format!("{}: unknown engine {}", model.id, build.engine));
                continue;
            };
            if !on.insert(build.engine.as_str()) {
                problems.push(format!("{}: two builds on {}", model.id, build.engine));
            }
            if !engine.families.contains(&model.family) {
                problems.push(format!(
                    "{} on {}: the engine does not run the {} family",
                    model.id, engine.id, model.family
                ));
            }
            if !engine.formats.contains(&build.format) {
                problems.push(format!("{} on {}: format {} not read", model.id, engine.id, build.format));
            }
            if engine.runs == Runs::Native && !download_ok(&build.download) {
                problems
                    .push(format!("{} on {}: native build needs an https download with sha256", model.id, engine.id));
            }
            let usable: BTreeSet<Capability> = engine
                .accelerators
                .iter()
                .chain(engine.packages.iter().flat_map(|p| &p.accelerators))
                .copied()
                .collect();
            if !build.accelerators.iter().flatten().all(|c| usable.contains(c)) {
                problems.push(format!("{} on {}: an accelerator the engine never uses", model.id, engine.id));
            }
        }
        if model.builds.is_empty() {
            problems.push(format!("{}: no builds", model.id));
        }
        let voice_options =
            family.options.iter().any(|o| o.get("from").and_then(|f| f.as_str()) == Some("model.voices"));
        if voice_options && model.voices.is_empty() {
            problems.push(format!("{}: a voice model lists no voices", model.id));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac() -> Device {
        Device {
            runs: Runs::Native,
            os: "macos".into(),
            arch: "aarch64".into(),
            capabilities: vec![Capability::Cpu, Capability::Coreml, Capability::Metal, Capability::Mlx],
            memory_mb: None,
        }
    }

    fn build<'a>(catalog: &'a Catalog, model: &str, engine: &str) -> &'a Build {
        catalog.model(model).unwrap().build(engine).unwrap()
    }

    #[test]
    fn the_bundled_catalog_is_sound() {
        let catalog = bundled_catalog();
        assert_eq!(check(&catalog), Vec::<String>::new());
        assert!(catalog.models.iter().any(|m| catalog.task_of(m) == Some(Task::Stt)));
        let kokoro = catalog.model("kokoro-82m-v1.0").unwrap();
        assert_eq!(catalog.task_of(kokoro), Some(Task::Tts));
        assert!(kokoro.voices.iter().any(|v| v.language == "es"));
    }

    #[test]
    fn a_native_build_runs_on_an_apple_silicon_mac_with_its_package_accelerators_best_first() {
        let catalog = bundled_catalog();
        let tiny = build(&catalog, "whisper-tiny", "sherpa-onnx");
        assert_eq!(accelerators_for(&catalog, tiny, &mac()), Some(vec![Capability::Cpu, Capability::Coreml]));
        let mut no_coreml = mac();
        no_coreml.capabilities.retain(|c| *c != Capability::Coreml);
        assert_eq!(accelerators_for(&catalog, tiny, &no_coreml), Some(vec![Capability::Cpu]), "only what is present");
    }

    #[test]
    fn a_page_build_never_runs_natively_nor_a_native_build_in_a_page() {
        let catalog = bundled_catalog();
        assert_eq!(accelerators_for(&catalog, build(&catalog, "whisper-tiny", "transformers-js"), &mac()), None);
        let page = Device { runs: Runs::Page, capabilities: vec![Capability::Wasm, Capability::Cpu], ..mac() };
        assert_eq!(accelerators_for(&catalog, build(&catalog, "whisper-tiny", "sherpa-onnx"), &page), None);
    }

    #[test]
    fn no_package_for_this_platform_means_it_does_not_run() {
        let catalog = bundled_catalog();
        let tiny = build(&catalog, "whisper-tiny", "sherpa-onnx");
        for (os, arch) in [("linux", "x86_64"), ("windows", "x86_64"), ("macos", "x86_64")] {
            let device = Device { os: os.into(), arch: arch.into(), capabilities: vec![Capability::Cpu], ..mac() };
            assert_eq!(accelerators_for(&catalog, tiny, &device), None, "{os}/{arch}");
        }
    }

    #[test]
    fn package_requirements_build_needs_and_build_accelerators_are_enforced() {
        let mut catalog = bundled_catalog();
        catalog.engines[0].packages[0].requires = vec![Capability::Cuda];
        assert_eq!(accelerators_for(&catalog, build(&catalog, "whisper-tiny", "sherpa-onnx"), &mac()), None, "CUDA");

        let mut catalog = bundled_catalog();
        let tiny = catalog.models.iter_mut().find(|m| m.id == "whisper-tiny").unwrap();
        tiny.builds.iter_mut().find(|b| b.engine == "sherpa-onnx").unwrap().needs = vec![Capability::Unknown];
        let needs_unknown = build(&catalog, "whisper-tiny", "sherpa-onnx");
        assert_eq!(accelerators_for(&catalog, needs_unknown, &mac()), None, "a capability this app does not know");

        let mut catalog = bundled_catalog();
        let tiny = catalog.models.iter_mut().find(|m| m.id == "whisper-tiny").unwrap();
        tiny.builds.iter_mut().find(|b| b.engine == "sherpa-onnx").unwrap().accelerators =
            Some(vec![Capability::Coreml]);
        let coreml_only = build(&catalog, "whisper-tiny", "sherpa-onnx");
        assert_eq!(accelerators_for(&catalog, coreml_only, &mac()), Some(vec![Capability::Coreml]));
    }

    #[test]
    fn capabilities_by_name() {
        assert_eq!(capability_named("coreml"), Capability::Coreml);
        assert_eq!(capability_named("webgpu-f16"), Capability::WebgpuF16);
        assert_eq!(capability_named("warp-drive"), Capability::Unknown);
        assert_eq!(capability_name(Capability::Coreml), "coreml");
    }

    #[test]
    fn check_catches_broken_entries() {
        let mut catalog = bundled_catalog();
        let mut build = catalog.models[0].builds[0].clone();
        build.engine = "nope".into();
        catalog.models[0].builds.push(build);
        catalog.engines[0].packages[0].download.as_mut().unwrap().sha256 = "short".into();
        catalog
            .families
            .get_mut("whisper")
            .unwrap()
            .options
            .push(serde_json::json!({"id": "mood", "kind": "colour-wheel"}));
        let problems = check(&catalog);
        assert!(problems.iter().any(|p| p.contains("unknown engine nope")));
        assert!(problems.iter().any(|p| p.contains("sha256")));
        assert!(problems.iter().any(|p| p.contains("unknown kind \"colour-wheel\"")));
    }

    #[test]
    fn a_build_must_suit_its_engine() {
        let mut catalog = bundled_catalog();
        catalog.engines[0].families.retain(|f| f != "kokoro");
        catalog.models[0].builds[1].accelerators = Some(vec![Capability::Cuda]);
        let problems = check(&catalog);
        assert!(problems.iter().any(|p| p.contains("does not run the kokoro family")));
        assert!(problems.iter().any(|p| p.contains("never uses")));
    }

    #[test]
    fn a_library_without_its_hash_is_refused() {
        let mut catalog = bundled_catalog();
        catalog.engines[0].packages[0].libraries[0].sha256 = String::new();
        catalog.engines[0].packages[0].libraries[1].path = "../evil.dylib".into();
        assert_eq!(check(&catalog).iter().filter(|p| p.contains("library")).count(), 2);
    }

    #[test]
    fn whisper_languages_are_whisper_s_own() {
        assert_eq!(WHISPER_LANGUAGES.len(), 100);
        assert_eq!(whisper_language("es"), Some("es"));
        assert_eq!(whisper_language(""), Some(""), "empty asks Whisper to detect it");
        assert_eq!(whisper_language("auto"), None);
        assert_eq!(whisper_language("es-ES"), None);
        assert_eq!(whisper_language("xx"), None);
    }

    #[test]
    fn native_device_is_this_one() {
        let device = native_device();
        assert_eq!(device.os, std::env::consts::OS);
        assert_eq!(device.runs, Runs::Native);
        assert!(device.has(Capability::Cpu));
    }
}
