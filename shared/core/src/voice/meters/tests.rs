use super::Meters;

#[test]
fn the_level_goes_when_its_percent_changes() {
    let mut meters = Meters::default();
    assert!(meters.level(0.0, true), "the first one always goes");
    assert!(!meters.level(0.0, true));
    assert!(!meters.level(0.004, true), "the same percent");
    assert!(meters.level(0.12, true));
    assert!(!meters.level(0.1201, true));
    assert!(meters.level(0.0, true));
    assert!(!meters.level(-1.0, true), "out of range is clamped: still 0");
    assert!(meters.level(2.0, true));
    assert!(!meters.level(1.0, true));
}

#[test]
fn a_silent_output_goes_once_and_a_sounding_one_every_time() {
    let mut meters = Meters::default();
    assert!(meters.playback(0.0, true), "the first silence closes the mouth");
    assert!(!meters.playback(0.0, true));
    assert!(!meters.playback(0.0, true));
    assert!(meters.playback(0.3, true));
    assert!(meters.playback(0.3, true), "a reply that sounds is never held back");
    assert!(meters.playback(0.0, true));
    assert!(!meters.playback(f32::NAN, true), "not a level: silence");
}

#[test]
fn nothing_goes_while_the_page_is_hidden_and_the_first_level_after_does() {
    let mut meters = Meters::default();
    assert!(meters.level(0.5, true));
    assert!(!meters.level(0.5, true));
    for value in [0.1, 0.2, 0.3] {
        assert!(!meters.level(value, false), "hidden: dropped, not queued");
        assert!(!meters.playback(value, false));
    }
    assert!(meters.level(0.3, true), "shown again: the current level goes, even if it repeats the last one sent");
    assert!(meters.playback(0.0, true), "and the output's silence closes the mouth once");
    assert!(!meters.playback(0.0, true));
}
