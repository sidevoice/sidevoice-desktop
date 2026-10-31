//! The page's channel: where the call's events go. A page opens a channel when it loads (`voice_events`), and the host's
//! events for that page go over it, in the order the host made them. A channel belongs to one page generation; what is
//! for another generation is not sent, whichever channel is registered, since the page that comes next is the current one
//! from the moment the host says so and an event can still be on its way.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use sidevoice_app_core::app::{EventSink, PageGeneration};
use tauri::ipc::Channel;

/// A page's generation and the channel it opened.
type Registered = (PageGeneration, Channel<Value>);

/// The channel of the current page, if it has opened one. It is the app's `EventSink` for the host, and the plugin's
/// state for `voice_events`: the app makes one, hands a clone to each.
#[derive(Clone, Default)]
pub struct Pages(Arc<Mutex<Option<Registered>>>);

impl Pages {
    /// The page of generation `page` opened `channel`: it replaces the channel of any page before it, or its own.
    pub(crate) fn register(&self, page: PageGeneration, channel: Channel<Value>) {
        *self.0.lock().unwrap() = Some((page, channel));
    }
}

impl EventSink for Pages {
    /// Over the registered channel when it is `page`'s; nowhere otherwise (no page has opened one yet, or the page it was
    /// for is gone). The lock is held across the send, so events made on different threads reach the page in one order.
    fn event(&self, page: PageGeneration, event: Value) {
        let held = self.0.lock().unwrap();
        if let Some((registered, channel)) = held.as_ref() {
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
        pages.register(PageGeneration(1), channel);
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
        pages.register(PageGeneration(1), channel);
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
        pages.register(PageGeneration(1), old);
        pages.event(PageGeneration(1), json!("one"));

        // The next page opens its own: the old page's events, still on their way, go nowhere, not to the new channel.
        let (new, new_arrived) = channel();
        pages.register(PageGeneration(2), new);
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
        pages.register(PageGeneration(5), first);
        pages.register(PageGeneration(5), second);
        pages.event(PageGeneration(5), json!("x"));
        assert!(first_arrived.lock().unwrap().is_empty());
        assert_eq!(*second_arrived.lock().unwrap(), [json!("x")]);
    }
}
