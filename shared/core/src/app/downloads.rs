//! What the app does around its model downloads: told when the first one starts and when the last one ends, however
//! many run in between. A phone keeps its screen on for that long, since the system suspends the app, and the download
//! with it, once the screen turns off.

use std::sync::{Arc, Mutex};

/// What the app does while a model download runs. Both calls must return quickly (no waiting for the system): they are
/// made under the host's count, so that the first start and the last end arrive in order.
pub trait DownloadActivity: Send + Sync + 'static {
    /// The first download starts (none was running).
    fn started(&self);
    /// The last download has ended, whether it finished, failed or was cancelled.
    fn ended(&self);
}

/// An app with nothing to do around its downloads (the desktop: its process runs on while the screen is off).
pub struct NoDownloadActivity;

impl DownloadActivity for NoDownloadActivity {
    fn started(&self) {}

    fn ended(&self) {}
}

/// How many downloads run, and the app's [`DownloadActivity`], told the first start and the last end.
pub(super) struct Downloads {
    running: Mutex<usize>,
    activity: Arc<dyn DownloadActivity>,
}

impl Downloads {
    pub(super) fn new(activity: Arc<dyn DownloadActivity>) -> Arc<Self> {
        Arc::new(Self { running: Mutex::new(0), activity })
    }

    /// A download starts; it ends when the returned value is dropped, wherever the download ends.
    pub(super) fn begin(self: &Arc<Self>) -> Running {
        let mut running = self.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *running == 0 {
            self.activity.started();
        }
        *running += 1;
        Running(Arc::clone(self))
    }
}

/// One running download.
pub(super) struct Running(Arc<Downloads>);

impl Drop for Running {
    fn drop(&mut self) {
        let mut running = self.0.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *running -= 1;
        if *running == 0 {
            self.0.activity.ended();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Log(Mutex<Vec<&'static str>>);

    impl DownloadActivity for Log {
        fn started(&self) {
            self.0.lock().unwrap().push("started");
        }

        fn ended(&self) {
            self.0.lock().unwrap().push("ended");
        }
    }

    #[test]
    fn the_first_start_and_the_last_end_are_told_once_however_many_downloads_overlap() {
        let log = Arc::new(Log::default());
        let downloads = Downloads::new(log.clone());
        let first = downloads.begin();
        let second = downloads.begin();
        assert_eq!(*log.0.lock().unwrap(), ["started"]);
        drop(first);
        assert_eq!(*log.0.lock().unwrap(), ["started"]);
        drop(second);
        assert_eq!(*log.0.lock().unwrap(), ["started", "ended"]);
        drop(downloads.begin());
        assert_eq!(*log.0.lock().unwrap(), ["started", "ended", "started", "ended"]);
    }
}
