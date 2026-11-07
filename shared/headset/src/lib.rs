//! What a headset's presses do, for every app that answers one (the desktop's macOS, the phones): the choices a person has
//! (`mute`, say one thing, skip what is playing, nothing), the change `host.shortcuts.setHeadset` asks for and the
//! `HeadsetStatus` `host.shortcuts.get` reports (docs/BRIDGE.md → "The shortcuts"). Pure: how a press reaches the app
//! (Now Playing, a media session) is `sidevoice-shared-tauri`'s, and where the choice is kept is each app's.
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// What a headset's press does: the single press (play/pause) or the double press (the system's next track).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HeadsetPress {
    #[default]
    Mute,
    OneTurn,
    Skip,
    None,
}

impl HeadsetPress {
    /// Its name in the page's `HeadsetPress`.
    pub fn name(self) -> &'static str {
        match self {
            HeadsetPress::Mute => "mute",
            HeadsetPress::OneTurn => "oneTurn",
            HeadsetPress::Skip => "skip",
            HeadsetPress::None => "none",
        }
    }

    pub fn parse(name: &str) -> Option<HeadsetPress> {
        match name {
            "mute" => Some(HeadsetPress::Mute),
            "oneTurn" => Some(HeadsetPress::OneTurn),
            "skip" => Some(HeadsetPress::Skip),
            "none" => Some(HeadsetPress::None),
            _ => None,
        }
    }

    /// Every action a press can be given, in the order the page lists them.
    pub const ALL: [HeadsetPress; 4] = [
        HeadsetPress::Mute,
        HeadsetPress::OneTurn,
        HeadsetPress::Skip,
        HeadsetPress::None,
    ];

    /// The command of the room (`run({command})`, docs/BRIDGE.md → "Commands") this press asks for; `None` for none.
    pub fn command(self) -> Option<&'static str> {
        match self {
            HeadsetPress::Mute => Some("toggle-mute"),
            HeadsetPress::OneTurn => Some("one-turn"),
            HeadsetPress::Skip => Some("skip-reply"),
            HeadsetPress::None => None,
        }
    }
}

/// What a host's headset presses can do during a call (`HeadsetStatus.actions`): the actions the single press can be given, and
/// the double press's where one reaches the app (`None`: none does, the page draws no row for it). A press with one action has
/// nothing to choose: it does that. Each OS's media controls say which (`sidevoice-shared-tauri`'s `media_controls::presses`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadsetCapabilities {
    pub press: &'static [HeadsetPress],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_press: Option<&'static [HeadsetPress]>,
}

impl HeadsetCapabilities {
    /// Both presses reach the app, and each can do anything.
    pub const EVERY: HeadsetCapabilities = HeadsetCapabilities {
        press: &HeadsetPress::ALL,
        double_press: Some(&HeadsetPress::ALL),
    };

    /// Only the system's own mute reaches the app, on the single press (iOS during a call: the AirPods and Beats Press to Mute);
    /// no double press does.
    pub const SYSTEM_MUTE: HeadsetCapabilities = HeadsetCapabilities {
        press: &[HeadsetPress::Mute],
        double_press: None,
    };

    /// Whether `change` asks only for what these presses can do; the message is the `bad_request` one.
    pub fn allow(self, change: HeadsetChange) -> Result<(), String> {
        if let Some(press) = change.press {
            if !self.press.contains(&press) {
                return Err(format!("the single press cannot do {} here", press.name()));
            }
        }
        match (change.double_press, self.double_press) {
            (None, _) => Ok(()),
            (Some(_), None) => Err("no double press reaches the app here".into()),
            (Some(press), Some(actions)) if !actions.contains(&press) => {
                Err(format!("the double press cannot do {} here", press.name()))
            }
            _ => Ok(()),
        }
    }
}

/// What `chosen` is among `actions`: itself where it is one of them, else the press's default `fallback` where that is, else
/// the first. A choice kept from where the press could do more stays kept; this is what it does here.
fn within(chosen: HeadsetPress, actions: &[HeadsetPress], fallback: HeadsetPress) -> HeadsetPress {
    if actions.contains(&chosen) {
        chosen
    } else if actions.contains(&fallback) || actions.is_empty() {
        fallback
    } else {
        actions[0]
    }
}

/// What the person chose, as an app keeps it: the single press mutes by default and the double press does nothing, so that
/// a keyboard's next-track key, which arrives the same way, does nothing until chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadsetChoice {
    pub press: HeadsetPress,
    pub double_press: HeadsetPress,
}

/// A kept record is read field by field: a field that is missing, or holds a value this build does not know (a later build's
/// action, a hand edit), is that field's default, and the others stay as kept. Only what is not JSON at all is the app's to
/// replace with the defaults.
impl<'de> Deserialize<'de> for HeadsetChoice {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let record = Value::deserialize(deserializer)?;
        let defaults = HeadsetChoice::default();
        let field = |name: &str, fallback: HeadsetPress| {
            record
                .get(name)
                .and_then(Value::as_str)
                .and_then(HeadsetPress::parse)
                .unwrap_or(fallback)
        };
        Ok(HeadsetChoice {
            press: field("press", defaults.press),
            double_press: field("doublePress", defaults.double_press),
        })
    }
}

impl Default for HeadsetChoice {
    fn default() -> Self {
        HeadsetChoice {
            press: HeadsetPress::Mute,
            double_press: HeadsetPress::None,
        }
    }
}

/// `host.shortcuts.setHeadset(change)` as the page sent it, checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeadsetChange {
    pub press: Option<HeadsetPress>,
    pub double_press: Option<HeadsetPress>,
}

impl HeadsetChange {
    /// Reads the three fields of the page's `change`; the message is the `bad_request` one. The app leaves a headset's
    /// buttons to other apps outside a call, so `outsideCall: "join"` is refused. A null `doublePress` is not given.
    pub fn parse(
        press: Option<&str>,
        double: Option<&Value>,
        outside: Option<&str>,
    ) -> Result<HeadsetChange, String> {
        let double_press = match double.filter(|value| !value.is_null()) {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .and_then(HeadsetPress::parse)
                    .ok_or_else(|| format!("no such press: {value}"))?,
            ),
        };
        if outside.is_some_and(|value| value != "leave") {
            return Err("the app leaves a headset's buttons to other apps outside a call".into());
        }
        let press = press
            .map(|press| {
                HeadsetPress::parse(press).ok_or_else(|| format!("no such press: {press}"))
            })
            .transpose()?;
        Ok(HeadsetChange {
            press,
            double_press,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.press.is_none() && self.double_press.is_none()
    }
}

impl HeadsetChoice {
    /// The choice with `change` applied: what is not in it stays.
    pub fn with(self, change: HeadsetChange) -> HeadsetChoice {
        HeadsetChoice {
            press: change.press.unwrap_or(self.press),
            double_press: change.double_press.unwrap_or(self.double_press),
        }
    }
}

/// What a headset's buttons do here: `host.shortcuts`' `HeadsetStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Headset {
    pub press: HeadsetPress,
    /// A double press arrives as the system's next-track command, which the app answers while it holds the media session.
    /// Always there where the app owns the headset; `Option` is the page's `null`: a path that cannot send one.
    pub double_press: Option<HeadsetPress>,
    /// The headset only acts in a call: the app leaves the buttons to other apps outside one.
    pub outside_call: &'static str,
    /// The system's call holds the headset (CallKit, Telecom): the system mutes, and the choices are not the app's to make.
    /// Left out when false.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub managed_by_system: bool,
    /// What each press can do here (`HeadsetStatus.actions`: the actions of each, the double press's left out where none reaches
    /// the app), where the host says: `None` (left out) is every action, as a host that predates it reports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions: Option<HeadsetCapabilities>,
}

impl Headset {
    pub fn new(press: HeadsetPress, double_press: HeadsetPress) -> Headset {
        Headset {
            press,
            double_press: Some(double_press),
            outside_call: "leave",
            managed_by_system: false,
            actions: None,
        }
    }

    /// The status of a `choice` on a host whose presses can do what `capabilities` says: each press as it acts here, the double
    /// press `null` where none reaches the app, and the actions of each; `managed_by_system`: the system's call holds the headset.
    pub fn of(
        choice: HeadsetChoice,
        managed_by_system: bool,
        capabilities: HeadsetCapabilities,
    ) -> Headset {
        let here = choice.within(capabilities);
        Headset {
            double_press: capabilities.double_press.map(|_| here.double_press),
            managed_by_system,
            actions: Some(capabilities),
            ..Headset::new(here.press, here.double_press)
        }
    }
}

/// Which press of the headset arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presses {
    Single,
    Double,
}

impl HeadsetChoice {
    /// What the choice does on a host whose presses can do what `capabilities` says ([`within`] for each press; a double press
    /// that does not reach the app does nothing). The choice kept is not changed: this is only what it does here.
    pub fn within(self, capabilities: HeadsetCapabilities) -> HeadsetChoice {
        let defaults = HeadsetChoice::default();
        HeadsetChoice {
            press: within(self.press, capabilities.press, defaults.press),
            double_press: capabilities
                .double_press
                .map_or(HeadsetPress::None, |actions| {
                    within(self.double_press, actions, defaults.double_press)
                }),
        }
    }

    /// What `which` does now, on a host whose presses can do what `capabilities` says. While the system's call holds the headset
    /// (`managed_by_system`) the system mutes: the single press is a mute, whatever was chosen, and the double press nothing.
    pub fn for_press(
        self,
        which: Presses,
        managed_by_system: bool,
        capabilities: HeadsetCapabilities,
    ) -> HeadsetPress {
        let here = self.within(capabilities);
        match (which, managed_by_system) {
            (Presses::Single, false) => here.press,
            (Presses::Double, false) => here.double_press,
            (Presses::Single, true) => HeadsetPress::Mute,
            (Presses::Double, true) => HeadsetPress::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_defaults_mute_on_one_press_and_nothing_on_two() {
        let choice = HeadsetChoice::default();
        assert_eq!(
            (choice.press, choice.double_press),
            (HeadsetPress::Mute, HeadsetPress::None)
        );
    }

    #[test]
    fn each_press_asks_the_room_for_its_command() {
        assert_eq!(HeadsetPress::Mute.command(), Some("toggle-mute"));
        assert_eq!(HeadsetPress::OneTurn.command(), Some("one-turn"));
        assert_eq!(HeadsetPress::Skip.command(), Some("skip-reply"));
        assert_eq!(HeadsetPress::None.command(), None);
        assert_eq!(HeadsetPress::parse("oneTurn"), Some(HeadsetPress::OneTurn));
        assert_eq!(
            HeadsetPress::parse("one-turn"),
            None,
            "the page's names, not the room's"
        );
    }

    #[test]
    fn a_change_is_checked_and_keeps_what_it_does_not_name() {
        let change = HeadsetChange::parse(None, Some(&json!("skip")), Some("leave")).unwrap();
        assert_eq!(
            change,
            HeadsetChange {
                press: None,
                double_press: Some(HeadsetPress::Skip)
            }
        );
        let now = HeadsetChoice {
            press: HeadsetPress::OneTurn,
            double_press: HeadsetPress::None,
        }
        .with(change);
        assert_eq!(
            (now.press, now.double_press),
            (HeadsetPress::OneTurn, HeadsetPress::Skip)
        );
        assert!(
            HeadsetChange::parse(None, Some(&Value::Null), None)
                .unwrap()
                .is_empty(),
            "null is not given"
        );
        assert!(HeadsetChange::parse(Some("loud"), None, None).is_err());
        assert!(HeadsetChange::parse(None, Some(&json!(3)), None).is_err());
        assert!(HeadsetChange::parse(None, None, Some("join")).is_err());
    }

    #[test]
    fn the_status_is_the_pages_and_says_so_only_when_the_system_holds_the_headset() {
        let new =
            serde_json::to_value(Headset::new(HeadsetPress::Mute, HeadsetPress::None)).unwrap();
        assert_eq!(
            new,
            json!({"press": "mute", "doublePress": "none", "outsideCall": "leave"}),
            "a host that reports no actions (the desktop) reports what it always did"
        );
        let system = serde_json::to_value(Headset::of(
            HeadsetChoice::default(),
            true,
            HeadsetCapabilities::EVERY,
        ))
        .unwrap();
        assert_eq!(system["managedBySystem"], json!(true));
    }

    #[test]
    fn a_host_whose_presses_can_do_everything_reports_both_presses_and_every_action() {
        let choice = HeadsetChoice {
            press: HeadsetPress::Skip,
            double_press: HeadsetPress::OneTurn,
        };
        let every = ["mute", "oneTurn", "skip", "none"];
        assert_eq!(
            serde_json::to_value(Headset::of(choice, false, HeadsetCapabilities::EVERY)).unwrap(),
            json!({
                "press": "skip",
                "doublePress": "oneTurn",
                "outsideCall": "leave",
                "actions": {"press": every, "doublePress": every},
            })
        );
    }

    #[test]
    fn where_only_the_system_s_mute_reaches_the_app_there_is_no_double_press_and_the_single_press_mutes(
    ) {
        // A choice kept from a build whose presses could do more (TestFlight 61 on iOS): what it does here, not what is kept.
        let kept = HeadsetChoice {
            press: HeadsetPress::Skip,
            double_press: HeadsetPress::OneTurn,
        };
        let mute = HeadsetCapabilities::SYSTEM_MUTE;
        assert_eq!(
            serde_json::to_value(Headset::of(kept, false, mute)).unwrap(),
            json!({"press": "mute", "doublePress": null, "outsideCall": "leave", "actions": {"press": ["mute"]}})
        );
        assert_eq!(
            kept.for_press(Presses::Single, false, mute),
            HeadsetPress::Mute
        );
        assert_eq!(
            kept.for_press(Presses::Double, false, mute),
            HeadsetPress::None
        );
        assert_eq!(kept.within(mute), HeadsetChoice::default());
        assert_eq!(
            kept.within(HeadsetCapabilities::EVERY),
            kept,
            "the choice itself is kept"
        );
    }

    #[test]
    fn a_change_asks_only_for_what_the_presses_can_do() {
        let change =
            |press: Option<HeadsetPress>, double_press: Option<HeadsetPress>| HeadsetChange {
                press,
                double_press,
            };
        let mute = HeadsetCapabilities::SYSTEM_MUTE;
        assert_eq!(mute.allow(change(Some(HeadsetPress::Mute), None)), Ok(()));
        assert_eq!(mute.allow(HeadsetChange::default()), Ok(()));
        assert!(mute.allow(change(Some(HeadsetPress::Skip), None)).is_err());
        assert!(
            mute.allow(change(None, Some(HeadsetPress::None))).is_err(),
            "no double press to set"
        );
        for press in HeadsetPress::ALL {
            assert_eq!(
                HeadsetCapabilities::EVERY.allow(change(Some(press), Some(press))),
                Ok(())
            );
        }
    }

    #[test]
    fn what_a_press_does_falls_back_to_its_default_and_then_to_the_first_action() {
        assert_eq!(
            within(
                HeadsetPress::Skip,
                &[HeadsetPress::Mute, HeadsetPress::OneTurn],
                HeadsetPress::Mute
            ),
            HeadsetPress::Mute
        );
        assert_eq!(
            within(
                HeadsetPress::Skip,
                &[HeadsetPress::OneTurn, HeadsetPress::None],
                HeadsetPress::Mute
            ),
            HeadsetPress::OneTurn
        );
        assert_eq!(
            within(
                HeadsetPress::OneTurn,
                &[HeadsetPress::OneTurn],
                HeadsetPress::Mute
            ),
            HeadsetPress::OneTurn
        );
        for press in HeadsetPress::ALL {
            assert_eq!(HeadsetPress::parse(press.name()), Some(press));
        }
    }

    #[test]
    fn a_partial_record_keeps_the_defaults_for_what_it_lacks() {
        let kept = |text: &str| serde_json::from_str::<HeadsetChoice>(text).unwrap();
        assert_eq!(kept("{}"), HeadsetChoice::default());
        assert_eq!(kept(r#"{"press":"mute"}"#).double_press, HeadsetPress::None);
        assert_eq!(kept(r#"{"press":"skip"}"#).double_press, HeadsetPress::None);
        assert_eq!(kept(r#"{"doublePress":"skip"}"#).press, HeadsetPress::Mute);
    }

    #[test]
    fn a_value_this_build_does_not_know_is_that_field_s_default_and_the_other_stays() {
        let kept = |text: &str| serde_json::from_str::<HeadsetChoice>(text).unwrap();
        assert_eq!(
            kept(r#"{"press":"skip","doublePress":"say-one"}"#),
            HeadsetChoice {
                press: HeadsetPress::Skip,
                double_press: HeadsetPress::None
            }
        );
        assert_eq!(
            kept(r#"{"press":3,"doublePress":"oneTurn"}"#),
            HeadsetChoice {
                press: HeadsetPress::Mute,
                double_press: HeadsetPress::OneTurn
            }
        );
        assert_eq!(
            kept("[]"),
            HeadsetChoice::default(),
            "not a record: the defaults"
        );
        let every = HeadsetChoice {
            press: HeadsetPress::OneTurn,
            double_press: HeadsetPress::Skip,
        };
        assert_eq!(
            kept(&serde_json::to_string(&every).unwrap()),
            every,
            "what it writes it reads back"
        );
    }

    #[test]
    fn the_system_s_call_leaves_only_its_mute() {
        let choice = HeadsetChoice {
            press: HeadsetPress::Skip,
            double_press: HeadsetPress::OneTurn,
        };
        let every = HeadsetCapabilities::EVERY;
        assert_eq!(
            choice.for_press(Presses::Single, true, every),
            HeadsetPress::Mute
        );
        assert_eq!(
            choice.for_press(Presses::Double, true, every),
            HeadsetPress::None
        );
        assert_eq!(
            choice.for_press(Presses::Single, false, every),
            HeadsetPress::Skip
        );
        assert_eq!(
            choice.for_press(Presses::Double, false, every),
            HeadsetPress::OneTurn
        );
    }
}
