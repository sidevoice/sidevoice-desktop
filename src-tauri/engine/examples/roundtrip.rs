//! The native engine end to end on this machine, for CI on Apple Silicon: download two models of sidevoice-engine's
//! catalogue, load both, say a sentence with Kokoro and hear it back with Whisper — Spanish, English, Spanish again,
//! through the one model each has in memory — then unload them; through the same calls the page makes (model +
//! engine, and optionally the accelerator).
//!
//!   cargo run --release -p sidevoice-desktop-engine --example roundtrip -- <store dir> [accelerator]
use sidevoice_desktop_engine::NativeEngines;
use std::time::Instant;

fn main() {
    let store = std::env::args().nth(1).expect("usage: roundtrip <store dir> [accelerator]");
    let accelerator = std::env::args().nth(2);
    let accelerator = accelerator.as_deref();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a Tokio runtime");
    let engines = NativeEngines::new(store, runtime.handle().clone()).expect("the engine");
    println!("capabilities: {}", serde_json::to_string(&engines.device).unwrap());
    assert!(engines.device.memory_mb.is_some_and(|mb| mb > 0), "the OS reports the machine's memory");
    for model in ["kokoro-82m-v1.0", "whisper-tiny"] {
        let started = Instant::now();
        let mut last = 0;
        engines
            .install(model, "sherpa-onnx", &mut |done, total| {
                let percent = (done * 100).checked_div(total).unwrap_or(100);
                if percent >= last + 25 {
                    last = percent;
                    println!("  {model}: {percent}%");
                }
            })
            .unwrap_or_else(|e| panic!("installing {model}: {e}"));
        println!("installed {model} in {:.1}s", started.elapsed().as_secs_f32());
    }
    let installed = engines.installed().expect("installed");
    println!("installed: {}", serde_json::to_string(&installed).unwrap());
    assert!(installed.iter().any(|b| b.model == "whisper-tiny" && b.engine == "sherpa-onnx"));
    assert!(installed.iter().any(|b| b.model == "kokoro-82m-v1.0" && b.engine == "sherpa-onnx"));
    println!("accelerator: {}", accelerator.unwrap_or("the engine's"));
    // Loaded before use, as the page does on select and the app as a call connects (sidevoice/sidevoice-core#21 D11, D13).
    for model in ["kokoro-82m-v1.0", "whisper-tiny"] {
        let load = engines.load(model, "sherpa-onnx", accelerator).unwrap_or_else(|e| panic!("loading {model}: {e}"));
        println!("loaded {model} in {} ms", load.load_ms);
    }
    let loaded = engines.loaded();
    println!("loaded: {}", serde_json::to_string(&loaded).unwrap());
    assert_eq!(loaded.len(), 2, "both models in memory");
    println!("memory: {}", serde_json::to_string(&engines.memory()).unwrap());
    // One model in memory each, every language through it: Spanish, then English, then Spanish again.
    for (voice, language, sentence, expected) in [
        ("ef_dora", "es", "Hola, esto es una prueba de voz de Sidevoice.", ["prueba", "voz"]),
        ("af_heart", "en", "Hello, this is a voice test from Sidevoice.", ["test", "voice"]),
        ("ef_dora", "es", "Hola, esto es una prueba de voz de Sidevoice.", ["prueba", "voz"]),
    ] {
        let started = Instant::now();
        let audio = engines
            .synthesize("kokoro-82m-v1.0", "sherpa-onnx", accelerator, voice, 1.0, sentence)
            .unwrap_or_else(|e| panic!("synthesize: {e}"));
        let seconds = audio.samples.len() as f32 / audio.sample_rate as f32;
        println!(
            "synthesized {seconds:.2}s of audio at {} Hz with {voice} in {:.2}s",
            audio.sample_rate,
            started.elapsed().as_secs_f32()
        );
        assert!(seconds > 1.0, "too little audio");
        let started = Instant::now();
        let text = engines
            .transcribe("whisper-tiny", "sherpa-onnx", accelerator, language, &audio.samples, audio.sample_rate)
            .unwrap_or_else(|e| panic!("transcribe: {e}"));
        println!("transcribed ({language}) in {:.2}s: {text:?}", started.elapsed().as_secs_f32());
        let heard = text.to_lowercase();
        assert!(expected.iter().any(|w| heard.contains(w)), "Whisper did not hear the sentence: {text:?}");
    }
    assert_eq!(engines.loaded().len(), 2, "still one instance each: the language changed, nothing was loaded again");
    engines.unload("whisper-tiny", "sherpa-onnx", None);
    engines.unload("kokoro-82m-v1.0", "sherpa-onnx", None);
    assert!(engines.loaded().is_empty(), "unloaded");
    println!("ROUNDTRIP OK");
}
