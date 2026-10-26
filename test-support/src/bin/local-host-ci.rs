// CI's tool for the local host (.github/workflows/build.yml). Never shipped.
//
// - `core <D>`: a stand-in core on the socket it publishes in `D/core/core.json` (the macOS probe runs the real app against it). Prints each
//   request it gets.
// - `serve <D> <app-dir> <port-file>`: as the first user — the stand-in core, and the app's side paired with it
//   (`local-host.json` in `app-dir`, the proxy on loopback); writes the proxy's port (only the port) to `port-file`
//   and serves until killed. `D` is left 0755 so the refusal a second user meets is the core directory's own.
// - `attack <D> <app-dir> <port> <socket>`: as a second user — must fail to open the core's `<socket>` (the endpoint its
//   record names; the other user cannot read the record, so the caller hands it over) (so it can neither pair nor link
//   as a connector), to read the app's pairing, and to use the proxy without the secret (plain requests, the
//   connector link, a WebSocket). Prints one line per attempt; exits 1 if any of them got through.
//
// `serve` and `attack` need the local host, which a host without it answers as unsupported (exit 2).

use sidevoice_desktop_test_support::fake_core::FakeCore;
use sidevoice_desktop_test_support::platform::local_host::{Ci, LocalHostCi};
use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ran = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["core", data] => {
            let core = FakeCore::start(std::path::Path::new(data), 1, true).expect("the stand-in core starts");
            println!("fake-core listening on {}", core.socket().display());
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        ["serve", data, app, port_file] => Ci::serve(data, app, port_file),
        ["attack", data, app, port, socket] => Ci::attack(data, app, port.parse().expect("a port"), socket),
        _ => {
            eprintln!("usage: local-host-ci core <D> | serve <D> <app-dir> <port-file> | attack <D> <app-dir> <port> <socket>");
            exit(2);
        }
    };
    if let Err(error) = ran {
        eprintln!("local-host-ci: {error}");
        exit(2);
    }
}
