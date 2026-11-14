//! The call's surfaces (the iOS Live Activity, Android's call notification; docs/MOBILE.md → "The call surface"): what
//! the page reports of them, and how the app turns that into the plugin's [`CallSurface`].
//!
//! The page projects them in its room snapshot (sidevoice-web's `callSurface`, beside the `callCard` the snapshot already
//! carries), and the bridge relays it as the snapshot's `surface`. The stills' bytes are not in it: the page hands each
//! one to the host once (`host.callSurface.still`, the app's `call_still`) before any snapshot names it by id. Every field has a default, so a page that sends less (or an older
//! page that sends nothing) shows an empty, idle call.
use serde::Deserialize;
use sidevoice_mobile_plugin::{CallSurface, Other, Phase, SpokenPage, Still, SurfaceWords};

/// What the bridge relays of the page's `callSurface` (the page's own shape; read, never written).
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SurfaceReport {
    /// The person's turn can still be taken back ("Cancelar envío").
    pending_cancellable: bool,
    /// The current conversation's reach: `listening`, `holding` (it cannot receive) or `offline`.
    reach: Option<String>,
    /// The model as the page words it ("Opus 5.5"), and the machine.
    model: Option<String>,
    machine: Option<String>,
    /// When the working phase began (ms since the epoch: the room's busy clock), when the page knows it.
    working_since: Option<f64>,
    /// How many conversations the call has, the current one included.
    conversation_count: u32,
    /// The current conversation's still.
    still: Option<Still>,
    /// Up to three other conversations.
    others: Vec<Other>,
    page: Option<PageReport>,
    said: Option<String>,
    words: SurfaceWords,
}

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PageReport {
    text: String,
    lit: u32,
}

/// How many other conversations the stack shows.
const OTHERS: usize = 3;

/// What the page reports for the surfaces: the room's (the snapshot's `callCard` fields: the agent's `mood`, `idle`,
/// `listening`, `working`, `speaking` or `offline`; the person's open `turn`, `listening` or `transcribing`; why the call is
/// `busy`, `reconnecting` or `switching`; whether a reply can be skipped) and the page's `callSurface` (`surface`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reported {
    pub mood: Option<String>,
    pub turn: Option<String>,
    pub busy_reason: Option<String>,
    pub can_skip: bool,
    /// The page's language (the snapshot's `callCard.language`).
    pub language: Option<String>,
    pub surface: SurfaceReport,
}

impl SurfaceReport {
    /// The page's `callSurface` as the bridge relays it; what cannot be read is an empty one (logged), so that a surface the
    /// app does not understand never stops the call it is part of.
    pub fn read(value: &serde_json::Value) -> SurfaceReport {
        if value.is_null() {
            return SurfaceReport::default();
        }
        serde_json::from_value(value.clone()).unwrap_or_else(|_| {
            // The parser's text can quote the page's: nothing of it is logged.
            log::warn!("call_state: the call's surface was not read");
            SurfaceReport::default()
        })
    }
}

/// The surface the page's report shows. While the call switches conversation it keeps the one it showed (`shown`) until
/// the new conversation's report comes (HANDOVER §1: switching has no state of its own).
pub fn surface(reported: &Reported, shown: &CallSurface) -> CallSurface {
    if reported.busy_reason.as_deref() == Some("switching") {
        return shown.clone();
    }
    let report = &reported.surface;
    // A still the snapshot names, with the file the surfaces read it from.
    let still = |still: &Option<Still>| {
        still
            .clone()
            .filter(|still| Still::valid_id(&still.id))
            .map(|still| Still {
                file: Still::file_name(&still.id),
                ..still
            })
    };
    CallSurface {
        phase: phase(reported),
        can_skip: reported.can_skip,
        can_cancel_send: report.pending_cancellable,
        model: report.model.clone().unwrap_or_default(),
        machine: report.machine.clone().unwrap_or_default(),
        working_since: report.working_since,
        still: still(&report.still),
        others: report
            .others
            .iter()
            .take(OTHERS)
            .map(|other| Other {
                title: other.title.clone(),
                still: still(&other.still),
            })
            .collect(),
        conversation_count: report.conversation_count,
        page: report
            .page
            .as_ref()
            .filter(|page| !page.text.is_empty())
            .map(|page| SpokenPage {
                text: page.text.clone(),
                lit: page.lit,
            }),
        said: report.said.clone().filter(|said| !said.trim().is_empty()),
        words: report.words.clone(),
        language: reported.language.clone().unwrap_or_default(),
    }
}

/// What the call is doing, in the order the page's state line says it (HANDOVER §1, D-LA-HOLDING-ORDER; sidevoice-web
/// `call-surface.js`): offline (or reconnecting) over everything, then the person's open turn, then the agent's voice,
/// then the person's words just sent, then the agent's work, then a conversation that cannot receive, which is a standing
/// condition and takes only idle's place: what is happening now always wins.
fn phase(reported: &Reported) -> Phase {
    let mood = reported.mood.as_deref().unwrap_or_default();
    let said = reported.surface.said.as_deref();
    if reported.busy_reason.as_deref() == Some("reconnecting") || mood == "offline" {
        return Phase::Offline;
    }
    match reported.turn.as_deref() {
        Some("listening") => return Phase::Listening,
        Some("transcribing") => return Phase::Transcribing,
        _ => {}
    }
    if mood == "speaking" {
        return Phase::Speaking;
    }
    if said.is_some_and(|said| !said.trim().is_empty()) {
        return Phase::Said;
    }
    match mood {
        "working" => Phase::Working,
        "listening" => Phase::Listening,
        // The page's line says working while the stage's mood is still idle (the person's words just sent, or the gap
        // before the stage's own 3 s wait is over): it sends the working start only while its line says working, so a start
        // is the call working, whatever the mood (sidevoice-web `call-surface.js`, `saying`).
        "idle" if reported.surface.working_since.is_some() => Phase::Working,
        _ if reported.surface.reach.as_deref() == Some("holding") => Phase::Holding,
        _ => Phase::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page reports `mood`, `turn` and `busy`, with the surface `json`.
    fn reported(mood: &str, turn: Option<&str>, busy: Option<&str>, json: &str) -> Reported {
        Reported {
            mood: Some(mood.into()),
            turn: turn.map(Into::into),
            busy_reason: busy.map(Into::into),
            can_skip: false,
            language: None,
            surface: SurfaceReport::read(&serde_json::from_str(json).unwrap()),
        }
    }

    fn shown(reported: &Reported) -> CallSurface {
        surface(reported, &CallSurface::default())
    }

    #[test]
    fn a_page_that_says_nothing_shows_an_idle_empty_call() {
        assert_eq!(shown(&Reported::default()), CallSurface::default());
        assert_eq!(
            SurfaceReport::read(&serde_json::Value::Null),
            SurfaceReport::default()
        );
        assert_eq!(
            SurfaceReport::read(&serde_json::json!({})),
            SurfaceReport::default()
        );
    }

    #[test]
    fn a_surface_that_cannot_be_read_is_an_empty_one() {
        for json in [r#"{"others":null}"#, r#"{"words":3}"#, r#"[]"#, r#""x""#] {
            let value = serde_json::from_str(json).unwrap();
            assert_eq!(
                SurfaceReport::read(&value),
                SurfaceReport::default(),
                "{json}"
            );
        }
    }

    #[test]
    fn the_page_s_surface_is_read_with_defaults_for_what_it_leaves_out() {
        let surface = shown(&reported(
            "working",
            None,
            None,
            r#"{"pendingCancellable":true,"reach":"listening","model":"Opus 5.5","machine":"mac-studio",
                "workingSince":1700000000000,"conversationCount":5,"still":{"id":"c1#240","k":1.4},
                "others":[{"threadId":"a","title":"A","reach":"listening","unread":0,"still":{"id":"a#108","k":1}},{"title":"B","still":null},{"title":"C"},{"title":"D"}],
                "page":{"text":"Hola…","lit":2,"more":true},"said":null,
                "words":{"state":"Trabajando","offline":"Desconectada","skip":"Saltar","unknown":"ignored"},
                "unknown":true}"#,
        ));
        assert_eq!(surface.phase, Phase::Working);
        assert!(surface.can_cancel_send);
        assert_eq!(
            (surface.model.as_str(), surface.machine.as_str()),
            ("Opus 5.5", "mac-studio")
        );
        assert_eq!(surface.working_since, Some(1_700_000_000_000.0));
        assert_eq!(surface.still.as_ref().map(|still| still.k), Some(1.4));
        assert_eq!(
            surface.still.as_ref().map(|still| still.file.as_str()),
            Some("633123323430.png")
        );
        assert_eq!(surface.others.len(), 3, "the stack shows three faces");
        assert_eq!(
            surface.others[0].still.as_ref().map(|still| still.k),
            Some(1.0)
        );
        assert_eq!(surface.others[1].still, None);
        assert_eq!(surface.conversation_count, 5);
        assert_eq!(surface.page.as_ref().map(|page| page.lit), Some(2));
        assert_eq!(surface.words.offline, "Desconectada");
        assert_eq!(surface.words.mute, "");
    }

    #[test]
    fn a_still_whose_id_cannot_name_a_file_is_none() {
        let long = "a".repeat(sidevoice_mobile_plugin::STILL_ID_LIMIT + 1);
        let surface = shown(&reported(
            "idle",
            None,
            None,
            &format!(r#"{{"still":{{"id":""}},"others":[{{"still":{{"id":"{long}"}}}}]}}"#),
        ));
        assert_eq!(surface.still, None);
        assert_eq!(surface.others[0].still, None);
    }

    #[test]
    fn no_working_time_is_made_up() {
        assert_eq!(
            shown(&reported("working", None, None, "{}")).working_since,
            None
        );
    }

    #[test]
    fn the_phase_follows_the_page_s_state_line() {
        let (empty, said, holding) = ("{}", r#"{"said":"Mira el test"}"#, r#"{"reach":"holding"}"#);
        let cases = [
            (reported("idle", None, None, empty), Phase::Idle),
            (reported("unknown", None, None, empty), Phase::Idle),
            (reported("listening", None, None, empty), Phase::Listening),
            (reported("working", None, None, empty), Phase::Working),
            (reported("speaking", None, None, empty), Phase::Speaking),
            (reported("offline", None, None, holding), Phase::Offline),
            (
                reported("speaking", None, Some("reconnecting"), empty),
                Phase::Offline,
            ),
            (reported("speaking", None, None, holding), Phase::Speaking),
            (reported("idle", None, None, holding), Phase::Holding),
            (
                reported("speaking", Some("listening"), None, said),
                Phase::Listening,
            ),
            (
                reported("idle", Some("transcribing"), None, empty),
                Phase::Transcribing,
            ),
            (reported("working", None, None, said), Phase::Said),
            (reported("speaking", None, None, said), Phase::Speaking),
            // The page's line says working before the stage's mood does: the working start makes it Working.
            (
                reported("idle", None, None, r#"{"workingSince":1000}"#),
                Phase::Working,
            ),
            (
                reported(
                    "idle",
                    None,
                    None,
                    r#"{"workingSince":1000,"reach":"holding"}"#,
                ),
                Phase::Working,
            ),
            (
                reported(
                    "idle",
                    None,
                    None,
                    r#"{"workingSince":1000,"said":"Mira el test"}"#,
                ),
                Phase::Said,
            ),
            (
                reported("idle", Some("listening"), None, r#"{"workingSince":1000}"#),
                Phase::Listening,
            ),
            (
                reported("speaking", None, None, r#"{"workingSince":1000}"#),
                Phase::Speaking,
            ),
        ];
        for (reported, expected) in cases {
            assert_eq!(shown(&reported).phase, expected, "{reported:?}");
        }
    }

    /// Every pair of what can be true at once: the higher in offline > the person's turn (listening, transcribing) >
    /// speaking > said > working > holding > idle wins (D-LA-HOLDING-ORDER).
    #[test]
    fn of_any_two_the_higher_wins() {
        // Each signal: what the page reports for it, in that order (highest first).
        #[derive(Clone, Copy)]
        struct Signal {
            phase: Phase,
            mood: Option<&'static str>,
            turn: Option<&'static str>,
            busy: Option<&'static str>,
            said: bool,
            holding: bool,
        }
        let none = Signal {
            phase: Phase::Idle,
            mood: None,
            turn: None,
            busy: None,
            said: false,
            holding: false,
        };
        let signals = [
            Signal {
                phase: Phase::Offline,
                busy: Some("reconnecting"),
                ..none
            },
            Signal {
                phase: Phase::Offline,
                mood: Some("offline"),
                ..none
            },
            Signal {
                phase: Phase::Listening,
                turn: Some("listening"),
                ..none
            },
            Signal {
                phase: Phase::Transcribing,
                turn: Some("transcribing"),
                ..none
            },
            Signal {
                phase: Phase::Speaking,
                mood: Some("speaking"),
                ..none
            },
            Signal {
                phase: Phase::Said,
                said: true,
                ..none
            },
            Signal {
                phase: Phase::Working,
                mood: Some("working"),
                ..none
            },
            Signal {
                phase: Phase::Holding,
                holding: true,
                ..none
            },
            Signal {
                phase: Phase::Idle,
                mood: Some("idle"),
                ..none
            },
        ];
        for (high_at, high) in signals.iter().enumerate() {
            for low in &signals[high_at + 1..] {
                // Two moods, or two turns, cannot be reported at once: such a pair is no case.
                if (high.mood.is_some() && low.mood.is_some())
                    || (high.turn.is_some() && low.turn.is_some())
                {
                    continue;
                }
                let mut json = serde_json::Map::new();
                if high.said || low.said {
                    json.insert("said".into(), "Mira el test".into());
                }
                if high.holding || low.holding {
                    json.insert("reach".into(), "holding".into());
                }
                let reported = Reported {
                    mood: high.mood.or(low.mood).map(Into::into),
                    turn: high.turn.or(low.turn).map(Into::into),
                    busy_reason: high.busy.or(low.busy).map(Into::into),
                    can_skip: false,
                    language: None,
                    surface: SurfaceReport::read(&serde_json::Value::Object(json)),
                };
                assert_eq!(
                    shown(&reported).phase,
                    high.phase,
                    "{:?} over {:?}",
                    high.phase,
                    low.phase
                );
            }
        }
    }

    #[test]
    fn the_page_s_language_is_carried_and_none_is_the_device_s() {
        let mut reported = reported("idle", None, None, "{}");
        assert_eq!(shown(&reported).language, "");
        reported.language = Some("es".into());
        assert_eq!(shown(&reported).language, "es");
    }

    #[test]
    fn switching_conversation_keeps_what_was_shown() {
        let before = shown(&reported("speaking", None, None, r#"{"model":"Opus 5.5"}"#));
        let switching = reported("idle", None, Some("switching"), r#"{"model":"Other"}"#);
        assert_eq!(surface(&switching, &before), before);
    }
}
