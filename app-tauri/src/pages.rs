//! The page's channel: where the call's events go. A page opens a channel when it loads (`voice_events`), and the host's
//! events for that page go over it, in the order the host made them. A channel belongs to one page generation; what is
//! for another generation is not sent, whichever channel is registered, since the page that comes next is the current one
//! from the moment the host says so and an event can still be on its way.
//!
//! A request to open a channel can be late: the page that made it may have been replaced meanwhile, and the request
//! reads the current generation when it is handled, which is the new page's. So a request says which document it is
//! from, and the registry refuses a document older than the one it already holds for the generation.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use sidevoice_app_core::app::{EventSink, PageGeneration};
use sidevoice_app_core::voice::host::{refusal, Refusal};
use tauri::ipc::Channel;

/// Which document a request is from: what the page itself says its start was, `performance.timeOrigin` (milliseconds
/// since the epoch), which is later for a document that replaced another in the same window. It is the page's own word,
/// and the room page is the one caller there is (the caller check); what it settles is only which of two requests that
/// reached the app out of order is the newer document's.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Document(pub f64);

/// A page's generation, the document it registered from and the channel it opened.
type Registered = (PageGeneration, Document, Channel<Value>);

/// The channel of the current page, if it has opened one. It is the app's `EventSink` for the host, and the plugin's
/// state for `voice_events`: the app makes one, hands a clone to each.
#[derive(Clone, Default)]
pub struct Pages(Arc<Mutex<Option<Registered>>>);

impl Pages {
    /// A page opened `channel`, from `document`. Which page it is, `current` says, read once the registry is held, so that
    /// the reading and the registering are one step. Within one generation the newest document keeps the channel: a
    /// document older than the one registered is refused `page_stale` (its request was late, and the document it was
    /// from has been replaced), and the same document registering again replaces its channel. A newer generation takes
    /// the registry whatever the documents say, so that a clock that stepped back cannot shut a page out for good.
    /// Answers the generation it was registered for.
    pub(crate) fn register(
        &self,
        document: Document,
        current: impl FnOnce() -> PageGeneration,
        channel: Channel<Value>,
    ) -> Result<PageGeneration, Refusal> {
        let mut held = self.0.lock().unwrap();
        let page = current();
        if held.as_ref().is_some_and(|(registered, was, _)| *registered == page && document < *was) {
            return Err(refusal("page_stale", "That page has been replaced by a newer one."));
        }
        *held = Some((page, document, channel));
        Ok(page)
    }
}

impl EventSink for Pages {
    /// Over the registered channel when it is `page`'s; nowhere otherwise (no page has opened one yet, or the page it was
    /// for is gone). The lock is held across the send, so events made on different threads reach the page in one order.
    fn event(&self, page: PageGeneration, event: Value) {
        let held = self.0.lock().unwrap();
        if let Some((registered, _, channel)) = held.as_ref() {
            if *registered == page {
                let _ = channel.send(event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tauri::ipc::InvokeResponseBody;

    use super::*;

    const DOC: Document = Document(1_700_000_000_000.5);

    /// A channel and what arrived on it, as JSON, in order.
    fn channel() -> (Channel<Value>, Arc<Mutex<Vec<Value>>>) {
        let arrived = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&arrived);
        let channel = Channel::new(move |body| {
            let InvokeResponseBody::Json(text) = body else { panic!("json") };
            kept.lock().unwrap().push(serde_json::from_str(&text).unwrap());
            Ok(())
        });
        (channel, arrived)
    }

    #[test]
    fn events_arrive_over_the_pages_channel_in_the_order_they_were_made() {
        let pages = Pages::default();
        let (channel, arrived) = channel();
        pages.register(DOC, || PageGeneration(1), channel).unwrap();
        for n in 0..200 {
            pages.event(PageGeneration(1), json!({"key": "a", "type": "level", "data": n}));
        }
        let arrived = arrived.lock().unwrap();
        assert_eq!(arrived.len(), 200);
        for (n, event) in arrived.iter().enumerate() {
            assert_eq!(event["data"], n);
        }
    }

    #[test]
    fn events_made_on_several_threads_keep_each_threads_order() {
        let pages = Pages::default();
        let (channel, arrived) = channel();
        pages.register(DOC, || PageGeneration(1), channel).unwrap();
        let threads: Vec<_> = (0..4)
            .map(|thread| {
                let pages = pages.clone();
                std::thread::spawn(move || {
                    for n in 0..100 {
                        pages.event(PageGeneration(1), json!({"thread": thread, "n": n}));
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let arrived = arrived.lock().unwrap();
        assert_eq!(arrived.len(), 400);
        for thread in 0..4 {
            let mine: Vec<i64> =
                arrived.iter().filter(|e| e["thread"] == thread).map(|e| e["n"].as_i64().unwrap()).collect();
            assert_eq!(mine, (0..100).collect::<Vec<_>>(), "thread {thread}");
        }
    }

    #[test]
    fn nothing_is_sent_for_a_page_that_is_not_the_one_with_the_channel() {
        let pages = Pages::default();
        // No page has opened a channel yet.
        pages.event(PageGeneration(1), json!("lost"));

        let (old, old_arrived) = channel();
        pages.register(DOC, || PageGeneration(1), old).unwrap();
        pages.event(PageGeneration(1), json!("one"));

        // The next page opens its own: the old page's events, still on their way, go nowhere, not to the new channel.
        let (new, new_arrived) = channel();
        pages.register(DOC, || PageGeneration(2), new).unwrap();
        pages.event(PageGeneration(1), json!("late: the old page's stop"));
        pages.event(PageGeneration(2), json!("two"));
        pages.event(PageGeneration(1), json!("later still"));
        // And one for a page that has not registered yet.
        pages.event(PageGeneration(3), json!("early"));

        assert_eq!(*old_arrived.lock().unwrap(), [json!("one")]);
        assert_eq!(*new_arrived.lock().unwrap(), [json!("two")]);
    }

    #[test]
    fn a_page_that_registers_again_replaces_its_channel() {
        let pages = Pages::default();
        let (first, first_arrived) = channel();
        let (second, second_arrived) = channel();
        pages.register(DOC, || PageGeneration(5), first).unwrap();
        pages.register(DOC, || PageGeneration(5), second).unwrap();
        pages.event(PageGeneration(5), json!("x"));
        assert!(first_arrived.lock().unwrap().is_empty());
        assert_eq!(*second_arrived.lock().unwrap(), [json!("x")]);
    }
}

#[cfg(test)]
mod races {
    use std::sync::mpsc;
    use std::time::Duration;

    use serde_json::json;
    use tauri::ipc::InvokeResponseBody;

    use super::*;

    const OLD: Document = Document(1_700_000_000_000.5);
    const NEW: Document = Document(1_700_000_000_090.25);

    fn channel() -> (Channel<Value>, Arc<Mutex<Vec<Value>>>) {
        let arrived = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&arrived);
        let channel = Channel::new(move |body| {
            let InvokeResponseBody::Json(text) = body else { panic!("json") };
            kept.lock().unwrap().push(serde_json::from_str(&text).unwrap());
            Ok(())
        });
        (channel, arrived)
    }

    fn code(refused: Refusal) -> String {
        refused.params["code"].as_str().unwrap().to_owned()
    }

    /// The schedule: the old page's request is held up before it reads anything; the page is reloaded; the new page
    /// registers; then the old request resumes. It reads the new generation, as it would for any page, so only what it
    /// says it is from can tell it from the new page's.
    #[test]
    fn an_old_pages_request_that_resumes_after_the_new_page_registered_does_not_take_its_channel() {
        let pages = Pages::default();
        let current = Arc::new(Mutex::new(PageGeneration(1)));
        let (old, old_arrived) = channel();
        let (new, new_arrived) = channel();
        let (held_up, at_the_gate) = mpsc::channel();
        let (resume, resumed) = mpsc::channel::<()>();

        let late = {
            let (pages, current) = (pages.clone(), Arc::clone(&current));
            std::thread::spawn(move || {
                held_up.send(()).unwrap();
                resumed.recv().unwrap(); // held up before it reads the generation or takes the registry
                pages.register(OLD, move || *current.lock().unwrap(), old)
            })
        };
        at_the_gate.recv().unwrap();

        // The page is reloaded, and the new page registers.
        *current.lock().unwrap() = PageGeneration(2);
        assert_eq!(pages.register(NEW, || *current.lock().unwrap(), new).unwrap(), PageGeneration(2));

        resume.send(()).unwrap();
        assert_eq!(code(late.join().unwrap().unwrap_err()), "page-stale");

        pages.event(PageGeneration(2), json!("for the new page"));
        assert_eq!(*new_arrived.lock().unwrap(), [json!("for the new page")]);
        assert!(old_arrived.lock().unwrap().is_empty(), "the late request registered nothing");
    }

    /// The other order: the late request of the old page gets in before the new page's. Its channel is a dead page's, and
    /// the new page's registration replaces it.
    #[test]
    fn the_new_pages_registration_replaces_a_late_old_ones() {
        let pages = Pages::default();
        let (old, old_arrived) = channel();
        let (new, new_arrived) = channel();
        assert_eq!(pages.register(OLD, || PageGeneration(2), old).unwrap(), PageGeneration(2));
        assert_eq!(pages.register(NEW, || PageGeneration(2), new).unwrap(), PageGeneration(2));
        pages.event(PageGeneration(2), json!("for the new page"));
        assert_eq!(*new_arrived.lock().unwrap(), [json!("for the new page")]);
        assert!(old_arrived.lock().unwrap().is_empty());
    }

    /// A page asks again (its first answer was refused) with the document it has always been.
    #[test]
    fn the_same_document_registering_again_replaces_its_channel() {
        let pages = Pages::default();
        let (first, first_arrived) = channel();
        let (second, second_arrived) = channel();
        pages.register(NEW, || PageGeneration(2), first).unwrap();
        pages.register(NEW, || PageGeneration(2), second).unwrap();
        pages.event(PageGeneration(2), json!("x"));
        assert!(first_arrived.lock().unwrap().is_empty());
        assert_eq!(*second_arrived.lock().unwrap(), [json!("x")]);
    }

    /// A clock that stepped back between two loads gives the newer document the smaller time: it still registers, since
    /// the registry holds the older generation's.
    #[test]
    fn a_newer_generation_registers_whatever_the_clock_says() {
        let pages = Pages::default();
        let (before, _) = channel();
        let (after, after_arrived) = channel();
        pages.register(NEW, || PageGeneration(1), before).unwrap();
        pages.register(OLD, || PageGeneration(2), after).unwrap();
        pages.event(PageGeneration(2), json!("heard"));
        assert_eq!(*after_arrived.lock().unwrap(), [json!("heard")]);
    }

    /// A registration paused after it read the generation holds the registry: the page that changes meanwhile registers
    /// after it, and is the one that stays.
    #[test]
    fn reading_the_generation_and_registering_are_one_step() {
        let pages = Pages::default();
        let current = Arc::new(Mutex::new(PageGeneration(1)));
        let (old, old_arrived) = channel();
        let (new, new_arrived) = channel();
        let (read, has_read) = mpsc::channel();
        let (go, resume) = mpsc::channel::<()>();

        let slow = {
            let (pages, current) = (pages.clone(), Arc::clone(&current));
            std::thread::spawn(move || {
                pages.register(
                    OLD,
                    || {
                        let page = *current.lock().unwrap();
                        read.send(page).unwrap();
                        resume.recv().unwrap();
                        page
                    },
                    old,
                )
            })
        };
        assert_eq!(has_read.recv().unwrap(), PageGeneration(1));
        *current.lock().unwrap() = PageGeneration(2);
        let late = {
            let (pages, current) = (pages.clone(), Arc::clone(&current));
            std::thread::spawn(move || pages.register(NEW, move || *current.lock().unwrap(), new))
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(!late.is_finished(), "the new page's registration waits for the old one's");
        go.send(()).unwrap();
        assert_eq!(slow.join().unwrap().unwrap(), PageGeneration(1));
        assert_eq!(late.join().unwrap().unwrap(), PageGeneration(2));

        pages.event(PageGeneration(2), json!("new"));
        pages.event(PageGeneration(1), json!("old"));
        assert_eq!(*new_arrived.lock().unwrap(), [json!("new")]);
        assert!(old_arrived.lock().unwrap().is_empty());
    }
}
