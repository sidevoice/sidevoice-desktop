//! The model catalog's shape, and which of its models this device can run (docs/ENGINES.md).
//!
//! One catalog for every client (browser, desktop app, node, later mobile). An **engine** is a runtime that
//! runs models of some formats: either in the page (`runs: browser`, e.g. transformers.js) or as a package
//! downloaded at run time for one OS/architecture (`runs: native`, e.g. sherpa-onnx). A **model** is abstract
//! (Whisper tiny, Kokoro 82M) and says, per engine it runs on, which files and configuration that engine needs
//! — so bring-your-own-model is one more model entry in a format some engine reads. Each client detects its
//! capabilities and offers only what fits; picking an offer downloads the engine package (if missing) and the
//! model files.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// In the page itself (the web interface's workers): nothing to download but the model.
    Browser,
    /// A package for this OS/architecture, downloaded at run time and run by the client (or a node).
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
    /// `aarch64`, `x86_64` (Rust's `std::env::consts::ARCH`).
    pub arch: String,
    /// Capabilities the package cannot run without (e.g. `cuda` for a CUDA build).
    #[serde(default)]
    pub requires: Vec<Capability>,
    /// What it can use when present, best first (e.g. `coreml`, `cpu`).
    #[serde(default)]
    pub accelerators: Vec<Capability>,
    pub download: Download,
    /// Shared libraries to load, in order, relative to the unpacked root.
    #[serde(default)]
    pub libraries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Engine {
    pub id: String,
    pub label: String,
    pub version: String,
    pub runs: Runs,
    /// Model formats it reads: `onnx`, `ggml`, `mlx`…
    pub formats: Vec<String>,
    pub tasks: Vec<Task>,
    /// For `runs: browser`, what the page needs (any one of them).
    #[serde(default)]
    pub needs_any: Vec<Capability>,
    /// For `runs: native`, one per platform build.
    #[serde(default)]
    pub packages: Vec<Package>,
    pub license: String,
    pub homepage: String,
}

/// How one model runs on one engine: its files in that engine's format, and the engine's configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Build {
    pub format: String,
    /// Capabilities this build needs beyond the engine's own (e.g. a WebGPU-only quantization).
    #[serde(default)]
    pub requires: Vec<Capability>,
    /// Files a native engine downloads; a browser engine fetches its own (`config` says from where).
    #[serde(default)]
    pub download: Option<Download>,
    /// Engine-specific: file names inside the download, a repository id, options.
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voice {
    pub id: String,
    /// The engine's speaker index.
    pub sid: i32,
    /// BCP-47-ish language (`es`, `en-us`…) and, where it differs, what the engine's phonemizer calls it.
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub task: Task,
    /// Languages it handles; `multi` for "many" (Whisper).
    pub languages: Vec<String>,
    /// Engine id → how it runs there.
    pub builds: BTreeMap<String, Build>,
    #[serde(default)]
    pub voices: Vec<Voice>,
    pub license: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u32,
    pub engines: Vec<Engine>,
    pub models: Vec<Model>,
}

/// What this device is and offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub os: String,
    pub arch: String,
    pub capabilities: Vec<Capability>,
}

impl Device {
    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// This native process's device: OS and architecture as compiled, and what they imply. Apple Silicon always has
/// Metal, Core ML (with the Neural Engine) and can run MLX; nothing here probes for CUDA.
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
    Device { os, arch, capabilities }
}

/// One thing a person can pick: a model on an engine, runnable on this device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub model: String,
    pub engine: String,
    pub task: Task,
    pub label: String,
    pub languages: Vec<String>,
    pub runs: Runs,
    /// Bytes to download before first use: the engine package (if native) plus the model's files.
    pub download_size: u64,
    /// What the engine will use on this device, best first.
    pub accelerators: Vec<Capability>,
}

/// The engine package for this device, if the engine has one it can run.
pub fn package_for<'a>(engine: &'a Engine, device: &Device) -> Option<&'a Package> {
    engine
        .packages
        .iter()
        .find(|p| p.os == device.os && p.arch == device.arch && p.requires.iter().all(|c| device.has(*c)))
}

fn engine_fits(engine: &Engine, device: &Device) -> bool {
    match engine.runs {
        Runs::Native => package_for(engine, device).is_some(),
        Runs::Browser => engine.needs_any.iter().any(|c| device.has(*c)),
    }
}

/// Every (model, engine) this device can run, in catalog order. A browser engine needs a device that says what
/// its page has (`wasm`, `webgpu`); a native client merges the page's capabilities into its own before asking.
pub fn offers(catalog: &Catalog, device: &Device) -> Vec<Offer> {
    let mut out = Vec::new();
    for model in &catalog.models {
        for (engine_id, build) in &model.builds {
            let Some(engine) = catalog.engines.iter().find(|e| &e.id == engine_id) else { continue };
            if !engine.tasks.contains(&model.task) || !engine.formats.contains(&build.format) {
                continue;
            }
            if !engine_fits(engine, device) || !build.requires.iter().all(|c| device.has(*c)) {
                continue;
            }
            let package = package_for(engine, device);
            let accelerators = match package {
                Some(p) => p.accelerators.iter().copied().filter(|c| device.has(*c)).collect(),
                None => engine.needs_any.iter().copied().filter(|c| device.has(*c)).collect(),
            };
            out.push(Offer {
                model: model.id.clone(),
                engine: engine.id.clone(),
                task: model.task,
                label: model.label.clone(),
                languages: model.languages.clone(),
                runs: engine.runs,
                download_size: package.map(|p| p.download.size).unwrap_or(0)
                    + build.download.as_ref().map(|d| d.size).unwrap_or(0),
                accelerators,
            });
        }
    }
    out
}

/// The catalog this app ships (docs/ENGINES.md: until nodes serve the shared one).
pub const BUNDLED_CATALOG: &str = include_str!("../../../catalog/engines.json");

pub fn bundled_catalog() -> Catalog {
    serde_json::from_str(BUNDLED_CATALOG).expect("the bundled catalog parses (tested)")
}

/// Problems a catalog must not have: unknown engines, missing downloads or hashes for native builds.
pub fn check(catalog: &Catalog) -> Vec<String> {
    let mut problems = Vec::new();
    let hex = |s: &str| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit());
    for engine in &catalog.engines {
        for p in &engine.packages {
            if !hex(&p.download.sha256) || !p.download.url.starts_with("https://") {
                problems.push(format!("{} {}/{}: download needs https and a sha256", engine.id, p.os, p.arch));
            }
        }
        if engine.runs == Runs::Native && engine.packages.is_empty() {
            problems.push(format!("{}: native engine without packages", engine.id));
        }
    }
    for model in &catalog.models {
        for (engine_id, build) in &model.builds {
            let Some(engine) = catalog.engines.iter().find(|e| &e.id == engine_id) else {
                problems.push(format!("{}: unknown engine {engine_id}", model.id));
                continue;
            };
            if !engine.formats.contains(&build.format) {
                problems.push(format!("{} on {engine_id}: format {} not read", model.id, build.format));
            }
            if engine.runs == Runs::Native {
                match &build.download {
                    Some(d) if hex(&d.sha256) && d.url.starts_with("https://") => {}
                    _ => problems
                        .push(format!("{} on {engine_id}: native build needs an https download with sha256", model.id)),
                }
            }
        }
        if model.task == Task::Tts && model.voices.is_empty() {
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
            os: "macos".into(),
            arch: "aarch64".into(),
            capabilities: vec![Capability::Cpu, Capability::Coreml, Capability::Metal, Capability::Mlx],
        }
    }

    #[test]
    fn the_bundled_catalog_is_sound() {
        let catalog = bundled_catalog();
        assert_eq!(check(&catalog), Vec::<String>::new());
        assert!(catalog.models.iter().any(|m| m.task == Task::Stt));
        assert!(catalog.models.iter().any(|m| m.task == Task::Tts && m.voices.iter().any(|v| v.language == "es")));
    }

    #[test]
    fn an_apple_silicon_mac_gets_the_native_engine_and_no_browser_models_without_a_page() {
        let offers = offers(&bundled_catalog(), &mac());
        assert!(!offers.is_empty());
        assert!(offers.iter().all(|o| o.runs == Runs::Native && o.engine == "sherpa-onnx"));
        let tiny = offers.iter().find(|o| o.model == "whisper-tiny").unwrap();
        assert!(tiny.download_size > 100_000_000, "engine package + model");
        assert_eq!(tiny.accelerators, vec![Capability::Coreml, Capability::Cpu]);
    }

    #[test]
    fn a_page_s_capabilities_bring_the_browser_engine() {
        let mut device = mac();
        device.capabilities.push(Capability::Wasm);
        let offers = offers(&bundled_catalog(), &device);
        assert!(offers.iter().any(|o| o.runs == Runs::Browser && o.model == "whisper-tiny"));
        assert!(
            !offers.iter().any(|o| o.runs == Runs::Browser && o.model == "whisper-large-v3-turbo"),
            "a WebGPU-only build needs WebGPU"
        );
        device.capabilities.extend([Capability::Webgpu, Capability::WebgpuF16]);
        assert!(offers_of(&device).iter().any(|o| o.runs == Runs::Browser && o.model == "whisper-large-v3-turbo"));
    }

    fn offers_of(device: &Device) -> Vec<Offer> {
        offers(&bundled_catalog(), device)
    }

    #[test]
    fn other_platforms_get_nothing_native_tonight() {
        for (os, arch) in [("linux", "x86_64"), ("windows", "x86_64"), ("macos", "x86_64")] {
            let device = Device { os: os.into(), arch: arch.into(), capabilities: vec![Capability::Cpu] };
            assert!(offers_of(&device).is_empty(), "{os}/{arch}");
        }
    }

    #[test]
    fn requirements_are_enforced() {
        let mut catalog = bundled_catalog();
        catalog.engines[0].packages[0].requires = vec![Capability::Cuda];
        assert!(offers(&catalog, &mac()).iter().all(|o| o.runs != Runs::Native), "a CUDA build on a Mac: no");
    }

    #[test]
    fn check_catches_broken_entries() {
        let mut catalog = bundled_catalog();
        let build = catalog.models[0].builds.values().next().unwrap().clone();
        catalog.models[0].builds.insert("nope".into(), build);
        catalog.engines[0].packages[0].download.sha256 = "short".into();
        let problems = check(&catalog);
        assert!(problems.iter().any(|p| p.contains("unknown engine nope")));
        assert!(problems.iter().any(|p| p.contains("sha256")));
    }

    #[test]
    fn native_device_is_this_one() {
        let device = native_device();
        assert_eq!(device.os, std::env::consts::OS);
        assert!(device.has(Capability::Cpu));
    }
}
