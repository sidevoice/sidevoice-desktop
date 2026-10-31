//! The voice call's commands (`window.__sidevoiceDesktop.host.voice`, docs/BRIDGE.md → "The voice call"): sidevoice-voice's
//! `VoiceCall` with the models the app picks on its engine, the device's own microphone and speaker, the system's
//! default ones or those the page chooses, and Sonora's echo cancellation between them. A call has an input half, an
//! output half or both, and loads and opens only what its halves use; the page changes it while it runs
//! ([`AppHost::update`]) and loads models ahead of it ([`AppHost::preload`]). The page keeps the room, which the call
//! knows nothing of: it tells the room the call's turns, and has the call say what the room sends.
//!
//! Each command takes the generation of the page that sent it ([`PageGeneration`]) and is refused as `page_stale` when that
//! is not the current page: a page that reloaded must not move the calls of the one that replaced it.
//!
//! The calls themselves (the lanes of the shared devices, their lifecycle, what a stopped call or a preload keeps loaded)
//! are `voice::host`'s; this is what the page's commands do with them: admit, choose the models from the engine's
//! catalogues, make the call, tell the page.

use std::sync::Arc;

use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use sidevoice_voice::AudioDevices;

use super::{AppHost, PageGeneration};
use crate::engine::NativeEngines;
use crate::voice::host::{failed, refusal, refused, Applied, LaneAudio, Refusal, IDLE_UNLOAD_MINUTES};
use crate::voice::models::{EngineLoader, Halves};
use crate::voice::{candidates, choose, CatalogCandidates, SettingsChange, VoiceChoice, VoiceSettings};

/// What [`AppHost::set_audio_devices`] chooses: a missing kind stays as it is, `null` is the system's default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioChoice {
    #[serde(default, deserialize_with = "present")]
    input: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    output: Option<Option<String>>,
}

/// A field that is there, `null` included.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

fn reset_busy() -> Refusal {
    refusal("reset.busy", "Data reset is under way.")
}

impl<A: LaneAudio> AppHost<A> {
    fn admit(&self) -> Result<crate::app::Lease, Refusal> {
        self.inner.hooks.admission.admit().ok_or_else(reset_busy)
    }

    fn engines_for_call(&self) -> Result<Arc<NativeEngines>, Refusal> {
        self.inner.engines.clone().ok_or_else(|| refusal("engine_unavailable", "The engine did not start."))
    }

    /// What the app makes of `settings` on the engine's catalogues as they list their models now: the model of each slot
    /// the halves use and the configuration, or the refusal that says why not.
    async fn choice_of(&self, engines: &Arc<NativeEngines>, settings: &VoiceSettings) -> Result<VoiceChoice, Refusal> {
        let catalogs = {
            let engines = Arc::clone(engines);
            self.inner
                .runtime
                .spawn_blocking(move || engines.catalogs())
                .await
                .map_err(|e| refusal("internal", e.to_string()))?
                .map_err(|e| refusal(e.key, e.message))?
        };
        let candidates: Vec<CatalogCandidates> = catalogs.iter().map(candidates).collect();
        let choice = choose(settings, &candidates).map_err(refused)?;
        crate::debug::debug(&format!("voice settings: {}", serde_json::to_string(&choice).unwrap_or_default()));
        Ok(choice)
    }

    /// Attaches a lane now, so routing may be chosen before its start.
    pub async fn create_call(&self, page: PageGeneration, key: String) -> Result<(), Refusal> {
        self.current(page)?;
        self.entered(self.inner.voice.create_call_in(key, page.0)).await
    }

    /// The acknowledged next-frame microphone route; `None` sends silence to all running lanes.
    pub async fn route_input(&self, page: PageGeneration, key: Option<String>) -> Result<(), Refusal> {
        self.current(page)?;
        self.entered(self.inner.voice.route_input_in(page.0, key)).await
    }

    /// Stops the call, hands the page what it still had to tell, and releases its lane. The models it loaded stay for
    /// their idle minutes.
    pub async fn dispose_call(&self, page: PageGeneration, key: &str) -> Result<(), Refusal> {
        self.current(page)?;
        self.entered(self.inner.voice.dispose_call(key)).await
    }

    /// Starts the lane's call with the person's choices (`VoiceSettings`, the seam's: an `input` half, an `output` half
    /// or both): each slot is a model of one of the engine's catalogues (`{catalog, model}`, checked against what it
    /// lists); the app adds the detector (for an input half) and the end-of-turn model (for `smart`), and makes the
    /// configuration (every number the person does not choose). Answers once the call is made and asked to start; it
    /// loads the models its halves use (installing them if they are not), opens the speaker and, with an input half,
    /// the microphone, and [`AppHost::ready`] says when it works. With `take`, the call keeps what the microphone gives
    /// from then until that listening ends, and hands it to the page then (`{type: "take", data}`).
    pub async fn start(
        &self,
        page: PageGeneration,
        key: &str,
        settings: VoiceSettings,
        take: bool,
    ) -> Result<(), Refusal> {
        self.current(page)?;
        let (host, key) = (self.clone(), key.to_owned());
        self.detached(async move { host.started(page, &key, settings, take).await }).await
    }

    async fn started(
        &self,
        page: PageGeneration,
        key: &str,
        settings: VoiceSettings,
        take: bool,
    ) -> Result<(), Refusal> {
        let _lease = self.admit()?;
        // The page's own lane, carried through the awaits below: what is done at the end is done to it, whatever is made
        // under its key meanwhile.
        let lane = self.inner.voice.lane_of(page.0, key).await?;
        let engines = self.engines_for_call()?;
        let choice = self.choice_of(&engines, &settings).await?;
        self.loading_allowed()?;
        let loader = EngineLoader::shared(engines.engine(), Arc::clone(&self.inner.voice.model_cache));
        let applied = Applied { halves: Halves::new(&loader, &choice, None), settings };
        self.inner.voice.open(&lane, key, applied, take, self.page_sink(page)).await
    }

    /// Resolves once the call works: at its first state that is not idle (`off` for a call without an input half);
    /// rejects with the call's error code, or `stopped` when [`AppHost::stop`] comes first.
    pub async fn ready(&self, page: PageGeneration, key: &str) -> Result<(), Refusal> {
        self.current(page)?;
        self.entered(async {
            let _lease = self.admit()?;
            self.inner.voice.ready(key).await
        })
        .await
    }

    /// Changes the call while it runs (`SettingsChange`: a half that is absent stays, `null` removes it, settings
    /// replace it or add it), without restarting it: a half whose slots keep their models keeps what it loaded, other
    /// models load while the call goes on as it was, and each half's change takes effect at its safe point (the input
    /// half's at the next turn, the output half's at the next utterance). Changes are made in the order they come.
    /// Resolves once the change is accepted, not applied; refuses what leaves the call no half, or a model the app
    /// cannot make, changing nothing. A model that cannot load is an error of the call with its code, and its half stays
    /// as it was.
    pub async fn update(&self, page: PageGeneration, key: &str, change: SettingsChange) -> Result<(), Refusal> {
        self.current(page)?;
        let (host, key) = (self.clone(), key.to_owned());
        self.detached(async move { host.updated(page, &key, change).await }).await
    }

    async fn updated(&self, page: PageGeneration, key: &str, change: SettingsChange) -> Result<(), Refusal> {
        let lane = self.inner.voice.lane_of(page.0, key).await?;
        let _turn = lane.changing.lock().await;
        let _lease = self.admit()?;
        let current = self.inner.voice.applied(&lane).await?;
        let settings = current.settings.changed(change);
        let engines = self.engines_for_call()?;
        let choice = self.choice_of(&engines, &settings).await?;
        self.loading_allowed()?;
        let loader = EngineLoader::shared(engines.engine(), Arc::clone(&self.inner.voice.model_cache));
        let applied = Applied { halves: Halves::new(&loader, &choice, Some(&current.halves)), settings };
        self.inner.voice.update(&lane, applied).await
    }

    /// Loads the models `settings` use into the host, ahead of a call: the slots a [`AppHost::start`] of them would ask
    /// for (an absent half loads none of its own, `silence` no end-of-turn model), once for a model already loaded or
    /// loading, opening no microphone and no speaker. They are held like a stopped call's: a call that starts on the
    /// same models takes them without loading, and they are let go `idle_unload_minutes` (10 by default) after the end of
    /// the load. Resolves once every slot is loaded; rejects `voice_failed` with the code of a slot that cannot load,
    /// what else loaded staying loaded.
    pub async fn preload(&self, page: PageGeneration, settings: VoiceSettings) -> Result<(), Refusal> {
        self.current(page)?;
        let host = self.clone();
        self.detached(async move { host.preloaded(settings).await }).await
    }

    async fn preloaded(&self, settings: VoiceSettings) -> Result<(), Refusal> {
        let _lease = self.admit()?;
        let engines = self.engines_for_call()?;
        let choice = self.choice_of(&engines, &settings).await?;
        self.loading_allowed()?;
        let loader = EngineLoader::shared(engines.engine(), Arc::clone(&self.inner.voice.model_cache));
        let minutes = settings.idle_unload_minutes.unwrap_or(IDLE_UNLOAD_MINUTES);
        self.inner
            .voice
            .preload(Halves::new(&loader, &choice, None), minutes)
            .await
            .map_err(|code| failed("The models could not be loaded", code))
    }

    /// Stops listening and speaking; the lane stays until [`AppHost::dispose_call`]. A [`AppHost::ready`] still waiting
    /// rejects with `stopped`. Answers once the page has what the stop ends (`true`), or at once with `false` before the
    /// call starts.
    pub async fn stop(&self, page: PageGeneration, key: &str) -> Result<bool, Refusal> {
        self.current(page)?;
        self.entered(self.inner.voice.stop(key)).await
    }

    /// Says text on one call; its keyed events return only to that call's handle.
    pub async fn say(
        &self,
        page: PageGeneration,
        key: &str,
        say_key: String,
        text: String,
        language: Option<String>,
    ) -> Result<(), Refusal> {
        self.current(page)?;
        self.entered(self.inner.voice.say(key, say_key, text, language, self.page_sink(page))).await
    }

    /// Cancels what is being said under `say_key`: the part not yet heard is dropped, and its outcome says `cancelled`.
    /// Once it has ended, nothing.
    pub async fn cancel_say(&self, page: PageGeneration, key: &str, say_key: &str) -> Result<(), Refusal> {
        self.current(page)?;
        self.entered(self.inner.voice.cancel_say(key, say_key)).await
    }

    /// Mutes or unmutes the microphone for this call: it hears silence, and an open turn ends only as silence ends it.
    /// Kept from the call's creation, before it starts too.
    pub async fn mute(&self, page: PageGeneration, key: &str, muted: bool) -> Result<(), Refusal> {
        self.current(page)?;
        self.entered(self.inner.voice.mute(key, muted)).await
    }

    /// Cancels what the person said that is not reported yet; nothing before the call starts.
    pub async fn cancel_input(&self, page: PageGeneration, key: &str) -> Result<(), Refusal> {
        self.current(page)?;
        self.entered(self.inner.voice.cancel_input(key)).await
    }

    /// Closes the open turn now with what was captured so far: it is transcribed and told like any turn. It neither
    /// mutes, nor touches the playback or how later turns end. Resolves once the turn is closed, also when there is
    /// nothing to close (the call not started, no open turn, the call stopped).
    pub async fn end_input(&self, page: PageGeneration, key: &str) -> Result<(), Refusal> {
        self.current(page)?;
        self.entered(self.inner.voice.end_input(key)).await
    }

    /// The system's microphones and speakers and the ones chosen (`null`: the system's default), as the seam's
    /// `VoiceAudioDevices`: `{inputs, outputs, input, output, outputSelectable, lost?}`, each device `{id, label}`. A
    /// chosen device that went away is the default one from then, said once as `lost`. From now on the page is told
    /// when they change (`{type: "devices", data}`).
    pub async fn audio_devices(&self, page: PageGeneration) -> Result<Value, Refusal> {
        self.current(page)?;
        self.watch_devices();
        let devices = self.inner.voice.devices.clone();
        listed(self.inner.runtime.spawn_blocking(move || devices.list()).await)
    }

    /// Chooses the microphone, the speaker or both, at any time: a call that runs switches its microphone at once and
    /// its speaker once what it is playing has ended. Answers the devices as [`AppHost::audio_devices`] does; rejects
    /// `{key: "voice_audio_device_unknown", code: "audio-device-unknown"}` for an id the system does not list, nothing
    /// changed.
    pub async fn set_audio_devices(&self, page: PageGeneration, choice: AudioChoice) -> Result<Value, Refusal> {
        self.current(page)?;
        self.watch_devices();
        let devices = self.inner.voice.devices.clone();
        listed(self.inner.runtime.spawn_blocking(move || devices.choose(choice.input, choice.output)).await)
    }
}

/// The devices as the seam has them, or the app's refusal with the code the system's listing gave.
fn listed(listing: Result<Result<AudioDevices, String>, tokio::task::JoinError>) -> Result<Value, Refusal> {
    let devices = listing.map_err(|e| refusal("internal", e.to_string()))?.map_err(|code| {
        let mut refused = refusal(&format!("voice_{}", code.replace('-', "_")), format!("The audio devices: {code}."));
        refused.params.insert("code".into(), json!(code));
        refused
    })?;
    Ok(json!(devices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_choice_tells_null_from_absent() {
        let choice: AudioChoice = serde_json::from_value(json!({"input": null})).unwrap();
        assert_eq!((choice.input, choice.output), (Some(None), None));
        let choice: AudioChoice = serde_json::from_value(json!({"output": "usb", "input": "mic"})).unwrap();
        assert_eq!((choice.input, choice.output), (Some(Some("mic".into())), Some(Some("usb".into()))));
        assert!(serde_json::from_value::<AudioChoice>(json!({"speaker": "x"})).is_err(), "an unknown key is refused");
    }
}
