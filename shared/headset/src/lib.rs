//! What a headset's presses do, for every app that answers one (the desktop's macOS, the phones): the choices a person has
//! (`mute`, say one thing, skip what is playing, nothing), the change `host.shortcuts.setHeadset` asks for and the
//! `HeadsetStatus` `host.shortcuts.get` reports (docs/BRIDGE.md → "The shortcuts"). Pure: how a press reaches the app
//! (Now Playing, a media session) is `sidevoice-shared-tauri`'s, and where the choice is kept is each app's.
use serde::{Deserialize, Serialize};
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
    pub fn parse(name: &str) -> Option<HeadsetPress> {
        match name {
            "mute" => Some(HeadsetPress::Mute),
            "oneTurn" => Some(HeadsetPress::OneTurn),
            "skip" => Some(HeadsetPress::Skip),
            "none" => Some(HeadsetPress::None),
            _ => None,
        }
    }

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

/// What the person chose, as an app keeps it: the single press mutes by default and the double press does nothing, so that
/// a keyboard's next-track key, which arrives the same way, does nothing until chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HeadsetChoice {
    pub press: HeadsetPress,
    pub double_press: HeadsetPress,
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
    /// Choosing anything but the defaults shows a playback card beside the call's own surface (iOS, with the Live Activity), so the
    /// page says so. Left out when false.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_default_adds_card: bool,
}

impl Headset {
    pub fn new(press: HeadsetPress, double_press: HeadsetPress) -> Headset {
        Headset {
            press,
            double_press: Some(double_press),
            outside_call: "leave",
            managed_by_system: false,
            non_default_adds_card: false,
        }
    }

    /// The status of a `choice`; `managed_by_system`: the system's call holds the headset; `adds_card`: a choice other than the
    /// defaults shows a playback card here ([`now_playing_gives_way`]).
    pub fn of(choice: HeadsetChoice, managed_by_system: bool, adds_card: bool) -> Headset {
        Headset {
            managed_by_system,
            non_default_adds_card: adds_card,
            ..Headset::new(choice.press, choice.double_press)
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
    /// Whether this is the defaults (single press mute, double press nothing): what the system's own mute handles, with no remote
    /// command of the app needed.
    pub fn is_default(self) -> bool {
        self == HeadsetChoice::default()
    }
}

/// Whether the playback card (Now Playing, and with it the app's remote commands) gives way to the call's own surface (iOS: the
/// Live Activity), as `replaced` says. Only the defaults give way: mute comes from the system's mute gesture and the surface's
/// button. A choice of any other action needs the remote commands, so the card stays for it.
pub fn now_playing_gives_way(replaced: bool, choice: HeadsetChoice) -> bool {
    replaced && choice.is_default()
}

impl HeadsetChoice {
    /// What `which` does now. While the system's call holds the headset (`managed_by_system`) the system mutes: the single
    /// press is a mute, whatever was chosen, and the double press nothing.
    pub fn for_press(self, which: Presses, managed_by_system: bool) -> HeadsetPress {
        match (which, managed_by_system) {
            (Presses::Single, false) => self.press,
            (Presses::Double, false) => self.double_press,
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
        let own =
            serde_json::to_value(Headset::of(HeadsetChoice::default(), false, false)).unwrap();
        assert_eq!(
            own,
            json!({"press": "mute", "doublePress": "none", "outsideCall": "leave"})
        );
        let system =
            serde_json::to_value(Headset::of(HeadsetChoice::default(), true, false)).unwrap();
        assert_eq!(system["managedBySystem"], json!(true));
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
    fn the_card_gives_way_to_the_call_s_surface_only_for_the_defaults() {
        let defaults = HeadsetChoice::default();
        let single = HeadsetChoice {
            press: HeadsetPress::OneTurn,
            ..defaults
        };
        let double = HeadsetChoice {
            double_press: HeadsetPress::Skip,
            ..defaults
        };
        assert!(
            now_playing_gives_way(true, defaults),
            "the Live Activity alone, as designed"
        );
        assert!(
            !now_playing_gives_way(true, single),
            "another single press needs the remote commands"
        );
        assert!(
            !now_playing_gives_way(true, double),
            "and so does a double press"
        );
        assert!(
            !now_playing_gives_way(false, defaults),
            "no surface of its own: the card stays"
        );
        assert!(defaults.is_default() && !single.is_default() && !double.is_default());
    }

    #[test]
    fn the_system_s_call_leaves_only_its_mute() {
        let choice = HeadsetChoice {
            press: HeadsetPress::Skip,
            double_press: HeadsetPress::OneTurn,
        };
        assert_eq!(choice.for_press(Presses::Single, true), HeadsetPress::Mute);
        assert_eq!(choice.for_press(Presses::Double, true), HeadsetPress::None);
        assert_eq!(choice.for_press(Presses::Single, false), HeadsetPress::Skip);
        assert_eq!(
            choice.for_press(Presses::Double, false),
            HeadsetPress::OneTurn
        );
    }
}
