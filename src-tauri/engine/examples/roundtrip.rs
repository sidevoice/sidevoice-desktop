//! The native engine end to end on this machine, for CI on Apple Silicon: download sherpa-onnx and two models
//! from the bundled catalog, say a Spanish sentence with Kokoro, and hear it back with Whisper — through the same
//! calls the page makes (model + engine, and optionally the accelerator).
//!
//!   cargo run --release -p sidevoice-desktop-engine --example roundtrip -- <store dir> [accelerator]
use sidevoice_desktop_core::engines::{bundled_catalog, capability_named, Capability};
use sidevoice_desktop_engine::NativeEngines;
use std::time::Instant;

fn main() {
    let store = std::env::args().nth(1).expect("usage: roundtrip <store dir> [accelerator]");
    let accelerator: Option<Capability> = std::env::args()
        .nth(2)
        .map(|name| capability_named(&name))
        .inspect(|a| assert_ne!(*a, Capability::Unknown, "an accelerator: cpu, coreml…"));
    let engines = NativeEngines::new(bundled_catalog(), store);
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
    let installed = engines.installed();
    println!("installed: {}", serde_json::to_string(&installed).unwrap());
    assert!(installed.iter().any(|b| b.model == "whisper-tiny" && b.engine == "sherpa-onnx"));
    assert!(installed.iter().any(|b| b.model == "kokoro-82m-v1.0" && b.engine == "sherpa-onnx"));
    println!("accelerator: {}", accelerator.map(|a| format!("{a:?}")).unwrap_or_else(|| "the resolver's".into()));
    let sentence = "Hola, esto es una prueba de voz de Sidevoice.";
    let started = Instant::now();
    let audio = engines
        .synthesize("kokoro-82m-v1.0", "sherpa-onnx", accelerator, "ef_dora", 1.0, sentence)
        .expect("synthesize");
    let seconds = audio.samples.len() as f32 / audio.sample_rate as f32;
    println!(
        "synthesized {seconds:.2}s of audio at {} Hz in {:.2}s",
        audio.sample_rate,
        started.elapsed().as_secs_f32()
    );
    assert!(seconds > 1.0, "too little audio");
    let started = Instant::now();
    let text = engines
        .transcribe("whisper-tiny", "sherpa-onnx", accelerator, "es", &audio.samples, audio.sample_rate)
        .expect("transcribe");
    println!("transcribed in {:.2}s: {text:?}", started.elapsed().as_secs_f32());
    let heard = text.to_lowercase();
    assert!(heard.contains("prueba") || heard.contains("voz"), "Whisper did not hear the sentence: {text:?}");
    println!("ROUNDTRIP OK");
}
