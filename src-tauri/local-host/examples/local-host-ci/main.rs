//! CI's tool for the local host (`cargo xtask isolation` and `smoke`). Feature `fake-core`; never shipped.
//!
//! - `core <D>`: a stand-in core on `D/core/local.sock` (the macOS probe runs the real app against it). Prints each
//!   request it gets.
//! - `serve <D> <app-dir> <port-file>`: as the first user — the stand-in core, and the app's side paired with it
//!   (`local-host.json` in `app-dir`, the proxy on loopback); writes the proxy's port (only the port) to `port-file`
//!   and serves until killed. `D` is left 0755 so the refusal a second user meets is the core directory's own.
//! - `attack <D> <app-dir> <port>`: as a second user — must fail to open `local.sock` (so it can neither pair nor link
//!   as a connector), to read the app's pairing, and to use the proxy without the secret (plain requests, the
//!   connector link, a WebSocket). Prints one line per attempt; exits 1 if any of them got through.
//!
//! The local host is Unix only (src/lib.rs); elsewhere this tool only says so.

#[cfg(unix)]
mod unix;

fn main() {
    #[cfg(unix)]
    unix::main();
    #[cfg(not(unix))]
    {
        eprintln!("local-host-ci: the local host is Unix only");
        std::process::exit(2);
    }
}
