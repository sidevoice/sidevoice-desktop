use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::json;

use super::page_event;

#[test]
fn a_take_reaches_the_page_with_its_samples_as_little_endian_floats_in_base64() {
    let samples = [0.0_f32, 0.5, -1.0, 0.25];
    let turns =
        json!([{"turn_id": "c-turn-0", "speech_ms": 0, "pauses": [{"from_ms": 0, "to_ms": 0}], "end_ms": null}]);
    let event = page_event(16_000, &samples, 1_000, 1_001, turns.clone());
    assert_eq!(event["type"], "take");
    let data = &event["data"];
    assert_eq!(
        (&data["sampleRate"], &data["started_at"], &data["ended_at"], &data["turns"]),
        (&json!(16_000), &json!(1_000), &json!(1_001), &turns)
    );
    let bytes = STANDARD.decode(data["samples"].as_str().expect("base64")).expect("decodes");
    let decoded: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|four| f32::from_le_bytes(*four)).collect();
    assert_eq!(decoded, samples);
    // An empty take is an empty string.
    assert_eq!(page_event(16_000, &[], 0, 0, json!([]))["data"]["samples"], "");
}
