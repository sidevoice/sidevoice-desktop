//! The page's way to the local host (design §4.1, "The proxy (N1)"): `127.0.0.1:<ephemeral>` for the app's lifetime,
//! a 32-byte secret made per launch and kept in memory only. The page holds the secret, never the device token; the
//! proxy checks every request against the table below, swaps the secret for the token and carries the request over
//! the core's private socket.
//!
//! | Request | Rule |
//! |---|---|
//! | any | `Host` exactly `127.0.0.1:<port>`, else 421. `Origin` present and an app origin, else 403 (missing, empty, `null`, other). |
//! | `OPTIONS` + `Access-Control-Request-Method` | answered here, never forwarded, no secret: method ∈ GET/POST/PUT/PATCH/DELETE, requested headers ⊆ `authorization, content-type, accept`; 204 with the CORS headers, `Max-Age: 600`, `Access-Control-Allow-Private-Network: true` when asked. Anything else 403. |
//! | other HTTP | `Authorization: Bearer <secret>` (constant time), else 401 with the CORS headers; forwarded with the device token in its place, `Origin` unchanged; the core's answer comes back as it is. |
//! | WebSocket upgrade | the offered subprotocols include `sidevoice.token.<secret>`, else 401; rewritten to the device token; 101 spliced both ways. |
//! | `/api/local*`, `/api/device/local*`, `/api/connectors/link*` | 404: native's own routes, never the page's. |
//!
//! A security header (`Host`, `Origin`, `Authorization`, the preflight's, `Upgrade`, `Sec-WebSocket-Protocol`,
//! `Content-Length`) sent twice, a head that does not parse, an encoded path that could mean two things, or a body
//! sent chunked is refused. No redirect is followed (the core's 3xx goes back as it is) and nothing is cached.
//!
//! **One request per connection.** Each head is read, checked and rewritten on its own, and the proxy closes the
//! connection after the answer (`Connection: close` both ways). The proxy then never has to find where one request or
//! answer ends and the next begins on a reused connection — the place where a second, unchecked request could ride
//! behind a checked one — and it keeps no state between requests. The price is a loopback TCP handshake per request,
//! which browsers handle as a matter of course.
//!
//! Every tunnel (a request in flight, an open WebSocket) closes when the app stops the proxy, when the token is
//! revoked, and when a new pairing replaces it ([`Proxy::set_token`]).

use crate::core_socket::CoreSocket;
use crate::http::{self, Headers, RequestHead};
use crate::identity;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

/// The bundled page's origins (`sidevoice_desktop_core::settings::APP_ORIGIN`, per platform): either is the app.
pub const APP_ORIGINS: [&str; 2] = ["tauri://localhost", "http://tauri.localhost"];
pub const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
pub const REQUEST_HEADERS: [&str; 3] = ["authorization", "content-type", "accept"];
/// Native's own routes on the socket: never forwarded for the page.
pub const NATIVE_ONLY: [&str; 3] = ["/api/local", "/api/device/local", "/api/connectors/link"];
/// How a page names its token on a socket (sidevoice-core `server/devices.py`, `TOKEN_SUBPROTOCOL`).
pub const TOKEN_PROTOCOL: &str = "sidevoice.token.";

/// How long a client has to send its whole head; then how long an authenticated body may pause; how long the core may
/// stay silent mid-answer; how long a refused client's leftover input is read before closing.
const HEAD_DEADLINE: Duration = Duration::from_secs(5);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const LINGER: Duration = Duration::from_secs(1);
const CORE_TIMEOUT: Duration = Duration::from_secs(300);
/// Connections served at once; beyond that a new one is closed at once.
const MAX_CONNECTIONS: usize = 256;
/// Connections still sending their head (none of them authenticated yet). Room for a new one is made only by closing
/// one that is past its head deadline or has sent nothing for [`HEAD_IDLE`]; a head still arriving is never cut short
/// to admit a newcomer, which is refused instead (review R1-c #3).
const MAX_PENDING: usize = 64;
const HEAD_IDLE: Duration = Duration::from_secs(1);

/// What to do with one request head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Answered here: a refusal, or a preflight. `why` is for the log.
    Answer { status: u16, headers: Headers, why: &'static str },
    /// An ordinary request, forwarded with the token; its body is `length` bytes.
    Forward { length: u64 },
    /// A WebSocket upgrade, forwarded with the token in its subprotocol.
    Upgrade,
}

fn refuse(status: u16, why: &'static str) -> Decision {
    Decision::Answer { status, headers: Headers::default(), why }
}

/// The CORS an app origin gets on what the proxy answers itself.
fn cors(origin: &str) -> Headers {
    let mut headers = Headers::default();
    headers.push("access-control-allow-origin", origin);
    headers.push("vary", "Origin");
    headers
}

fn refuse_cors(status: u16, origin: &str, why: &'static str) -> Decision {
    Decision::Answer { status, headers: cors(origin), why }
}

/// Constant time over equal lengths; the secret's length is public (43 characters).
fn same_secret(offered: &str, secret: &str) -> bool {
    offered.len() == secret.len() && bool::from(offered.as_bytes().ct_eq(secret.as_bytes()))
}

/// The ingress table, for one head. `port` is the proxy's own, `secret` this launch's.
pub fn decide(head: &RequestHead, port: u16, secret: &str) -> Decision {
    let headers = &head.headers;
    match headers.one("host") {
        Err(_) => return refuse(400, "two Host headers"),
        Ok(Some(host)) if host == format!("127.0.0.1:{port}") => {}
        Ok(_) => return refuse(421, "wrong Host"),
    }
    let origin = match headers.one("origin") {
        Ok(Some(origin)) if APP_ORIGINS.contains(&origin) => origin,
        Err(_) => return refuse(403, "two Origin headers"),
        Ok(Some("")) => return refuse(403, "empty Origin"),
        Ok(Some("null")) => return refuse(403, "null Origin"),
        Ok(Some(_)) => return refuse(403, "foreign Origin"),
        Ok(None) => return refuse(403, "no Origin"),
    };
    if !head.target.starts_with('/') {
        return refuse(400, "not an origin-form target");
    }

    if head.method == "OPTIONS" {
        return preflight(headers, origin);
    }

    // Framing, for every request that is not a preflight, before choosing between HTTP and a socket (review R1-c
    // #4): one length, never chunked, and no body on an upgrade.
    if headers.has("transfer-encoding") {
        return refuse_cors(411, origin, "chunked request body");
    }
    let length = match headers.one("content-length") {
        Err(_) => return refuse_cors(400, origin, "two Content-Length headers"),
        Ok(None) => 0,
        Ok(Some(value)) => match value.parse::<u64>() {
            Ok(length) if value.bytes().all(|b| b.is_ascii_digit()) => length,
            _ => return refuse_cors(400, origin, "bad Content-Length"),
        },
    };

    let upgrade = match headers.one("upgrade") {
        Err(_) => return refuse(400, "two Upgrade headers"),
        Ok(value) => value.is_some_and(|v| v.eq_ignore_ascii_case("websocket")),
    };
    if upgrade {
        if head.method != "GET" || !headers.lists("connection", "upgrade") {
            return refuse(400, "malformed upgrade");
        }
        if length != 0 {
            return refuse(400, "a body on an upgrade");
        }
        let offered = match headers.one("sec-websocket-protocol") {
            Err(_) => return refuse(400, "two Sec-WebSocket-Protocol headers"),
            Ok(offered) => offered.unwrap_or(""),
        };
        let carries_secret = offered
            .split(',')
            .filter_map(|p| p.trim().strip_prefix(TOKEN_PROTOCOL))
            .fold(false, |found, offered| found | same_secret(offered, secret));
        if !carries_secret {
            return refuse(401, "socket without the secret");
        }
        return match native_only(&head.target) {
            Err(why) => refuse(400, why),
            Ok(true) => refuse(404, "native-only route"),
            Ok(false) => Decision::Upgrade,
        };
    }

    let authorized = match headers.one("authorization") {
        Err(_) => return refuse_cors(401, origin, "two Authorization headers"),
        Ok(value) => value
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .is_some_and(|(_, offered)| same_secret(offered, secret)),
    };
    if !authorized {
        return refuse_cors(401, origin, "no secret");
    }
    match native_only(&head.target) {
        Err(why) => refuse_cors(400, origin, why),
        Ok(true) => refuse_cors(404, origin, "native-only route"),
        Ok(false) => Decision::Forward { length },
    }
}

fn preflight(headers: &Headers, origin: &str) -> Decision {
    match headers.one("access-control-request-method") {
        Ok(Some(method)) if METHODS.contains(&method) => {}
        Ok(Some(_)) => return refuse(403, "preflight for a method not allowed"),
        Ok(None) => return refuse(403, "OPTIONS that is not a preflight"),
        Err(_) => return refuse(403, "two Access-Control-Request-Method headers"),
    }
    match headers.one("access-control-request-headers") {
        Err(_) => return refuse(403, "two Access-Control-Request-Headers headers"),
        Ok(Some(asked)) => {
            let allowed = asked
                .split(',')
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .all(|h| REQUEST_HEADERS.contains(&h.as_str()));
            if !allowed {
                return refuse(403, "preflight for a header not allowed");
            }
        }
        Ok(None) => {}
    }
    let private_network = match headers.one("access-control-request-private-network") {
        Err(_) => return refuse(403, "two Access-Control-Request-Private-Network headers"),
        Ok(value) => value.is_some_and(|v| v.eq_ignore_ascii_case("true")),
    };
    let mut answer = cors(origin);
    answer.push("access-control-allow-methods", METHODS.join(", "));
    answer.push("access-control-allow-headers", REQUEST_HEADERS.join(", "));
    answer.push("access-control-max-age", "600");
    if private_network {
        answer.push("access-control-allow-private-network", "true");
    }
    Decision::Answer { status: 204, headers: answer, why: "preflight" }
}

/// Whether the target's path is one of [`NATIVE_ONLY`], as the core would route it (percent-decoded, any case). A
/// path that decoding makes ambiguous (dot segments, empty segments, backslashes, a bad escape, NUL) is an `Err`.
pub fn native_only(target: &str) -> Result<bool, &'static str> {
    let raw = target.split(['?', '#']).next().unwrap_or("");
    let mut decoded = Vec::with_capacity(raw.len());
    let mut bytes = raw.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next(), bytes.next()];
            let [Some(h), Some(l)] = hex else { return Err("bad percent escape") };
            let value = std::str::from_utf8(&[h, l]).ok().and_then(|s| u8::from_str_radix(s, 16).ok());
            decoded.push(value.ok_or("bad percent escape")?);
        } else {
            decoded.push(b);
        }
    }
    let path = String::from_utf8_lossy(&decoded).to_ascii_lowercase();
    let ambiguous = path.contains('\\')
        || path.contains('\0')
        || path.contains("//")
        || path.split('/').any(|segment| segment == "." || segment == "..");
    if ambiguous {
        return Err("ambiguous path");
    }
    Ok(NATIVE_ONLY.iter().any(|prefix| path.starts_with(prefix)))
}

/// Hop-by-hop fields and what the proxy itself replaces: never carried to the core.
const DROPPED: [&str; 10] = [
    "authorization",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "te",
    "trailer",
    "upgrade",
    "expect",
    "sec-websocket-protocol",
];

/// The head the core gets: the page's, with the device token where the secret was, and `Connection: close` (or the
/// upgrade's own). Everything else — `Origin` included — as the page sent it.
pub fn upstream_head(head: &RequestHead, token: &str, upgrade: bool) -> Vec<u8> {
    let mut headers = head.headers.clone();
    let offered = headers.first("sec-websocket-protocol").unwrap_or("").to_string();
    for name in DROPPED {
        headers.remove(name);
    }
    if upgrade {
        // The page's own protocols stay (`sidevoice`); its secret becomes the token; nothing else claims to be one.
        let mut protocols: Vec<String> = offered
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty() && !p.starts_with(TOKEN_PROTOCOL))
            .map(str::to_string)
            .collect();
        protocols.push(format!("{TOKEN_PROTOCOL}{token}"));
        headers.push("connection", "Upgrade");
        headers.push("upgrade", "websocket");
        headers.push("sec-websocket-protocol", protocols.join(", "));
    } else {
        headers.push("authorization", format!("Bearer {token}"));
        headers.push("connection", "close");
    }
    http::request_head(&head.method, &head.target, &headers)
}

/// A socket of a tunnel, kept to shut it down from outside.
enum Closer {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Closer {
    fn close(&self) {
        let _ = match self {
            Closer::Tcp(s) => s.shutdown(Shutdown::Both),
            Closer::Unix(s) => s.shutdown(Shutdown::Both),
        };
    }
}

#[derive(Default)]
struct Inner {
    /// The device token; `None` while the app is not paired (the proxy then answers 503).
    token: Option<String>,
    tunnels: HashMap<u64, Vec<Closer>>,
}

/// A connection still sending its head.
struct Pending {
    id: u64,
    stream: TcpStream,
    since: Instant,
    /// When its last byte arrived, in ms from [`Shared::epoch`].
    progress: Arc<AtomicU64>,
}

struct Shared {
    inner: Mutex<Inner>,
    /// Connections still sending their head, oldest first.
    pending: Mutex<VecDeque<Pending>>,
    /// What [`Pending::progress`] counts from.
    epoch: Instant,
    core: CoreSocket,
    next: AtomicU64,
    active: AtomicUsize,
    stopped: AtomicBool,
    log: Box<dyn Fn(&str) + Send + Sync>,
}

impl Shared {
    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Closes one connection still sending its head that is past its deadline or idle for [`HEAD_IDLE`]; false when
    /// every one of them is still arriving.
    fn evict_one(&self, pending: &mut VecDeque<Pending>) -> bool {
        let (now, now_ms) = (Instant::now(), self.now_ms());
        let stale = pending.iter().position(|p| {
            now.duration_since(p.since) >= HEAD_DEADLINE
                || now_ms.saturating_sub(p.progress.load(Ordering::SeqCst)) >= HEAD_IDLE.as_millis() as u64
        });
        match stale.and_then(|at| pending.remove(at)) {
            Some(evicted) => {
                let _ = evicted.stream.shutdown(Shutdown::Both);
                true
            }
            None => false,
        }
    }

    /// A new connection among those sending their head: its progress counter, or `None` when there is no room (every
    /// one of them still arriving).
    fn admit(&self, id: u64, stream: &TcpStream) -> Option<Arc<AtomicU64>> {
        let clone = stream.try_clone().ok()?;
        let mut pending = self.pending.lock().unwrap();
        if pending.len() >= MAX_PENDING && !self.evict_one(&mut pending) {
            return None;
        }
        let progress = Arc::new(AtomicU64::new(self.now_ms()));
        pending.push_back(Pending { id, stream: clone, since: Instant::now(), progress: progress.clone() });
        Some(progress)
    }

    fn unpend(&self, id: u64) {
        self.pending.lock().unwrap().retain(|p| p.id != id);
    }

    fn track(&self, id: u64, closer: Closer) {
        self.inner.lock().unwrap().tunnels.entry(id).or_default().push(closer);
    }

    fn close_all(&self, inner: &mut Inner) {
        for (_, closers) in inner.tunnels.drain() {
            closers.iter().for_each(Closer::close);
        }
    }
}

/// The running proxy. Dropping it stops it.
pub struct Proxy {
    port: u16,
    secret: String,
    shared: Arc<Shared>,
}

impl Proxy {
    /// Binds `127.0.0.1:0`, makes this launch's secret and serves until stopped. `log` gets one line per refusal and
    /// per forwarded request (method, path, status; never a secret or a token).
    pub fn start(core: CoreSocket, log: impl Fn(&str) + Send + Sync + 'static) -> io::Result<Proxy> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let secret = identity::random(32);
        let shared = Arc::new(Shared {
            inner: Mutex::default(),
            pending: Mutex::default(),
            epoch: Instant::now(),
            core,
            next: AtomicU64::new(1),
            active: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
            log: Box::new(log),
        });
        let accepting = shared.clone();
        let accept_secret = secret.clone();
        thread::Builder::new().name("local-host-proxy".into()).spawn(move || {
            for stream in listener.incoming() {
                if accepting.stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                // Full: a stale head (past its deadline, or idle) makes room; with none, the new connection is closed.
                let full = accepting.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS;
                if full && !accepting.evict_one(&mut accepting.pending.lock().unwrap()) {
                    accepting.active.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let id = accepting.next.fetch_add(1, Ordering::SeqCst);
                let Some(progress) = accepting.admit(id, &stream) else {
                    (accepting.log)("proxy refused a connection (every head slot still arriving)");
                    accepting.active.fetch_sub(1, Ordering::SeqCst);
                    continue;
                };
                let (shared, secret) = (accepting.clone(), accept_secret.clone());
                let spawned = thread::Builder::new().name("local-host-tunnel".into()).spawn(move || {
                    serve(&shared, stream, id, &progress, port, &secret);
                    shared.active.fetch_sub(1, Ordering::SeqCst);
                });
                if spawned.is_err() {
                    accepting.unpend(id);
                    accepting.active.fetch_sub(1, Ordering::SeqCst);
                }
            }
        })?;
        Ok(Proxy { port, secret, shared })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The page's credential for this launch: in memory only, never written anywhere.
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// `http://127.0.0.1:<port>`: the local host's only URL, as the page knows it.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The device token requests are carried with; `None` stops carrying them. A change closes every tunnel: what
    /// was open was opened with the other token.
    pub fn set_token(&self, token: Option<String>) {
        let mut inner = self.shared.inner.lock().unwrap();
        if inner.token != token {
            inner.token = token;
            self.shared.close_all(&mut inner);
        }
    }

    pub fn close_tunnels(&self) {
        let mut inner = self.shared.inner.lock().unwrap();
        self.shared.close_all(&mut inner);
    }

    /// How many connections are being served now.
    pub fn active(&self) -> usize {
        self.shared.active.load(Ordering::SeqCst)
    }

    /// Stops accepting and closes every tunnel.
    pub fn stop(&self) {
        if self.shared.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.set_token(None);
        self.close_tunnels();
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop();
    }
}

fn answer(client: &mut TcpStream, status: u16, mut headers: Headers) {
    headers.push("content-length", "0");
    headers.push("cache-control", "no-store");
    headers.push("connection", "close");
    let _ = client.write_all(&http::response_head(status, http::reason(status), &headers));
    let _ = client.flush();
}

/// The path without its query, for the log.
fn path_of(target: &str) -> &str {
    target.split('?').next().unwrap_or("")
}

fn serve(shared: &Shared, mut client: TcpStream, id: u64, progress: &AtomicU64, port: u16, secret: &str) {
    let _ = client.set_read_timeout(Some(CLIENT_TIMEOUT));
    let _ = client.set_write_timeout(Some(CORE_TIMEOUT));
    let _ = client.set_nodelay(true);
    if let Ok(clone) = client.try_clone() {
        shared.track(id, Closer::Tcp(clone));
    }
    tunnel(shared, &mut client, id, progress, port, secret);
    shared.unpend(id);
    linger_close(&mut client);
    if let Some(closers) = shared.inner.lock().unwrap().tunnels.remove(&id) {
        closers.iter().for_each(Closer::close);
    }
}

/// Reads with one deadline for the whole of it, not for each read.
struct Deadline<'a> {
    stream: &'a TcpStream,
    until: Instant,
    /// Where to note when a byte last arrived (ms from the epoch given), while a head is read.
    progress: Option<(&'a AtomicU64, Instant)>,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        let mut stream = self.stream;
        let n = stream.read(buf)?;
        if let (Some((progress, epoch)), true) = (self.progress, n > 0) {
            progress.store(epoch.elapsed().as_millis() as u64, Ordering::SeqCst);
        }
        Ok(n)
    }
}

/// Ends our side, then reads what the client still sends (a body behind a refused head) for a moment before closing:
/// closing with unread input resets the connection, and the page would see a network error instead of the answer.
fn linger_close(client: &mut TcpStream) {
    let _ = client.shutdown(Shutdown::Write);
    let mut leftover = Deadline { stream: client, until: Instant::now() + LINGER, progress: None };
    let _ = io::copy(&mut (&mut leftover).take(1024 * 1024), &mut io::sink());
    let _ = client.shutdown(Shutdown::Both);
}

fn tunnel(shared: &Shared, client: &mut TcpStream, id: u64, progress: &AtomicU64, port: u16, secret: &str) {
    let mut buf = Vec::new();
    let until = Instant::now() + HEAD_DEADLINE;
    let mut reading = Deadline { stream: client, until, progress: Some((progress, shared.epoch)) };
    let head_read = http::read_head(&mut reading, &mut buf);
    shared.unpend(id);
    let end = match head_read {
        Ok(end) => end,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            (shared.log)("proxy refused 431 (head too large)");
            return answer(client, 431, Headers::default());
        }
        Err(_) => return,
    };
    // The head is in: from here an authenticated body may pause up to its own timeout.
    let _ = client.set_read_timeout(Some(CLIENT_TIMEOUT));
    let head = match http::parse_request(&buf[..end]) {
        Ok(head) => head,
        Err(why) => {
            (shared.log)(&format!("proxy refused 400 ({why})"));
            return answer(client, 400, Headers::default());
        }
    };
    let path = path_of(&head.target).to_string();
    let decision = decide(&head, port, secret);
    let (upgrade, length) = match decision {
        Decision::Answer { status, headers, why } => {
            (shared.log)(&format!("proxy {} {status} {} {path} ({why})", verb(status), head.method));
            return answer(client, status, headers);
        }
        Decision::Forward { length } => (false, length),
        Decision::Upgrade => (true, 0),
    };
    // A socket's own bytes go to the core only once it has accepted the socket (101): anything a client sent behind its
    // upgrade head, before that, is refused here.
    if upgrade && buf.len() > end {
        (shared.log)(&format!("proxy refused 400 {} {path} (data before the upgrade)", head.method));
        return answer(client, 400, Headers::default());
    }
    let origin = head.headers.first("origin").unwrap_or_default().to_string();

    let upstream = match shared.core.connect(Some(CORE_TIMEOUT)) {
        Ok(upstream) => upstream,
        Err(e) => {
            (shared.log)(&format!("proxy 502 {} {path} (core not reachable: {e:?})", head.method));
            return answer(client, 502, cors(&origin));
        }
    };
    // The token and the tunnel's registration under one lock: a token change after this closes this tunnel too.
    let token = {
        let mut inner = shared.inner.lock().unwrap();
        let token = inner.token.clone();
        if token.is_some() {
            if let Ok(clone) = upstream.try_clone() {
                inner.tunnels.entry(id).or_default().push(Closer::Unix(clone));
            }
        }
        token
    };
    let Some(token) = token else {
        (shared.log)(&format!("proxy 503 {} {path} (not paired)", head.method));
        return answer(client, 503, cors(&origin));
    };

    let mut upstream = upstream;
    let mut out = upstream_head(&head, &token, upgrade);
    let early = &buf[end..];
    let early = &early[..early.len().min(length as usize)];
    out.extend_from_slice(early);
    if upstream.write_all(&out).is_err() {
        return answer(client, 502, cors(&origin));
    }
    if !upgrade {
        let rest = length - early.len() as u64;
        match io::copy(&mut (&*client).take(rest), &mut upstream) {
            Ok(copied) if copied == rest => {}
            _ => return,
        }
    }

    let mut reply = Vec::new();
    let reply_end = match http::read_head(&mut upstream, &mut reply) {
        Ok(end) => end,
        Err(_) => {
            (shared.log)(&format!("proxy 502 {} {path} (no answer from the core)", head.method));
            return answer(client, 502, cors(&origin));
        }
    };
    let Ok(mut response) = http::parse_response(&reply[..reply_end]) else {
        return answer(client, 502, cors(&origin));
    };
    (shared.log)(&format!("proxy forwarded {} {path} -> {}", head.method, response.status));

    if upgrade && response.status == 101 {
        // The core accepted the socket: its answer as it is, then bytes both ways until either side ends.
        if client.write_all(&reply).is_err() {
            return;
        }
        let _ = client.set_read_timeout(None);
        let _ = upstream.set_read_timeout(None);
        let (Ok(mut from_client), Ok(mut to_core)) = (client.try_clone(), upstream.try_clone()) else { return };
        let inbound = thread::spawn(move || {
            let _ = io::copy(&mut from_client, &mut to_core);
            let _ = to_core.shutdown(Shutdown::Both);
            let _ = from_client.shutdown(Shutdown::Both);
        });
        let _ = io::copy(&mut upstream, client);
        let _ = client.shutdown(Shutdown::Both);
        let _ = upstream.shutdown(Shutdown::Both);
        let _ = inbound.join();
        return;
    }

    // An ordinary answer (or a refused upgrade): its head with `Connection: close`, then the rest as it comes.
    for name in ["connection", "keep-alive"] {
        response.headers.remove(name);
    }
    response.headers.push("connection", "close");
    let mut out = http::response_head(response.status, &response.reason, &response.headers);
    out.extend_from_slice(&reply[reply_end..]);
    if client.write_all(&out).is_ok() {
        let _ = io::copy(&mut upstream, client);
    }
}

fn verb(status: u16) -> &'static str {
    if status < 300 {
        "answered"
    } else {
        "refused"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORT: u16 = 41234;
    const SECRET: &str = "s3cr3t-s3cr3t-s3cr3t-s3cr3t-s3cr3t-s3cr3t-x";

    fn head(method: &str, target: &str, fields: &[(&str, &str)]) -> RequestHead {
        let mut text = format!("{method} {target} HTTP/1.1\r\n");
        for (name, value) in fields {
            text.push_str(&format!("{name}: {value}\r\n"));
        }
        text.push_str("\r\n");
        http::parse_request(text.as_bytes()).unwrap()
    }

    const HOST: (&str, &str) = ("Host", "127.0.0.1:41234");
    const ORIGIN: (&str, &str) = ("Origin", "tauri://localhost");

    fn bearer() -> String {
        format!("Bearer {SECRET}")
    }

    fn status(decision: &Decision) -> u16 {
        match decision {
            Decision::Answer { status, .. } => *status,
            Decision::Forward { .. } => 1,
            Decision::Upgrade => 2,
        }
    }

    fn decide_with(method: &str, target: &str, fields: &[(&str, &str)]) -> Decision {
        decide(&head(method, target, fields), PORT, SECRET)
    }

    #[test]
    fn host_must_be_exactly_the_proxy() {
        let auth = bearer();
        for host in ["localhost:41234", "127.0.0.1", "127.0.0.1:41235", "evil.example:41234", "127.0.0.1:41234.", ""] {
            let decision = decide_with("GET", "/api/x", &[("Host", host), ORIGIN, ("Authorization", &auth)]);
            assert_eq!(status(&decision), 421, "{host:?}");
        }
        let decision = decide_with("GET", "/api/x", &[ORIGIN, ("Authorization", &auth)]);
        assert_eq!(status(&decision), 421, "no Host");
        let decision = decide_with("GET", "/api/x", &[HOST, HOST, ORIGIN, ("Authorization", &auth)]);
        assert_eq!(status(&decision), 400, "two Hosts");
        assert_eq!(
            decide_with("GET", "/api/x", &[HOST, ORIGIN, ("Authorization", &auth)]),
            Decision::Forward { length: 0 }
        );
    }

    #[test]
    fn origin_must_be_the_apps_and_present() {
        let auth = bearer();
        for origin in [
            "",
            "null",
            "https://evil.example",
            "tauri://localhost.evil",
            "TAURI://LOCALHOST",
            "http://127.0.0.1:41234",
        ] {
            let decision = decide_with("GET", "/api/x", &[HOST, ("Origin", origin), ("Authorization", &auth)]);
            assert_eq!(status(&decision), 403, "{origin:?}");
            let Decision::Answer { headers, .. } = decision else { unreachable!() };
            assert!(!headers.has("access-control-allow-origin"), "no CORS for {origin:?}");
        }
        assert_eq!(status(&decide_with("GET", "/api/x", &[HOST, ("Authorization", &auth)])), 403, "missing");
        assert_eq!(status(&decide_with("GET", "/api/x", &[HOST, ORIGIN, ORIGIN, ("Authorization", &auth)])), 403);
        for origin in APP_ORIGINS {
            let decision = decide_with("GET", "/api/x", &[HOST, ("Origin", origin), ("Authorization", &auth)]);
            assert_eq!(decision, Decision::Forward { length: 0 }, "{origin}");
        }
        // The Origin rule holds for preflights and sockets too.
        let preflight =
            decide_with("OPTIONS", "/api/x", &[HOST, ("Origin", "null"), ("Access-Control-Request-Method", "GET")]);
        assert_eq!(status(&preflight), 403);
        let protocol = format!("sidevoice, {TOKEN_PROTOCOL}{SECRET}");
        let socket = ws(&[HOST, ("Origin", "https://evil.example"), ("Sec-WebSocket-Protocol", &protocol)]);
        assert_eq!(status(&socket), 403);
    }

    #[test]
    fn a_valid_preflight_is_answered_here_without_the_secret() {
        let decision = decide_with(
            "OPTIONS",
            "/api/presentation/echo",
            &[
                HOST,
                ORIGIN,
                ("Access-Control-Request-Method", "POST"),
                ("Access-Control-Request-Headers", "Authorization, content-type,accept"),
                ("Access-Control-Request-Private-Network", "true"),
            ],
        );
        let Decision::Answer { status: 204, headers, .. } = decision else { panic!("{decision:?}") };
        assert_eq!(headers.first("access-control-allow-origin"), Some("tauri://localhost"));
        assert_eq!(headers.first("vary"), Some("Origin"));
        assert_eq!(headers.first("access-control-allow-methods"), Some("GET, POST, PUT, PATCH, DELETE"));
        assert_eq!(headers.first("access-control-allow-headers"), Some("authorization, content-type, accept"));
        assert_eq!(headers.first("access-control-max-age"), Some("600"));
        assert_eq!(headers.first("access-control-allow-private-network"), Some("true"));
        // Not asked, not granted; a preflight to a native-only path is still only a preflight.
        let plain =
            decide_with("OPTIONS", "/api/local/health", &[HOST, ORIGIN, ("Access-Control-Request-Method", "GET")]);
        let Decision::Answer { status: 204, headers, .. } = plain else { panic!("{plain:?}") };
        assert!(!headers.has("access-control-allow-private-network"));
    }

    #[test]
    fn a_preflight_outside_the_allowlists_is_403() {
        for (method, asked) in [
            ("CONNECT", None),
            ("TRACE", None),
            ("OPTIONS", None),
            ("get", None),
            ("GET", Some("x-custom")),
            ("POST", Some("authorization, x-sidevoice-token")),
            ("POST", Some("cookie")),
        ] {
            let mut fields = vec![HOST, ORIGIN, ("Access-Control-Request-Method", method)];
            if let Some(asked) = asked {
                fields.push(("Access-Control-Request-Headers", asked));
            }
            assert_eq!(status(&decide_with("OPTIONS", "/api/x", &fields)), 403, "{method} {asked:?}");
        }
        assert_eq!(status(&decide_with("OPTIONS", "/api/x", &[HOST, ORIGIN])), 403, "OPTIONS that is no preflight");
        let twice = [HOST, ORIGIN, ("Access-Control-Request-Method", "GET"), ("Access-Control-Request-Method", "GET")];
        assert_eq!(status(&decide_with("OPTIONS", "/api/x", &twice)), 403);
    }

    #[test]
    fn other_requests_need_the_secret_and_get_cors_on_401() {
        let wrong = format!("Bearer {}", SECRET.replace('x', "y"));
        let short = "Bearer s3cr3t".to_string();
        let token_like = "Bearer the-device-token".to_string();
        let basic = format!("Basic {SECRET}");
        for auth in [
            None,
            Some(wrong.as_str()),
            Some(short.as_str()),
            Some(token_like.as_str()),
            Some(basic.as_str()),
            Some(""),
            Some("Bearer"),
        ] {
            let mut fields = vec![HOST, ORIGIN];
            if let Some(auth) = auth {
                fields.push(("Authorization", auth));
            }
            let decision = decide_with("POST", "/api/x", &fields);
            let Decision::Answer { status: 401, headers, .. } = decision else { panic!("{auth:?}: {decision:?}") };
            assert_eq!(headers.first("access-control-allow-origin"), Some("tauri://localhost"));
            assert_eq!(headers.first("vary"), Some("Origin"));
        }
        let auth = bearer();
        assert_eq!(
            status(&decide_with("GET", "/api/x", &[HOST, ORIGIN, ("Authorization", &auth), ("Authorization", &auth)])),
            401
        );
        let lower = format!("bearer {SECRET}");
        assert_eq!(
            decide_with("GET", "/api/x", &[HOST, ORIGIN, ("Authorization", &lower)]),
            Decision::Forward { length: 0 }
        );
    }

    #[test]
    fn bodies_are_framed_by_one_length() {
        let auth = bearer();
        let base = [HOST, ORIGIN, ("Authorization", auth.as_str())];
        let with = |extra: &[(&'static str, &'static str)]| {
            let mut fields = base.to_vec();
            fields.extend_from_slice(extra);
            decide_with("POST", "/api/x", &fields)
        };
        assert_eq!(with(&[("Content-Length", "12")]), Decision::Forward { length: 12 });
        assert_eq!(status(&with(&[("Content-Length", "12"), ("Content-Length", "12")])), 400);
        assert_eq!(status(&with(&[("Content-Length", "-1")])), 400);
        assert_eq!(status(&with(&[("Content-Length", "+5")])), 400);
        assert_eq!(status(&with(&[("Transfer-Encoding", "chunked")])), 411);
    }

    #[test]
    fn native_only_routes_are_404_however_spelled() {
        let auth = bearer();
        for target in [
            "/api/local/health",
            "/api/local",
            "/api/device/local/pair",
            "/api/device/local",
            "/api/connectors/link/?EIO=4&transport=websocket",
            "/API/Local/health",
            "/api/%6cocal/health",
            "/api/device/%6Cocal",
            "/api/connectors%2flink",
        ] {
            let decision = decide_with("POST", target, &[HOST, ORIGIN, ("Authorization", &auth)]);
            assert_eq!(status(&decision), 404, "{target}");
        }
        for target in [
            "/api//local/health",
            "/api/x/../local/health",
            "/api/./local",
            "/api\\local",
            "/api/%2e%2e/x",
            "/a%zz",
            "/a%2",
        ] {
            let decision = decide_with("GET", target, &[HOST, ORIGIN, ("Authorization", &auth)]);
            assert_eq!(status(&decision), 400, "{target}");
        }
        for target in ["/api/device/devices", "/api/presentation/echo?x=/api/local", "/api/devices/local"] {
            let decision = decide_with("GET", target, &[HOST, ORIGIN, ("Authorization", &auth)]);
            assert_eq!(decision, Decision::Forward { length: 0 }, "{target}");
        }
        assert_eq!(
            status(&decide_with("GET", "http://127.0.0.1:41234/api/x", &[HOST, ORIGIN, ("Authorization", &auth)])),
            400
        );
    }

    fn ws(fields: &[(&str, &str)]) -> Decision {
        let mut all = vec![
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade"),
            ("Sec-WebSocket-Key", "k"),
            ("Sec-WebSocket-Version", "13"),
        ];
        all.extend_from_slice(fields);
        decide_with("GET", "/api/call", &all)
    }

    #[test]
    fn a_socket_needs_the_secret_among_its_protocols() {
        let good = format!("sidevoice, {TOKEN_PROTOCOL}{SECRET}");
        assert_eq!(ws(&[HOST, ORIGIN, ("Sec-WebSocket-Protocol", &good)]), Decision::Upgrade);
        let wrong = format!("sidevoice, {TOKEN_PROTOCOL}nope");
        for offered in [Some("sidevoice"), Some(wrong.as_str()), None] {
            let mut fields = vec![HOST, ORIGIN];
            if let Some(offered) = offered {
                fields.push(("Sec-WebSocket-Protocol", offered));
            }
            assert_eq!(status(&ws(&fields)), 401, "{offered:?}");
        }
        assert_eq!(
            status(&ws(&[HOST, ORIGIN, ("Sec-WebSocket-Protocol", &good), ("Sec-WebSocket-Protocol", &good)])),
            400
        );
        assert_eq!(status(&ws(&[HOST, ("Sec-WebSocket-Protocol", &good)])), 403, "no Origin");
        assert_eq!(status(&ws(&[("Host", "localhost:41234"), ORIGIN, ("Sec-WebSocket-Protocol", &good)])), 421);
        // The connector link is a socket too: never through the proxy.
        let link = decide(
            &head(
                "GET",
                "/api/connectors/link/?EIO=4&transport=websocket",
                &[HOST, ORIGIN, ("Upgrade", "websocket"), ("Connection", "Upgrade"), ("Sec-WebSocket-Protocol", &good)],
            ),
            PORT,
            SECRET,
        );
        assert_eq!(status(&link), 404);
    }

    #[test]
    fn an_upgrade_is_framed_like_any_request() {
        let good = format!("sidevoice, {TOKEN_PROTOCOL}{SECRET}");
        let socket = [HOST, ORIGIN, ("Sec-WebSocket-Protocol", good.as_str())];
        let with = |extra: &[(&str, &str)]| {
            let mut fields = socket.to_vec();
            fields.extend_from_slice(extra);
            ws(&fields)
        };
        assert_eq!(status(&with(&[("Transfer-Encoding", "chunked")])), 411);
        assert_eq!(status(&with(&[("Content-Length", "5")])), 400, "a body on an upgrade");
        assert_eq!(status(&with(&[("Content-Length", "0"), ("Content-Length", "0")])), 400);
        assert_eq!(status(&with(&[("Content-Length", "x")])), 400);
        assert_eq!(with(&[("Content-Length", "0")]), Decision::Upgrade);
        assert_eq!(with(&[]), Decision::Upgrade);
    }

    #[test]
    fn the_core_gets_the_token_and_never_the_secret() {
        let auth = bearer();
        let request = head(
            "POST",
            "/api/x?a=1",
            &[HOST, ORIGIN, ("Authorization", &auth), ("Connection", "keep-alive"), ("Content-Length", "2")],
        );
        let sent = String::from_utf8(upstream_head(&request, "TOKEN", false)).unwrap();
        assert!(sent.starts_with("POST /api/x?a=1 HTTP/1.1\r\n"), "{sent}");
        assert!(sent.contains("origin: tauri://localhost\r\n"), "Origin unchanged: {sent}");
        assert!(sent.contains("host: 127.0.0.1:41234\r\n"));
        assert!(sent.contains("authorization: Bearer TOKEN\r\n"));
        assert!(sent.contains("connection: close\r\n") && !sent.contains("keep-alive"));
        assert!(!sent.contains(SECRET));

        let offered = format!("sidevoice, {TOKEN_PROTOCOL}{SECRET}, {TOKEN_PROTOCOL}other");
        let socket = head(
            "GET",
            "/api/call",
            &[
                HOST,
                ORIGIN,
                ("Upgrade", "websocket"),
                ("Connection", "Upgrade"),
                ("Sec-WebSocket-Protocol", &offered),
                ("Authorization", "x"),
            ],
        );
        let sent = String::from_utf8(upstream_head(&socket, "TOKEN", true)).unwrap();
        assert!(sent.contains("sec-websocket-protocol: sidevoice, sidevoice.token.TOKEN\r\n"), "{sent}");
        assert!(sent.contains("connection: Upgrade\r\n") && sent.contains("upgrade: websocket\r\n"));
        assert!(!sent.contains(SECRET) && !sent.contains("authorization"), "{sent}");
    }
}
