//! Owned pointer observation. Workers only schedule bounded main-thread work; they never query windows or wait for UI.
use super::desktop_geometry;
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

/// What an OS does differently while the pointer is watched; everything else is [`watch`]. Each runs with no
/// application mutex held.
trait Os {
    /// Whether the pointer can be observed globally at all. Where it cannot (Wayland) the page's own events are used.
    fn available() -> bool {
        true
    }

    /// One-time setup before the first look at the pointer.
    fn prepare() {}

    /// Every mouse button as the OS reports it, where outside clicks are seen by polling; `None` where they are not.
    fn button_samples() -> Option<[ButtonSample; 3]> {
        None
    }

    /// The native cursor upkeep at each look at the pointer, only where the app must set the cursor itself.
    fn cursor_upkeep(_inside: Option<bool>, _page_wants_hand: bool, _was_inside: Option<bool>) {}

    /// Outside clicks delivered by the OS instead of polled: reported to `sink` until `stopped`. Returns what removes
    /// the native monitor, `None` where the OS offers none.
    fn click_monitor(_app: &AppHandle, _stopped: Arc<AtomicBool>, _sink: Sink) -> Option<Cleanup> {
        None
    }
}

type Cleanup = Box<dyn FnOnce() + Send>;

/// Owned cancellation and native cleanup. Dropping never joins a worker or waits for the main thread.
/// A callback already executing may finish; queued callbacks check cancellation before accessing the app.
pub struct Subscription {
    stopped: Arc<AtomicBool>,
    task: tauri::async_runtime::JoinHandle<()>,
    cleanup: Option<Cleanup>,
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

/// One mouse button as the OS reports it between two looks: `(down now, pressed since the last look)`. The second is
/// only a hint: another process may consume it, and fast clicks between looks are not guaranteed to show.
type ButtonSample = (bool, bool);

// A single outstanding callback prevents an unresponsive UI from accumulating work. The worker checks that the card
// is shown before asking the main thread for anything, so a hidden card costs the UI no wakeups.

/// Install during main-thread setup. State is read without holding native locks; callbacks run on the main thread.
/// `None` means global observation is unavailable (Wayland uses the page's own events).
/// Drop the subscription outside application locks to cancel queued callbacks and remove native monitors.
pub fn watch(app: &AppHandle, label: &'static str, margin: f64, state: State, sink: Sink) -> Option<Subscription> {
    if !imp::Native::available() {
        return None;
    }
    imp::Native::prepare();
    let stopped = Arc::new(AtomicBool::new(false));
    let queued = Arc::new(AtomicBool::new(false));
    let edges = Arc::new(Mutex::new(ButtonEdges::default()));
    let cleanup = imp::Native::click_monitor(app, stopped.clone(), sink.clone());
    let stop = stopped.clone();
    let app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(80)).await;
            if stop.load(Ordering::Acquire) {
                break;
            }
            if !state().shown {
                continue;
            }
            if queued.swap(true, Ordering::AcqRel) {
                continue;
            }
            let (a, stop, queued, state, sink, edges) =
                (app.clone(), stop.clone(), queued.clone(), state.clone(), sink.clone(), edges.clone());
            if app
                .run_on_main_thread(move || {
                    if !stop.load(Ordering::Acquire) {
                        let current = state();
                        if current.shown {
                            if let Some(w) = a.get_webview_window(label) {
                                let inside =
                                    desktop_geometry::pointer(&a).zip(desktop_geometry::frame(&w)).map(|(p, at)| {
                                        let margin = margin * desktop_geometry::unit(&w);
                                        Rect {
                                            x: at.x + margin,
                                            y: at.y + margin,
                                            width: at.width - 2.0 * margin,
                                            height: at.height - 2.0 * margin,
                                        }
                                        .contains(p)
                                    });
                                imp::Native::cursor_upkeep(inside, current.hand, current.inside);
                                let outside = imp::Native::button_samples()
                                    .is_some_and(|samples| edges.lock().unwrap().pressed(samples))
                                    && inside == Some(false);
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
    Some(Subscription { stopped, task, cleanup })
}

// The edge policy is shared so every host tests it: native files only report what the buttons are doing.
#[derive(Default)]
struct ButtonEdges([bool; 3]);
impl ButtonEdges {
    fn pressed(&mut self, samples: [ButtonSample; 3]) -> bool {
        let mut pressed = false;
        for (was_down, (down, hint)) in self.0.iter_mut().zip(samples) {
            pressed |= hint || (down && !*was_down);
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
        const UP: ButtonSample = (false, false);
        const HELD: ButtonSample = (true, false);
        assert!(buttons.pressed([(true, true), UP, UP]));
        assert!(!buttons.pressed([HELD, UP, UP]));
        assert!(buttons.pressed([HELD, HELD, UP]));
        assert!(!buttons.pressed([UP, UP, UP]));
        assert!(buttons.pressed([(false, true), UP, UP]));
        assert!(buttons.pressed([(false, true), UP, UP]));
        assert!(!buttons.pressed([UP, UP, UP]));
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
