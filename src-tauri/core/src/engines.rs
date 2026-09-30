//! The model catalog's shape, and which of its models this device can run (docs/ENGINES.md).
//!
//! One catalog for every client (browser, desktop app, host, later mobile), owned by `sidevoice/sidevoice-core`
//! (rubasace/sidevoice#124 §3): `catalog/engines.json` is a copy generated from it, never edited here. An
//! **engine** loads a build and runs it: in a page (`runs: page`, transformers.js) or as a package downloaded at
//! run time for one OS/architecture (`runs: native`, sherpa-onnx). A **family** (whisper, kokoro) says the task
//! and the options; a **model** (Whisper tiny, Kokoro 82M) has one **build** per engine it runs on: its files in
//! that engine's format and the engine's configuration. `offers()` is the resolver (§4); the core's Python and
//! the web's TypeScript implement the same function, and all three pass the core's `catalog/vectors.json`.

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
    /// Per platform (`macos-aarch64`, `page`): lower wins over the catalog's default engine order.
    #[serde(default)]
    pub rank: BTreeMap<String, u32>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ranking {
    /// Which engine wins when several builds of a model fit, best first.
    #[serde(default)]
    pub default: Vec<String>,
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
    #[serde(default)]
    pub ranking: Ranking,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
}

impl Device {
    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    fn platform(&self) -> String {
        match self.runs {
            Runs::Native => format!("{}-{}", self.os, self.arch),
            Runs::Page => "page".into(),
        }
    }
}

/// This native process's device: OS and architecture as compiled, and what they imply. Apple Silicon always has
/// Metal, Core ML (with the Neural Engine) and can run MLX; nothing here probes for CUDA or memory.
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

/// Places a model runs in besides a provider: this client, or the host it is paired with.
pub const PLACES: &[&str] = &["device", "host"];

/// A build and the accelerator it runs with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub engine: String,
    pub accelerator: Capability,
}

/// One model this place can run, on its best build, with every other build × accelerator ranked behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub model: String,
    pub task: Task,
    pub engine: String,
    pub accelerator: Capability,
    /// Bytes the first use downloads: the native engine's package unless bundled, plus the build's own files.
    pub download_size: u64,
    /// For diagnostics: why this build and accelerator.
    pub reason: String,
    /// What *Avanzado* offers instead, best first.
    pub alternatives: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownPlace(pub String);

/// The engine package for this device, if the engine has one it can run.
pub fn package_for<'a>(engine: &'a Engine, device: &Device) -> Option<&'a Package> {
    engine.packages.iter().find(|p| {
        p.os == device.os
            && p.arch.as_deref().map_or(true, |arch| arch == device.arch)
            && p.requires.iter().all(|c| device.has(*c))
    })
}

/// (accelerators best first, package) when `build` runs on this device; None when it does not.
fn fit<'a>(build: &Build, engine: &'a Engine, device: &Device) -> Option<(Vec<Capability>, Option<&'a Package>)> {
    // A page engine only in a page, a native one only in a native runtime (D5).
    if engine.runs != device.runs {
        return None;
    }
    let (usable, package) = match device.runs {
        Runs::Native => {
            let package = package_for(engine, device)?;
            (&package.accelerators, Some(package))
        }
        Runs::Page => (&engine.accelerators, None),
    };
    if !build.needs.iter().all(|c| device.has(*c)) {
        return None;
    }
    let accelerators: Vec<Capability> = build
        .accelerators
        .as_ref()
        .unwrap_or(usable)
        .iter()
        .copied()
        .filter(|c| usable.contains(c) && device.has(*c))
        .collect();
    (!accelerators.is_empty()).then_some((accelerators, package))
}

fn size_of(build: &Build, package: Option<&Package>) -> u64 {
    let engine = match package {
        Some(p) if !p.bundled => p.download.as_ref().map(|d| d.size).unwrap_or(0),
        _ => 0,
    };
    engine + build.download.as_ref().map(|d| d.size).unwrap_or(0)
}

/// One offer per model `place` can run on this device, in catalog order (rubasace/sidevoice#124 §4). A provider's
/// models are the provider's to list, so a provider place gets none here; an unknown place is refused.
pub fn offers(catalog: &Catalog, device: &Device, place: &str) -> Result<Vec<Offer>, UnknownPlace> {
    if !PLACES.contains(&place) {
        if catalog.providers.iter().any(|p| p.id == place) {
            return Ok(Vec::new());
        }
        return Err(UnknownPlace(format!("unknown place {place:?}")));
    }
    let order = &catalog.ranking.default;
    let platform = device.platform();
    let mut out = Vec::new();
    for model in &catalog.models {
        if let (Some(needed), Some(memory)) = (model.requires.memory_mb, device.memory_mb) {
            if memory < needed {
                continue;
            }
        }
        let Some(task) = catalog.task_of(model) else { continue };
        let mut fitting: Vec<(usize, &Build, Vec<Capability>, Option<&Package>)> = model
            .builds
            .iter()
            .enumerate()
            .filter_map(|(index, build)| {
                let (accelerators, package) = fit(build, catalog.engine(&build.engine)?, device)?;
                Some((index, build, accelerators, package))
            })
            .collect();
        if fitting.is_empty() {
            continue;
        }
        fitting.sort_by_key(|(index, build, _, _)| {
            let own = build.rank.get(&platform).copied();
            let default = order.iter().position(|e| *e == build.engine).unwrap_or(order.len());
            (own.is_none(), own.unwrap_or(0), default, *index)
        });
        let (_, best, accelerators, package) = &fitting[0];
        let why = if fitting.len() == 1 {
            "the only build that runs here".to_string()
        } else if best.rank.contains_key(&platform) {
            format!("this model ranks it first on {platform}")
        } else {
            "first in the catalogue's engine order".to_string()
        };
        let accelerator = accelerators[0];
        let pairs: Vec<Choice> = fitting
            .iter()
            .flat_map(|(_, build, usable, _)| {
                usable.iter().map(|a| Choice { engine: build.engine.clone(), accelerator: *a })
            })
            .collect();
        out.push(Offer {
            model: model.id.clone(),
            task,
            engine: best.engine.clone(),
            accelerator,
            download_size: size_of(best, *package),
            reason: format!("{} ({}): {why}", best.engine, capability_name(accelerator)),
            alternatives: pairs[1..].to_vec(),
        });
    }
    Ok(out)
}

/// A capability as the catalog writes it.
pub fn capability_name(capability: Capability) -> String {
    serde_json::to_value(capability).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
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
    for id in &catalog.ranking.default {
        if catalog.engine(id).is_none() {
            problems.push(format!("ranking: unknown engine {id}"));
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

    /// sidevoice-core's shared resolver vectors (a copy, like the catalog): capabilities in, offers out.
    const VECTORS: &str = include_str!("../../../catalog/vectors.json");

    #[derive(Deserialize)]
    struct Vectors {
        fixture: Catalog,
        vectors: Vec<Vector>,
    }

    #[derive(Deserialize)]
    struct Vector {
        name: String,
        catalog: String,
        capabilities: Device,
        place: String,
        #[serde(default)]
        offers: Option<Vec<Offer>>,
        #[serde(default)]
        error: Option<String>,
    }

    fn mac() -> Device {
        Device {
            runs: Runs::Native,
            os: "macos".into(),
            arch: "aarch64".into(),
            capabilities: vec![Capability::Cpu, Capability::Coreml, Capability::Metal, Capability::Mlx],
            memory_mb: None,
        }
    }

    fn offers_of(device: &Device) -> Vec<Offer> {
        offers(&bundled_catalog(), device, "device").unwrap()
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
    fn the_shared_vectors() {
        let shared: Vectors = serde_json::from_str(VECTORS).expect("the vectors parse");
        assert_eq!(check(&shared.fixture), Vec::<String>::new(), "the fixture is a sound catalog itself");
        let shipped = bundled_catalog();
        for vector in &shared.vectors {
            let catalog = if vector.catalog == "fixture" { &shared.fixture } else { &shipped };
            let got = offers(catalog, &vector.capabilities, &vector.place);
            match (&vector.error, &vector.offers) {
                (Some(_), _) => assert!(got.is_err(), "{}: should be refused", vector.name),
                (None, Some(expected)) => assert_eq!(got.as_ref().unwrap(), expected, "{}", vector.name),
                (None, None) => panic!("{}: neither offers nor an error", vector.name),
            }
        }
        assert!(
            shared.vectors.iter().any(|v| v.catalog == "fixture") && shared.vectors.iter().any(|v| v.error.is_some())
        );
    }

    #[test]
    fn an_apple_silicon_mac_gets_the_native_engine_and_no_page_models() {
        let offers = offers_of(&mac());
        assert_eq!(offers.len(), 5);
        assert!(offers.iter().all(|o| o.engine == "sherpa-onnx"));
        let tiny = offers.iter().find(|o| o.model == "whisper-tiny").unwrap();
        assert!(tiny.download_size > 100_000_000, "engine package + model");
        assert_eq!(tiny.accelerator, Capability::Cpu, "Core ML measured slower for these models");
        assert_eq!(tiny.alternatives, vec![Choice { engine: "sherpa-onnx".into(), accelerator: Capability::Coreml }]);
    }

    #[test]
    fn a_page_gets_the_page_engine_and_webgpu_only_builds_need_webgpu() {
        let page = |has: Vec<Capability>| Device {
            runs: Runs::Page,
            os: String::new(),
            arch: String::new(),
            capabilities: has,
            memory_mb: None,
        };
        let wasm = offers_of(&page(vec![Capability::Wasm]));
        assert!(wasm.iter().all(|o| o.engine == "transformers-js" && o.accelerator == Capability::Wasm));
        assert!(!wasm.iter().any(|o| o.model == "whisper-large-v3-turbo"), "a WebGPU-only build needs WebGPU");
        let full = offers_of(&page(vec![Capability::Webgpu, Capability::WebgpuF16, Capability::Wasm]));
        assert!(full.iter().any(|o| o.model == "whisper-large-v3-turbo" && o.accelerator == Capability::Webgpu));
    }

    #[test]
    fn other_platforms_get_nothing_native_tonight() {
        for (os, arch) in [("linux", "x86_64"), ("windows", "x86_64"), ("macos", "x86_64")] {
            let device = Device {
                runs: Runs::Native,
                os: os.into(),
                arch: arch.into(),
                capabilities: vec![Capability::Cpu],
                memory_mb: None,
            };
            assert!(offers_of(&device).is_empty(), "{os}/{arch}");
        }
    }

    #[test]
    fn requirements_are_enforced() {
        let mut catalog = bundled_catalog();
        catalog.engines[0].packages[0].requires = vec![Capability::Cuda];
        assert!(offers(&catalog, &mac(), "device").unwrap().is_empty(), "a CUDA build on a Mac: no");
    }

    #[test]
    fn a_capability_this_app_does_not_know_is_never_had() {
        let catalog: Catalog =
            serde_json::from_str(&BUNDLED_CATALOG.replacen("\"webgpu-f16\"", "\"webgpu-f32-someday\"", 1)).unwrap();
        assert!(catalog.models.iter().any(|m| m.builds.iter().any(|b| b.needs.contains(&Capability::Unknown))));
        let page = Device {
            runs: Runs::Page,
            os: String::new(),
            arch: String::new(),
            capabilities: vec![Capability::Webgpu, Capability::WebgpuF16],
            memory_mb: None,
        };
        assert!(!offers(&catalog, &page, "device").unwrap().iter().any(|o| o.model == "whisper-large-v3-turbo"));
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
    fn an_unknown_place_is_refused_and_a_provider_offers_nothing_here() {
        assert!(offers(&bundled_catalog(), &mac(), "nowhere").is_err());
        assert_eq!(offers(&bundled_catalog(), &mac(), "openai"), Ok(Vec::new()));
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
