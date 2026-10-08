//! What stays in memory, and for how long: sidevoice/sidevoice-core#21 D13, kept by the app. The engine unloads a model
//! when the last handle to it is dropped; this is who holds the handles. A model in memory is one per (engine, model,
//! accelerator), with the time its load took and its last use. The page loads and unloads them; with no call on, the
//! app frees what has gone unused for the idle time, and loads it again as the next call connects.
//!
//! Generic over what is held (`T`, a loaded model in the app), so the rules are tested without loading anything.

use crate::{epoch_ms, Loaded, Task};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

/// A model in memory: (engine, model, accelerator).
pub(crate) type Key = (String, String, String);

struct Resident<T> {
    instance: T,
    task: Task,
    load_ms: u64,
    since: SystemTime,
    /// The last use, for the idle rule (monotonic) and for the page (wall clock).
    used: Instant,
    used_at: SystemTime,
}

pub(crate) struct Residency<T> {
    resident: HashMap<Key, Resident<T>>,
    /// What `unload_idle` took out of memory (the newest of each task), to load again as the next call connects unless
    /// the page has unloaded it, or has another model of that task in memory by then.
    evicted: Vec<(Key, Task)>,
    /// How many unloads the page has made. A load (or preload) started before an unload of its build is not kept in
    /// memory: the unload is the page's later word (sidevoice-core#21 review R07).
    unloads: u64,
    /// The last unload of each build, per accelerator (`None`: all of them), with the count at it.
    retired: Vec<(u64, String, String, Option<String>)>,
}

impl<T> Default for Residency<T> {
    fn default() -> Self {
        Residency { resident: HashMap::new(), evicted: Vec::new(), unloads: 0, retired: Vec::new() }
    }
}

/// Whether a call is on, as the room reports it, and when the last one ended.
#[derive(Default)]
pub(crate) struct Calls {
    active: bool,
    ended: Option<Instant>,
}

impl Calls {
    /// The room says whether a call is on (joining counts). True when one has just started: the moment to preload.
    pub(crate) fn changed(&mut self, active: bool, now: Instant) -> bool {
        let started = active && !self.active;
        if self.active && !active {
            self.ended = Some(now);
        }
        self.active = active;
        started
    }
}

impl<T: Clone> Residency<T> {
    /// The page's unloads so far: what a load started now is checked against before it is kept.
    pub(crate) fn unloads(&self) -> u64 {
        self.unloads
    }

    /// Whether the page unloaded `key`'s build, on its accelerator or all of them, after unload number `since`.
    pub(crate) fn unloaded_since(&self, key: &Key, since: u64) -> bool {
        let covers = |a: &Option<String>| a.as_ref().is_none_or(|a| *a == key.2);
        self.retired
            .iter()
            .any(|(at, engine, model, a)| *at > since && *engine == key.0 && *model == key.1 && covers(a))
    }

    /// The model in memory under `key`, marked used now, with the time its load took.
    pub(crate) fn touch(&mut self, key: &Key) -> Option<(T, u64)> {
        let found = self.resident.get_mut(key)?;
        (found.used, found.used_at) = (Instant::now(), SystemTime::now());
        Some((found.instance.clone(), found.load_ms))
    }

    /// Keeps `instance`, loaded now, under `key` — unless the page unloaded it after unload number `since`, while it
    /// loaded: then it is not kept (it serves the call that loaded it, and goes). Answers whether it was kept.
    pub(crate) fn keep(&mut self, key: Key, instance: T, task: Task, load_ms: u64, since: u64) -> bool {
        if self.unloaded_since(&key, since) {
            return false;
        }
        let (at, used) = (SystemTime::now(), Instant::now());
        self.resident.insert(key, Resident { instance, task, load_ms, since: at, used, used_at: at });
        true
    }

    /// Frees `model` on `engine` on `accelerator`, or on every accelerator it is loaded on when none is named. A load
    /// of it still under way is not kept, and the app does not load it again as a call connects (D13).
    pub(crate) fn unload(&mut self, engine: &str, model: &str, accelerator: Option<&str>) {
        let ours = |(e, m, a): &Key| e == engine && m == model && accelerator.is_none_or(|x| x == a);
        self.unloads += 1;
        let at = self.unloads;
        self.resident.retain(|key, _| !ours(key));
        self.evicted.retain(|(key, _)| !ours(key));
        let accelerator = accelerator.map(str::to_string);
        self.retired.retain(|(_, e, m, a)| !(e == engine && m == model && *a == accelerator));
        self.retired.push((at, engine.to_string(), model.to_string(), accelerator));
    }

    /// The models in memory, oldest first.
    pub(crate) fn loaded(&self) -> Vec<Loaded> {
        let mut loaded: Vec<(SystemTime, Loaded)> =
            self.resident.iter().map(|(key, r)| (r.since, describe(key, r.since, r.used_at))).collect();
        loaded.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| (&a.1.model, &a.1.accelerator).cmp(&(&b.1.model, &b.1.accelerator)))
        });
        loaded.into_iter().map(|(_, l)| l).collect()
    }

    /// D13: with no call on, frees every model unused for `idle` — counted from its last use, or from when the last
    /// call ended if that is later — and remembers it for the next call. Answers what it freed.
    pub(crate) fn unload_idle(&mut self, calls: &Calls, idle: Duration, now: Instant) -> Vec<Loaded> {
        if calls.active {
            return Vec::new();
        }
        let is_idle = |r: &Resident<T>| {
            let from = calls.ended.map_or(r.used, |ended| ended.max(r.used));
            now.saturating_duration_since(from) >= idle
        };
        let mut keys: Vec<(SystemTime, Key)> =
            self.resident.iter().filter(|(_, r)| is_idle(r)).map(|(k, r)| (r.since, k.clone())).collect();
        keys.sort_by_key(|k| k.0); // oldest first: of two of one task, the newer is the one remembered
        let mut freed = Vec::new();
        for (_, key) in keys {
            let Some(r) = self.resident.remove(&key) else { continue };
            freed.push(describe(&key, r.since, r.used_at));
            self.evicted.retain(|(k, task)| *k != key && *task != r.task);
            self.evicted.push((key, r.task));
        }
        freed
    }

    /// What `unload_idle` freed and the next call should load again — not a task the page has another model of in
    /// memory by now — with the unload count to check those loads against. Taken: asked once per call.
    pub(crate) fn take_evicted(&mut self) -> (Vec<Key>, u64) {
        let replaced: Vec<Task> = self.resident.values().map(|r| r.task).collect();
        let evicted = std::mem::take(&mut self.evicted)
            .into_iter()
            .filter(|(_, task)| !replaced.contains(task))
            .map(|(key, _)| key)
            .collect();
        (evicted, self.unloads)
    }
}

fn describe((engine, model, accelerator): &Key, since: SystemTime, used_at: SystemTime) -> Loaded {
    Loaded {
        model: model.clone(),
        engine: engine.clone(),
        accelerator: accelerator.clone(),
        since: epoch_ms(since),
        last_used: epoch_ms(used_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: Duration = Duration::from_secs(600);

    fn key(model: &str, accelerator: &str) -> Key {
        ("sherpa-onnx".into(), model.into(), accelerator.into())
    }

    fn names(loaded: &[Loaded]) -> Vec<String> {
        loaded.iter().map(|l| format!("{}@{}/{}", l.model, l.engine, l.accelerator)).collect()
    }

    fn with(models: &[(&str, &str, Task)]) -> Residency<&'static str> {
        let mut residency = Residency::default();
        for (model, accelerator, task) in models {
            assert!(residency.keep(key(model, accelerator), "model", *task, 5, 0));
        }
        residency
    }

    #[test]
    fn d13_no_unload_during_a_call_then_the_idle_time_after_the_last_use_or_the_call_s_end() {
        let mut residency = with(&[("whisper-tiny", "cpu", Task::Stt)]);
        let mut calls = Calls::default();
        let start = Instant::now();
        assert!(calls.changed(true, start), "a call starting is the moment to preload");
        assert!(residency.unload_idle(&calls, IDLE, start + IDLE * 3).is_empty(), "never during a call");
        let ended = start + IDLE * 3;
        assert!(!calls.changed(false, ended));
        assert!(residency.unload_idle(&calls, IDLE, ended + IDLE / 2).is_empty(), "counted from the call's end");
        let freed = residency.unload_idle(&calls, IDLE, ended + IDLE);
        assert_eq!(names(&freed), ["whisper-tiny@sherpa-onnx/cpu"]);
        assert!(residency.loaded().is_empty());
    }

    #[test]
    fn what_an_idle_unload_freed_comes_back_with_the_next_call_unless_the_page_moved_on() {
        let mut residency = with(&[("whisper-tiny", "cpu", Task::Stt), ("kokoro-82m-v1.0", "cpu", Task::Tts)]);
        let calls = Calls::default();
        let freed = residency.unload_idle(&calls, IDLE, Instant::now() + IDLE);
        assert_eq!(freed.len(), 2);
        // The page loaded another speech-to-text model meanwhile: only the voice comes back.
        assert!(residency.keep(key("whisper-base", "cpu"), "model", Task::Stt, 5, residency.unloads()));
        let (evicted, _) = residency.take_evicted();
        assert_eq!(evicted, [key("kokoro-82m-v1.0", "cpu")]);
        assert!(residency.take_evicted().0.is_empty(), "taken once");

        // One the page unloaded after it was freed does not come back.
        let mut residency = with(&[("whisper-tiny", "cpu", Task::Stt)]);
        residency.unload_idle(&calls, IDLE, Instant::now() + IDLE);
        residency.unload("sherpa-onnx", "whisper-tiny", None);
        assert!(residency.take_evicted().0.is_empty());
    }

    #[test]
    fn r05_unloading_one_accelerator_frees_only_that_copy() {
        let mut residency = with(&[("whisper-base", "cpu", Task::Stt), ("whisper-base", "coreml", Task::Stt)]);
        residency.unload("sherpa-onnx", "whisper-base", Some("coreml"));
        assert_eq!(names(&residency.loaded()), ["whisper-base@sherpa-onnx/cpu"]);
        residency.unload("sherpa-onnx", "whisper-base", None);
        assert!(residency.loaded().is_empty());
    }

    #[test]
    fn r07_an_unload_during_a_load_wins_and_the_model_stays_out() {
        let mut residency: Residency<&str> = Residency::default();
        let since = residency.unloads();
        residency.unload("sherpa-onnx", "whisper-tiny", Some("cpu"));
        assert!(!residency.keep(key("whisper-tiny", "cpu"), "model", Task::Stt, 5, since));
        assert!(residency.loaded().is_empty());
        // Unloading another accelerator, or another model, does not touch this load.
        let since = residency.unloads();
        residency.unload("sherpa-onnx", "whisper-tiny", Some("coreml"));
        residency.unload("sherpa-onnx", "whisper-base", None);
        assert!(residency.keep(key("whisper-tiny", "cpu"), "model", Task::Stt, 5, since));
        // A load that starts after the unload is kept.
        assert!(residency.keep(key("whisper-tiny", "coreml"), "model", Task::Stt, 5, residency.unloads()));
        assert_eq!(residency.loaded().len(), 2);
    }

    #[test]
    fn a_model_in_memory_answers_the_time_its_load_took_and_is_marked_used() {
        let mut residency = with(&[("whisper-tiny", "cpu", Task::Stt)]);
        let before = residency.loaded()[0].last_used;
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(residency.touch(&key("whisper-tiny", "cpu")), Some(("model", 5)));
        assert!(residency.loaded()[0].last_used > before);
        assert_eq!(residency.touch(&key("whisper-tiny", "coreml")), None);
    }
}
