use super::*;
use sidevoice_voice::TurnTimings;

fn finished(id: &str, text: &str) -> TurnEvent {
    TurnEvent::Finished {
        turn_id: id.into(),
        text: text.into(),
        language: Some("es".into()),
        started_at: 10,
        ended_at: 20,
        merged: false,
        timings: TurnTimings { audio_ms: 1, endpoint_silence_ms: 1, recognition_ms: 1 },
    }
}

#[test]
fn only_a_turn_with_words_is_carried() {
    assert_eq!(CarriedTurn::of(&finished("t", "hola")).unwrap().text, "hola");
    assert_eq!(CarriedTurn::of(&finished("t", "  \n")), None, "no words");
    assert_eq!(CarriedTurn::of(&TurnEvent::Started { turn_id: "t".into(), started_at: 1 }), None);
    assert_eq!(CarriedTurn::of(&TurnEvent::Cancelled { turn_id: "t".into(), merged: false }), None);
}

#[test]
fn a_turn_is_handed_over_once_and_only_while_it_is_the_person_s_to_say() {
    let carry = Carry::default();
    let start = Instant::now();
    carry.keep(CarriedTurn::of(&finished("old", "one")).unwrap(), start);
    carry.keep(CarriedTurn::of(&finished("new", "two")).unwrap(), start + KEPT_FOR - Duration::from_secs(1));
    let taken = carry.take(start + KEPT_FOR + Duration::from_secs(1));
    assert_eq!(taken.iter().map(|turn| turn.turn_id.as_str()).collect::<Vec<_>>(), ["new"], "the old one lapsed");
    assert!(carry.take(start + KEPT_FOR).is_empty(), "said once");
}

#[test]
fn the_oldest_turns_go_first_when_too_many_are_kept() {
    let carry = Carry::default();
    let now = Instant::now();
    for n in 0..MOST + 2 {
        carry.keep(CarriedTurn::of(&finished(&n.to_string(), "words")).unwrap(), now);
    }
    let taken = carry.take(now);
    assert_eq!(taken.len(), MOST);
    assert_eq!(taken[0].turn_id, "2");
}

#[test]
fn a_turn_is_open_from_its_start_to_its_end() {
    let open = OpenTurns::default();
    open.seen(&TurnEvent::Started { turn_id: "a".into(), started_at: 1 });
    open.seen(&TurnEvent::Started { turn_id: "b".into(), started_at: 2 });
    open.seen(&TurnEvent::Cancelled { turn_id: "a".into(), merged: true });
    assert!(open.any());
    open.seen(&finished("b", "words"));
    assert!(!open.any());
}

#[test]
fn an_open_turn_is_waited_for_up_to_the_limit() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let open = OpenTurns::default();
        assert!(open.settled(Duration::from_millis(50)).await, "nothing open");
        open.seen(&TurnEvent::Started { turn_id: "a".into(), started_at: 1 });
        assert!(!open.settled(Duration::from_millis(60)).await, "it never ended");
    });
}
