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
fn a_turn_is_handed_over_only_for_its_loss_and_conversation_while_it_is_the_person_s_to_say() {
    let carry = Carry::default();
    let start = Instant::now();
    carry.keep(CarriedTurn::of(&finished("old", "one")).unwrap(), start);
    carry.bind(1, Some("a".into()));
    carry.keep(CarriedTurn::of(&finished("new", "two")).unwrap(), start + KEPT_FOR - Duration::from_secs(1));
    let at = start + KEPT_FOR + Duration::from_secs(1);
    let ids = |turns: Vec<CarriedTurn>| turns.into_iter().map(|turn| turn.turn_id).collect::<Vec<_>>();
    assert_eq!(ids(carry.peek(1, Some("a"), at)), ["new"], "the old one lapsed");
    assert_eq!(carry.peek(1, Some("a"), at)[0].age_ms, 2000, "its age goes with it");
    assert!(carry.peek(2, Some("a"), at).is_empty(), "another loss");
    assert!(carry.peek(1, Some("b"), at).is_empty(), "another conversation");
    assert_eq!(
        ids(carry.peek(1, Some("a"), at)),
        ["new"],
        "kept until settled: a page lost again is handed them again"
    );
}

#[test]
fn any_outcome_but_a_rejoin_drops_what_was_kept() {
    let carry = Carry::default();
    let now = Instant::now();
    carry.keep(CarriedTurn::of(&finished("floating", "one")).unwrap(), now);
    carry.bind(1, None);
    carry.keep(CarriedTurn::of(&finished("bound", "two")).unwrap(), now);
    assert_eq!(carry.settle(1), 2, "the one kept before the bind is the loss's too");
    assert!(carry.peek(1, None, now).is_empty());
    carry.bind(2, None);
    carry.keep(CarriedTurn::of(&finished("again", "three")).unwrap(), now);
    assert_eq!(carry.settle(1), 0, "another loss's settle leaves it");
    assert_eq!(carry.clear(), 1, "the person hung up");
    assert!(carry.peek(2, None, now).is_empty());
}

#[test]
fn the_oldest_turns_go_first_when_too_many_are_kept() {
    let carry = Carry::default();
    let now = Instant::now();
    carry.bind(1, None);
    for n in 0..MOST + 2 {
        carry.keep(CarriedTurn::of(&finished(&n.to_string(), "words")).unwrap(), now);
    }
    let taken = carry.peek(1, None, now);
    assert_eq!(taken.len(), MOST);
    assert_eq!(taken[0].turn_id, "2");
}

#[test]
fn a_turn_is_open_from_its_start_to_its_end_and_remembers_whether_its_start_was_told() {
    let open = OpenTurns::default();
    open.seen(&TurnEvent::Started { turn_id: "a".into(), started_at: 1 }, true);
    open.seen(&TurnEvent::Started { turn_id: "b".into(), started_at: 2 }, false);
    open.seen(&TurnEvent::Cancelled { turn_id: "a".into(), merged: true }, true);
    assert!(open.any());
    open.seen(&finished("b", "words"), false);
    assert!(!open.any());
    assert!(!open.take_told("b"), "its start was not told to a page");
}

#[test]
fn an_open_turn_is_waited_for_up_to_the_limit() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let open = OpenTurns::default();
        assert!(open.settled(Duration::from_millis(50)).await, "nothing open");
        open.seen(&TurnEvent::Started { turn_id: "a".into(), started_at: 1 }, false);
        assert!(!open.settled(Duration::from_millis(60)).await, "it never ended");
    });
}
