//! The runtime the local host drives `sidevoice_platform`'s async channel and child execution on. The local host
//! blocks (its callers are the app's blocking threads and its own), so each call waits here for its future; one
//! worker thread runs the IO and timers for all of them.

use std::future::Future;
use std::sync::OnceLock;
use tokio::runtime::{Builder, Runtime};

/// Runs `future` to completion on the calling thread, which blocks until then. Never called from async code: the local
/// host has none, and the app reaches it from blocking threads only.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("sidevoice-local-host")
                .enable_all()
                .build()
                .expect("the local host's runtime starts")
        })
        .block_on(future)
}
