use super::Meters;

#[test]
fn the_level_goes_when_its_percent_changes() {
    let mut meters = Meters::default();
    assert!(meters.level(0.0), "the first one always goes");
    assert!(!meters.level(0.0));
    assert!(!meters.level(0.004), "the same percent");
    assert!(meters.level(0.12));
    assert!(!meters.level(0.1201));
    assert!(meters.level(0.0));
    assert!(!meters.level(-1.0), "out of range is clamped: still 0");
    assert!(meters.level(2.0));
    assert!(!meters.level(1.0));
}

#[test]
fn a_silent_output_goes_once_and_a_sounding_one_every_time() {
    let mut meters = Meters::default();
    assert!(meters.playback(0.0), "the first silence closes the mouth");
    assert!(!meters.playback(0.0));
    assert!(!meters.playback(0.0));
    assert!(meters.playback(0.3));
    assert!(meters.playback(0.3), "a reply that sounds is never held back");
    assert!(meters.playback(0.0));
    assert!(!meters.playback(f32::NAN), "not a level: silence");
}
