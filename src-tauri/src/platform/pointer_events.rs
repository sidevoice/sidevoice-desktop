//! Owned pointer observation. Workers only schedule bounded main-thread work; they never query windows or wait for UI.
use super::desktop_geometry::{DesktopGeometry, NativeGeometry};
use sidevoice_desktop_core::call_controls::Rect;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tauri::{AppHandle, Manager};

#[cfg_attr(target_os = "macos", path = "pointer_events/macos.rs")]
#[cfg_attr(target_os = "windows", path = "pointer_events/windows.rs")]
#[cfg_attr(target_os = "linux", path = "pointer_events/linux.rs")]
mod imp;
pub use imp::NativePointerEvents;

pub struct PointerState {
    pub shown: bool,
    pub hand: bool,
    pub inside: Option<bool>,
}
pub enum PointerEvent {
    Inside(Option<bool>),
    OutsideClick,
}
pub type State = Arc<dyn Fn() -> PointerState + Send + Sync>;
pub type Sink = Arc<dyn Fn(PointerEvent) + Send + Sync>;

pub trait PointerEvents {
    /// Install during main-thread setup. State is read without holding native locks; callbacks run on the main
    /// thread. `None` means global observation is unavailable (Wayland uses the page's own events).
    /// Drop the subscription outside application locks to cancel queued callbacks and remove native monitors.
    fn watch(app: &AppHandle, label: &'static str, margin: f64, state: State, sink: Sink) -> Option<Subscription>;
}

/// Owned cancellation and native cleanup. Dropping never joins a worker or waits for the main thread.
/// A callback already executing may finish; queued callbacks check cancellation before accessing the app.
pub struct Subscription {
    stopped: Arc<AtomicBool>,
    task: tauri::async_runtime::JoinHandle<()>,
    cleanup: Option<Box<dyn FnOnce() + Send>>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.task.abort();
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

// A single outstanding callback prevents an unresponsive UI from accumulating work.
fn polling(
    app: &AppHandle,
    label: &'static str,
    margin: f64,
    state: State,
    sink: Sink,
    native: impl FnMut(Option<bool>, &PointerState) -> bool + Send + 'static,
) -> Subscription {
    let stopped = Arc::new(AtomicBool::new(false));
    let queued = Arc::new(AtomicBool::new(false));
    let native = Arc::new(Mutex::new(native));
    let stop = stopped.clone();
    let app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(80)).await;
            if stop.load(Ordering::Acquire) {
                break;
            }
            if queued.swap(true, Ordering::AcqRel) {
                continue;
            }
            let (a, stop, queued, state, sink, native) =
                (app.clone(), stop.clone(), queued.clone(), state.clone(), sink.clone(), native.clone());
            if app
                .run_on_main_thread(move || {
                    if !stop.load(Ordering::Acquire) {
                        let current = state();
                        if current.shown {
                            if let Some(w) = a.get_webview_window(label) {
                                let inside =
                                    NativeGeometry::pointer(&a).zip(NativeGeometry::frame(&w)).map(|(p, at)| {
                                        let margin = margin * NativeGeometry::unit(&w);
                                        Rect {
                                            x: at.x + margin,
                                            y: at.y + margin,
                                            width: at.width - 2.0 * margin,
                                            height: at.height - 2.0 * margin,
                                        }
                                        .contains(p)
                                    });
                                let outside = native.lock().unwrap()(inside, &current);
                                // No native or application mutex is held while invoking a sink.
                                if current.inside != inside {
                                    sink(PointerEvent::Inside(inside));
                                }
                                if outside {
                                    sink(PointerEvent::OutsideClick);
                                }
                            }
                        }
                    }
                    queued.store(false, Ordering::Release);
                })
                .is_err()
            {
                break;
            }
        }
    });
    Subscription { stopped, task, cleanup: None }
}

// Kept in the interface module so the Windows edge policy is tested on every host.
#[cfg(any(windows, test))]
#[derive(Default)]
struct ButtonEdges([bool; 3]);
#[cfg(any(windows, test))]
impl ButtonEdges {
    fn pressed(&mut self, samples: [u16; 3]) -> bool {
        let mut pressed = false;
        for (was_down, sample) in self.0.iter_mut().zip(samples) {
            let down = sample & 0x8000 != 0;
            pressed |= sample & 1 != 0 || (down && !*was_down);
            *was_down = down;
        }
        pressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn held_buttons_do_not_repeat_and_consecutive_short_click_hints_are_not_lost() {
        let mut buttons = ButtonEdges::default();
        assert!(buttons.pressed([0x8001, 0, 0]));
        assert!(!buttons.pressed([0x8000, 0, 0]));
        assert!(buttons.pressed([0x8000, 0x8000, 0]));
        assert!(!buttons.pressed([0, 0, 0]));
        assert!(buttons.pressed([1, 0, 0]));
        assert!(buttons.pressed([1, 0, 0]));
        assert!(!buttons.pressed([0, 0, 0]));
    }

    #[test]
    fn subscription_drop_cancels_pending_work_and_cleans_up_exactly_once_without_waiting() {
        let stopped = Arc::new(AtomicBool::new(false));
        let cleanups = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        struct Finished(std::sync::mpsc::Sender<()>);
        impl Drop for Finished {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let task = tauri::async_runtime::spawn(async move {
            let _finished = Finished(finished_tx);
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let count = cleanups.clone();
        let subscription = Subscription {
            stopped: stopped.clone(),
            task,
            cleanup: Some(Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            })),
        };
        drop(subscription);
        assert!(stopped.load(Ordering::Acquire));
        assert_eq!(cleanups.load(Ordering::SeqCst), 1);
        // The sleeping/pending task actually exits, independently of a UI event loop.
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }
}
