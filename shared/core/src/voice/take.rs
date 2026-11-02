//! A voice call's take as the room page gets it (docs/BRIDGE.md → "The voice call"): sidevoice-voice's `Take`, its
//! samples as the base64 of their 32-bit little-endian floats, the bridge's to turn back into a `Float32Array`, and the
//! rest as the call told it. A take of a few seconds is a few hundred kilobytes; as JSON numbers it would be several
//! times that, through a `webview.eval`.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::{json, Value};

#[cfg(test)]
mod tests;

/// The event the page's `voiceEvent` receives for a take: `{type: "take", data: {sampleRate, samples, started_at,
/// ended_at, turns}}`, `samples` the base64 of `samples` as 32-bit little-endian floats and `turns` the take's turns
/// as JSON.
pub fn page_event(sample_rate: u32, samples: &[f32], started_at: u64, ended_at: u64, turns: Value) -> Value {
    let bytes: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
    json!({
        "type": "take",
        "data": {
            "sampleRate": sample_rate,
            "samples": STANDARD.encode(bytes),
            "started_at": started_at,
            "ended_at": ended_at,
            "turns": turns,
        },
    })
}
