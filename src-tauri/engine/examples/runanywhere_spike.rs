//! SPIKE (spike/runanywhere, never merged): the bridge contract — capabilities / installed / install / load /
//! unload / transcribe / synthesize — run on RunAnywhere's RACommons v0.20.37 on Linux x86_64, with our catalogue
//! entries (URL + SHA-256), plus RunAnywhere's own download path fed the same entries. `baseline` runs the same
//! steps on our current sherpa-onnx adapter (upstream 1.13.8 Linux package) for comparison.
//!
//!   cargo run --release -p sidevoice-desktop-engine --example runanywhere_spike -- \
//!     src-tauri/engine/examples/runanywhere-catalog.json <store dir> <runanywhere-0.20.37-linux-x86_64.tar.bz2> \
//!     [runanywhere|baseline]
//!
//! The RunAnywhere engine package is not hosted anywhere (this is a spike): the third argument is the locally built
//! tarball, unpacked into the store as if downloaded. Models and the sherpa-onnx package are downloaded for real by
//! our installer.
use sidevoice_desktop_core::engines::{Capability, Catalog};
use sidevoice_desktop_engine::{runanywhere, NativeEngines};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn rss_mb() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb: f64 = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    kb / 1024.0
}

fn seed_runanywhere(catalog: &Catalog, store: &Path, tarball: &Path) {
    let engine = catalog.engine("runanywhere").expect("the spike catalogue names runanywhere");
    let download = engine.packages[0].download.as_ref().unwrap();
    let slot = store.join("engines").join(format!("runanywhere-{}-linux-x86_64", engine.version));
    if slot.join(".sidevoice-complete").exists() {
        return;
    }
    std::fs::create_dir_all(&slot).unwrap();
    let file = std::fs::File::open(tarball).expect("engine tarball");
    tar::Archive::new(bzip2::read::BzDecoder::new(file)).unpack(&slot).expect("unpack engine");
    std::fs::write(slot.join(".sidevoice-complete"), &download.sha256).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (catalog, store, tarball) = (&args[1], PathBuf::from(&args[2]), &args[3]);
    let baseline = args.get(4).is_some_and(|m| m == "baseline");
    let (e, stt, tts, voice) = if baseline {
        ("sherpa-onnx", "whisper-tiny", "kokoro-82m-v1.0", "ef_dora")
    } else {
        ("runanywhere", "whisper-tiny", "piper-es-es-davefx-medium", "davefx")
    };
    let catalog: Catalog = serde_json::from_str(&std::fs::read_to_string(catalog).unwrap()).unwrap();
    let problems = sidevoice_desktop_core::engines::check(&catalog);
    assert!(problems.is_empty(), "catalogue: {problems:?}");
    seed_runanywhere(&catalog, &store, Path::new(tarball));
    let engines = NativeEngines::new(catalog, &store);
    println!("== engine {e}: STT {stt}, TTS {tts} ({voice})");
    println!("== capabilities: {}", serde_json::to_string(&engines.device).unwrap());
    println!("== installed (before): {}", serde_json::to_string(&engines.installed()).unwrap());

    for model in [stt, tts] {
        let started = Instant::now();
        let mut calls = 0;
        engines.install(model, e, &mut |_, _| calls += 1).unwrap_or_else(|err| panic!("install {model}: {err}"));
        println!("== install {model}: {:.1}s, {calls} progress calls", started.elapsed().as_secs_f32());
    }
    println!("== installed (after): {}", serde_json::to_string(&engines.installed()).unwrap());

    // The engine itself (dlopen + init), then each model: loading a model loads its engine first.
    let rss0 = rss_mb();
    let t = engines.load(tts, e, None, voice).unwrap_or_else(|err| panic!("load {tts}: {err}"));
    println!("== load {tts} (first: includes the engine's dlopen/init): {} ms, RSS {rss0:.0} → {:.0} MB", t.as_millis(), rss_mb());
    let rss1 = rss_mb();
    let s = engines.load(stt, e, None, "es").unwrap_or_else(|err| panic!("load {stt}: {err}"));
    println!("== load {stt}: {} ms, RSS {rss1:.0} → {:.0} MB", s.as_millis(), rss_mb());
    println!("== loaded: {}", serde_json::to_string(&engines.loaded()).unwrap());

    let sentence = "Hola, esto es una prueba de voz de Sidevoice.";
    let mut audio = None;
    for pass in ["warm-up", "measured"] {
        let started = Instant::now();
        let a = engines.synthesize(tts, e, None, voice, 1.0, sentence).expect("synthesize");
        let secs = a.samples.len() as f32 / a.sample_rate as f32;
        let peak = a.samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        let took = started.elapsed().as_secs_f32();
        println!("== synthesize ({pass}): {secs:.2}s audio @ {} Hz, peak {peak:.2}, in {took:.2}s (RTF {:.3})", a.sample_rate, took / secs);
        audio = Some(a);
    }
    let audio = audio.unwrap();
    // Whisper wants 16 kHz; give both engines the same 16 kHz input (linear resample) so only the engine differs.
    let ratio = audio.sample_rate as f64 / 16000.0;
    let resampled: Vec<f32> = (0..(audio.samples.len() as f64 / ratio) as usize)
        .map(|i| {
            let x = i as f64 * ratio;
            let (j, f) = (x as usize, (x.fract()) as f32);
            let a = audio.samples[j];
            let b = *audio.samples.get(j + 1).unwrap_or(&a);
            a + (b - a) * f
        })
        .collect();
    for pass in ["warm-up", "measured", "measured again"] {
        let started = Instant::now();
        let text = engines.transcribe(stt, e, None, "es", &resampled, 16000).expect("transcribe");
        println!("== transcribe 16 kHz ({pass}): {:.2}s → {text:?}", started.elapsed().as_secs_f32());
        let heard = text.to_lowercase();
        assert!(heard.contains("prueba") || heard.contains("voz"), "Whisper did not hear it: {text:?}");
    }

    let rss2 = rss_mb();
    let dropped = (engines.unload(stt, e).unwrap(), engines.unload(tts, e).unwrap());
    println!("== unload: {dropped:?} dropped; RSS {rss2:.0} → {:.0} MB", rss_mb());
    println!("== loaded after unload: {}", serde_json::to_string(&engines.loaded()).unwrap());
    let r = engines.load(stt, e, None, "es").unwrap();
    println!("== reload {stt} (files on disk, engine up): {} ms, RSS {:.0} MB", r.as_millis(), rss_mb());
    let _ = engines.unload(stt, e);

    // Refusals, through our contract.
    for (what, result) in [
        ("kokoro (no build in catalogue)", engines.load("kokoro-82m-v1.0", e, None, "ef_dora").map(|_| ())),
        ("whisper-base (no build here)", engines.load("whisper-base", "runanywhere", None, "es").map(|_| ())),
        ("coreml accelerator", engines.load(stt, e, Some(Capability::Coreml), "es").map(|_| ())),
    ] {
        let outcome = result.map(|_| "ACCEPTED".to_string()).unwrap_or_else(|err| format!("{} — {}", err.key, err.message));
        println!("== refusal, {what}: {outcome}");
    }
    if baseline {
        println!("SPIKE BASELINE OK");
        return;
    }

    let ra = runanywhere::instance().expect("RACommons loaded");
    println!("== RACommons version: {}", ra.version);
    let rt = runanywhere::Runtime(ra.clone());
    let kokoro = <runanywhere::Runtime as sidevoice_desktop_engine::Runtime>::voice(
        &rt, "kokoro", &store, &serde_json::Value::Null, "es", Capability::Cpu);
    println!("== refusal, kokoro family at the runtime: {}", kokoro.err().map(|err| err.message).unwrap_or_default());
    // RACommons' own errors: an empty model directory, and a Piper directory handed to the STT component.
    let empty = store.join("empty-model");
    std::fs::create_dir_all(&empty).unwrap();
    let err = |r: Result<(), String>| r.err().unwrap_or_else(|| "ACCEPTED".into());
    println!("== RACommons error, STT on an empty dir: {}", err(runanywhere::Recognizer::new(ra.clone(), &empty, "empty", "es").map(|_| ())));
    let piper_dir = std::fs::read_dir(store.join("models").join(e).join(tts)).unwrap().flatten().find(|d| d.path().is_dir()).unwrap().path();
    println!("== RACommons error, STT on a Piper dir: {}", err(runanywhere::Recognizer::new(ra.clone(), &piper_dir, "wrong", "es").map(|_| ())));
    println!("== RACommons error, TTS on an empty dir: {}", err(runanywhere::Voice::new(ra.clone(), &empty, "empty").map(|_| ())));
    let whisper_dir = std::fs::read_dir(store.join("models").join(e).join(stt)).unwrap().flatten().find(|d| d.path().is_dir()).unwrap().path();
    println!("== RACommons error, TTS on a Whisper dir: {}", err(runanywhere::Voice::new(ra.clone(), &whisper_dir, "wrong").map(|_| ())));

    // RunAnywhere's own download path, fed our catalogue entry (URL + SHA-256). The destination is removed before
    // each case; the last case leaves a stale file in place on purpose.
    let entry = engines.catalog.model(tts).unwrap().build(e).unwrap().download.clone().unwrap();
    let dest = store.join("ra-download.tar.bz2");
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let fresh = || {
        let _ = std::fs::remove_file(&dest);
        let _ = std::fs::remove_file(&part);
    };
    fresh();
    let (mut calls, mut last) = (0u32, (0, 0));
    let started = Instant::now();
    let (status, http) = ra.download(&entry.url, &dest, Some(&entry.sha256), &mut |d, t| {
        calls += 1;
        last = (d, t);
        true
    });
    println!("== RA download, our SHA-256: status {status} (0 ok), HTTP {http}, {calls} progress calls, last {last:?}, {:.1}s", started.elapsed().as_secs_f32());
    fresh();
    let (status, http) = ra.download(&entry.url, &dest, Some(&"0".repeat(64)), &mut |_, _| true);
    println!("== RA download, wrong SHA-256: status {status} (5 checksum failed), HTTP {http}, left behind: dest {} / .part {}", dest.exists(), part.exists());
    let (status, http) = ra.download(&entry.url, &dest, Some(&entry.sha256), &mut |_, _| true);
    println!("== RA download, right SHA-256 after a failed one (.part kept): status {status}, HTTP {http}");
    fresh();
    let (status, _) = ra.download(&entry.url, &dest, None, &mut |_, _| true);
    println!("== RA download, no SHA-256: status {status} (0 = accepted unverified)");
    fresh();
    let started = Instant::now();
    let (status, _) = ra.download(&entry.url, &dest, Some(&entry.sha256), &mut |done, _| done < 4 * 1024 * 1024);
    let partial = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    println!("== RA download, cancelled at 4 MB: status {status} (6 cancelled) in {:.1}s, .part left: {partial} bytes", started.elapsed().as_secs_f32());
    let (status, _) = ra.download(&entry.url, &dest, Some(&entry.sha256), &mut |_, _| true);
    println!("== RA download, fresh request after the cancel (resumes the .part): status {status}");
    // A stale .part from some other download: a fresh request (resume_from_byte 0) is silently turned into a resume.
    for sha in [None, Some(entry.sha256.as_str())] {
        fresh();
        std::fs::write(&part, b"stale bytes from some other download").unwrap();
        let (status, _) = ra.download(&entry.url, &dest, sha, &mut |_, _| true);
        let head = std::fs::read(&dest).map(|b| b[..20.min(b.len())].to_vec()).unwrap_or_default();
        println!("== RA download, stale .part, SHA given: {}: status {status}, dest starts with {:?}", sha.is_some(), String::from_utf8_lossy(&head));
    }
    fresh();

    println!("== URLs RACommons asked our transport for: {:#?}", runanywhere::requested_urls());
    println!("SPIKE OK");
}
