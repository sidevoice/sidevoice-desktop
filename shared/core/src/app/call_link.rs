//! The call link's commands (`window.__sidevoiceHost.host.callLink`, docs/BRIDGE.md → "The call link"): the call's
//! websocket to the room, which the app opens and keeps for the page (`crate::call_link`). Like the voice's, each command
//! takes the generation of the page that sent it and is refused as `page_stale` when that is not the current page; a page
//! that is replaced leaves its links closed as going away, so the room parks their calls for the next page
//! ([`AppHost::page_changed`]).

use serde_json::Value;

use super::{AppHost, PageGeneration};
use crate::call_link::{LinkEvents, LinkRefusal, LinkRequest};
use crate::voice::host::{refusal, LaneAudio, Refusal};

fn refused(why: LinkRefusal) -> Refusal {
    refusal(why.key, why.message)
}

impl<A: LaneAudio> AppHost<A> {
    /// Opens a link under the page's `key`; its events (`open`, `message`, `error`, `close`) go to `events`.
    pub fn call_link_open(
        &self,
        page: PageGeneration,
        key: String,
        request: LinkRequest,
        events: LinkEvents,
    ) -> Result<(), Refusal> {
        self.current(page)?;
        self.inner.links.open(page.0, key, request, events).map_err(refused)
    }

    /// Sends a text frame on the link, after what was sent before.
    pub fn call_link_send(&self, page: PageGeneration, key: &str, text: String) -> Result<(), Refusal> {
        self.current(page)?;
        self.inner.links.send(page.0, key, text).map_err(refused)
    }

    /// Closes the link as a browser's `close(code, reason)`: without a code, a close frame without one (the room parks the
    /// call); only 1000 hangs up. Codes other than 1000 and 3000–4999 are refused.
    pub fn call_link_close(
        &self,
        page: PageGeneration,
        key: &str,
        code: Option<u16>,
        reason: Option<String>,
    ) -> Result<(), Refusal> {
        self.current(page)?;
        self.inner.links.close(page.0, key, code, reason).map_err(refused)
    }

    /// What the link holds after the page's last index: `{readyState, frames, close}` (`crate::call_link::CallLinks::since`).
    pub fn call_link_since(&self, page: PageGeneration, key: &str, after: u64) -> Result<Value, Refusal> {
        self.current(page)?;
        self.inner.links.since(page.0, key, after).map_err(refused)
    }

    /// The page is done with the link: nothing of it is kept.
    pub fn call_link_forget(&self, page: PageGeneration, key: &str) -> Result<(), Refusal> {
        self.current(page)?;
        self.inner.links.forget(page.0, key).map_err(refused)
    }
}
