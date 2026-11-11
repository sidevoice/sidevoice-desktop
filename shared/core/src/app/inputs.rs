//! The microphones, where the system's route is the one that chooses them (iOS: the audio session's available inputs and
//! its preferred input). The voice lists and opens its devices through cpal, which on such a system sees one default
//! input; the person's microphone is what the session routes to the call (the phone's own, a car's hands-free, AirPods).
//! The app implements [`InputRoute`] over that system; here the list the page is shown is made from it, and a choice is
//! taken by it, so that the page has one `audioDevices` whichever system the app runs on.
//!
//! Where the app has no such route ([`NoInputRoute`]: the desktop, whose devices are cpal's own) nothing here changes
//! what the voice says.

use std::sync::{Arc, Mutex};

use sidevoice_voice::{AudioDevice, AudioDevices, LostDevices};

/// The microphones the system's route offers, and the one it prefers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInputs {
    /// Each with the system's own id and name.
    pub inputs: Vec<AudioDevice>,
    /// The id of the input asked for (`None`: the system's choice, whatever the default route is).
    pub chosen: Option<String>,
}

/// What the app implements to have the system's route choose the microphone. Both calls may wait for the system: call them
/// off the runtime's threads.
pub trait InputRoute: Send + Sync + 'static {
    /// The microphones the system's route offers now; `None` where this app's microphones are the voice's own list.
    fn list(&self) -> Option<SystemInputs>;
    /// Asks the system to route the call's microphone from input `id` (`None`: leave it to the system). A stable code on
    /// refusal (`audio-device-unknown`, `audio-input-failed`).
    fn choose(&self, id: Option<&str>) -> Result<(), String>;
}

/// An app whose microphones are the voice's own (the desktop's).
pub struct NoInputRoute;

impl InputRoute for NoInputRoute {
    fn list(&self) -> Option<SystemInputs> {
        None
    }

    fn choose(&self, _id: Option<&str>) -> Result<(), String> {
        Err("audio-device-unknown".into())
    }
}

/// The app's [`InputRoute`], and what the person asked of it last: a microphone asked for that is no longer offered is said
/// once as lost, the system's choice from then on.
pub(super) struct Inputs {
    route: Arc<dyn InputRoute>,
    asked: Mutex<Option<String>>,
}

impl Inputs {
    pub(super) fn new(route: Arc<dyn InputRoute>) -> Self {
        Self { route, asked: Mutex::new(None) }
    }

    /// The part of a choice of microphone the system's route takes: all of it where there is one (nothing is left for the
    /// voice), none otherwise. `audio-device-unknown` for an id the route does not offer, nothing changed.
    pub(super) fn take(&self, wanted: Option<Option<String>>) -> Result<Option<Option<String>>, String> {
        let Some(id) = wanted else {
            return Ok(None);
        };
        let Some(system) = self.route.list() else {
            return Ok(Some(id));
        };
        if id.as_ref().is_some_and(|id| !system.inputs.iter().any(|input| &input.id == id)) {
            return Err("audio-device-unknown".into());
        }
        self.route.choose(id.as_deref())?;
        *self.asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = id;
        Ok(None)
    }

    /// The voice's list with the microphones the route offers in place of its own, and the route's choice as the chosen one.
    pub(super) fn apply(&self, mut devices: AudioDevices) -> AudioDevices {
        let Some(system) = self.route.list() else {
            return devices;
        };
        let chosen = system.chosen.filter(|id| system.inputs.iter().any(|input| &input.id == id));
        let mut asked = self.asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match &chosen {
            Some(id) => *asked = Some(id.clone()),
            None => {
                if let Some(gone) = asked.take() {
                    // The input asked for went away: the system's choice from now on, said once.
                    let _ = self.route.choose(None);
                    devices.lost.get_or_insert_with(LostDevices::default).input = Some(gone);
                }
            }
        }
        devices.inputs = system.inputs;
        devices.input = chosen;
        devices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str) -> AudioDevice {
        AudioDevice { id: id.into(), label: id.to_uppercase() }
    }

    fn voice() -> AudioDevices {
        AudioDevices {
            inputs: vec![input("default")],
            outputs: vec![input("out")],
            input: None,
            output: None,
            lost: None,
        }
    }

    /// An iPhone-like route: the inputs it offers, and the one it was asked for.
    struct Fake {
        inputs: Mutex<Vec<AudioDevice>>,
        chosen: Mutex<Option<String>>,
    }

    impl Fake {
        fn offering(ids: &[&str]) -> Arc<Fake> {
            Arc::new(Fake { inputs: Mutex::new(ids.iter().map(|id| input(id)).collect()), chosen: Mutex::new(None) })
        }
    }

    impl InputRoute for Fake {
        fn list(&self) -> Option<SystemInputs> {
            Some(SystemInputs {
                inputs: self.inputs.lock().unwrap().clone(),
                chosen: self.chosen.lock().unwrap().clone(),
            })
        }

        fn choose(&self, id: Option<&str>) -> Result<(), String> {
            *self.chosen.lock().unwrap() = id.map(str::to_owned);
            Ok(())
        }
    }

    #[test]
    fn without_a_route_the_voice_is_left_as_it_is() {
        let inputs = Inputs::new(Arc::new(NoInputRoute));
        assert_eq!(inputs.take(Some(Some("x".into()))), Ok(Some(Some("x".into()))));
        assert_eq!(inputs.take(None), Ok(None));
        assert_eq!(inputs.apply(voice()), voice());
    }

    #[test]
    fn the_routes_microphones_replace_the_voices_and_a_choice_is_the_routes() {
        let route = Fake::offering(&["phone", "car"]);
        let inputs = Inputs::new(route.clone());
        assert_eq!(inputs.take(Some(Some("car".into()))), Ok(None), "nothing is left for the voice");
        let listed = inputs.apply(voice());
        assert_eq!(listed.inputs, vec![input("phone"), input("car")]);
        assert_eq!(listed.input.as_deref(), Some("car"));
        assert_eq!(listed.outputs, voice().outputs, "the speakers stay the voice's");
        assert_eq!(route.chosen.lock().unwrap().as_deref(), Some("car"));
        assert_eq!(inputs.take(Some(None)), Ok(None));
        assert_eq!(inputs.apply(voice()).input, None);
        assert_eq!(route.chosen.lock().unwrap().as_deref(), None);
    }

    #[test]
    fn an_input_the_route_does_not_offer_is_refused_and_nothing_changes() {
        let route = Fake::offering(&["phone"]);
        let inputs = Inputs::new(route.clone());
        assert_eq!(inputs.take(Some(Some("car".into()))), Err("audio-device-unknown".into()));
        assert_eq!(*route.chosen.lock().unwrap(), None);
    }

    #[test]
    fn an_input_that_went_away_is_said_once_and_the_system_chooses_again() {
        let route = Fake::offering(&["phone", "pods"]);
        let inputs = Inputs::new(route.clone());
        inputs.take(Some(Some("pods".into()))).unwrap();
        assert_eq!(inputs.apply(voice()).input.as_deref(), Some("pods"));
        route.inputs.lock().unwrap().retain(|input| input.id != "pods");
        let listed = inputs.apply(voice());
        assert_eq!(listed.input, None);
        assert_eq!(listed.lost, Some(LostDevices { input: Some("pods".into()), output: None }));
        assert_eq!(*route.chosen.lock().unwrap(), None);
        assert_eq!(inputs.apply(voice()).lost, None, "said once");
    }

    #[test]
    fn a_choice_the_system_took_back_is_reported_as_it_is() {
        // iOS may drop the preference by itself while the input is still there: the list says what the system does.
        let route = Fake::offering(&["phone", "car"]);
        let inputs = Inputs::new(route.clone());
        inputs.take(Some(Some("car".into()))).unwrap();
        *route.chosen.lock().unwrap() = None;
        let listed = inputs.apply(voice());
        assert_eq!(listed.input, None);
        assert_eq!(listed.lost, Some(LostDevices { input: Some("car".into()), output: None }));
    }
}
