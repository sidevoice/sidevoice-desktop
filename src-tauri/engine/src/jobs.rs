//! The page's installs, one job per `install` call: its progress once it starts, and its cancel from the moment it
//! reaches the app (docs/BRIDGE.md → "The native engine").

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// the moment its call reaches the app, and can be cancelled from then on, waiting or running; it reports progress
/// once it starts — once it holds the install lock — until it ends: a job still waiting for another reports nothing,
/// and no job ever reports another's bytes.
#[derive(Default)]
pub struct Jobs(Mutex<JobsState>);

#[derive(Default)]
struct JobsState {
    jobs: HashMap<String, Job>,
    /// Cancelled before their install reached the app (the page's two calls may cross): refused as they arrive.
    early: Vec<String>,
    /// Jobs that ended lately, so a cancel that comes after the end is told so.
    ended: Vec<String>,
}

struct Job {
    model: String,
    engine: String,
    cancelled: bool,
    /// `[done, total]`, once started.
    bytes: Option<[u64; 2]>,
    /// The last speed sample (when, bytes done then) and the smoothed speed.
    sample: Option<(Instant, u64)>,
    bytes_per_s: Option<f64>,
}

/// What `engine_progress` answers for a job that has started.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Progress {
    pub job: String,
    pub model: String,
    pub engine: String,
    pub done: u64,
    pub total: u64,
    /// Smoothed over the last seconds; `None` until the first second is measured.
    pub bytes_per_s: Option<u64>,
}

/// How often the download speed is sampled; each sample weighs half against the speed so far.
const SPEED_SAMPLE: Duration = Duration::from_secs(1);
/// How many early cancels and ended jobs are remembered: ids are not kept forever.
const REMEMBERED: usize = 64;

impl Jobs {
    fn state(&self) -> MutexGuard<'_, JobsState> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The job, known from now on (waiting for the install lock until it starts). False when the page cancelled it
    /// before it got here.
    pub fn begin(&self, job: &str, model: &str, engine: &str) -> bool {
        let mut state = self.state();
        if let Some(at) = state.early.iter().position(|j| j == job) {
            state.early.remove(at);
            state.ended.push(job.to_string());
            return false;
        }
        let entry = Job {
            model: model.into(),
            engine: engine.into(),
            cancelled: false,
            bytes: None,
            sample: None,
            bytes_per_s: None,
        };
        state.jobs.insert(job.to_string(), entry);
        true
    }

    /// `job`'s progress once it has started, while it runs.
    pub fn get(&self, job: &str) -> Option<Progress> {
        let state = self.state();
        let entry = state.jobs.get(job)?;
        let [done, total] = entry.bytes?;
        Some(Progress {
            job: job.to_string(),
            model: entry.model.clone(),
            engine: entry.engine.clone(),
            done,
            total,
            bytes_per_s: entry.bytes_per_s.map(|b| b.round() as u64),
        })
    }

    /// Cancels `job`: true when its install will reject with `install_cancelled` — waiting, running (it stops at the
    /// next chunk or archive entry and removes what it was downloading), or not arrived yet (it is refused as it
    /// arrives); false when it has already ended.
    pub fn cancel(&self, job: &str) -> bool {
        let mut state = self.state();
        if let Some(entry) = state.jobs.get_mut(job) {
            entry.cancelled = true;
            return true;
        }
        if state.ended.iter().any(|j| j == job) {
            return false;
        }
        if state.early.len() >= REMEMBERED {
            state.early.remove(0);
        }
        state.early.push(job.to_string());
        true
    }

    pub(crate) fn cancelled(&self, job: &str) -> bool {
        self.state().jobs.get(job).is_some_and(|j| j.cancelled)
    }

    pub(crate) fn set(&self, job: &str, done: u64, total: u64) {
        self.set_at(job, done, total, Instant::now());
    }

    fn set_at(&self, job: &str, done: u64, total: u64, now: Instant) {
        let mut state = self.state();
        let Some(entry) = state.jobs.get_mut(job) else { return };
        entry.bytes = Some([done, total]);
        match entry.sample {
            None => entry.sample = Some((now, done)),
            Some((then, before)) if now.duration_since(then) >= SPEED_SAMPLE => {
                let speed = done.saturating_sub(before) as f64 / now.duration_since(then).as_secs_f64();
                entry.bytes_per_s = Some(entry.bytes_per_s.map_or(speed, |s| (s + speed) / 2.0));
                entry.sample = Some((now, done));
            }
            Some(_) => {}
        }
    }

    /// Ends `job`: false when it was cancelled, true otherwise. From here on a cancel no longer reaches it.
    pub(crate) fn finish(&self, job: &str) -> bool {
        let mut state = self.state();
        if state.ended.len() >= REMEMBERED {
            state.ended.remove(0);
        }
        state.ended.push(job.to_string());
        state.jobs.remove(job).is_some_and(|j| !j.cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_carries_the_job_its_build_and_a_smoothed_speed() {
        let jobs = Jobs::default();
        assert!(jobs.begin("s", "kokoro-82m-v1.0", "sherpa-onnx"));
        assert_eq!(jobs.get("s"), None, "nothing until it starts");
        let start = Instant::now();
        let mib = 1024 * 1024;
        jobs.set_at("s", 0, 10 * mib, start);
        let first = jobs.get("s").unwrap();
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::json!({
                "job": "s", "model": "kokoro-82m-v1.0", "engine": "sherpa-onnx", "done": 0, "total": 10 * mib, "bytes_per_s": null
            })
        );
        jobs.set_at("s", mib / 2, 10 * mib, start + Duration::from_millis(500));
        assert_eq!(jobs.get("s").unwrap().bytes_per_s, None, "not a second measured yet");
        jobs.set_at("s", mib, 10 * mib, start + Duration::from_secs(1));
        assert_eq!(jobs.get("s").unwrap().bytes_per_s, Some(mib));
        jobs.set_at("s", 4 * mib, 10 * mib, start + Duration::from_secs(2));
        assert_eq!(jobs.get("s").unwrap().bytes_per_s, Some(2 * mib), "half the last second's 3 MiB/s, half before");
    }

    #[test]
    fn a_cancel_reaches_a_job_running_waiting_or_not_arrived_and_never_one_that_ended() {
        let jobs = Jobs::default();
        jobs.begin("a", "whisper-small", "sherpa-onnx");
        assert!(jobs.cancel("a"));
        assert!(jobs.cancelled("a"));
        assert!(!jobs.finish("a"), "it ends cancelled");
        assert!(!jobs.cancel("a"), "an ended job is not cancelled again");

        assert!(jobs.cancel("early"), "not arrived yet: it will be refused");
        assert!(!jobs.begin("early", "whisper-small", "sherpa-onnx"), "refused as it arrives");
        assert!(!jobs.cancel("early"), "and now it has ended");

        jobs.begin("b", "whisper-tiny", "sherpa-onnx");
        assert!(jobs.finish("b"), "it ends as it ran");
        assert_eq!(jobs.get("b"), None);
    }
}
