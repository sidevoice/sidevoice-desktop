//! The call's link to the room, held by the app (docs/BRIDGE.md → "The call link"): the websocket a call speaks the
//! core's call protocol over, opened, kept and read here instead of in the page. A phone freezes the page's script when
//! its screen locks or another app is in front, while the app (its audio) keeps running; a link held by the page then
//! stops answering the room's keepalive and the room parks the call (briefs: native call link, N1–N5). Held here, the
//! link answers the keepalive itself and keeps what the room sends until the page reads it.
//!
//! The link is a socket and nothing more: the page says what to send (its hello, its turns, its reports) and reads
//! every frame the room sends, in order, as it would read its own websocket. What the link does by itself is what must
//! go on while the page does not run:
//!
//! - **The keepalive.** The room's `voice-ping` is answered here with `voice-pong`, and the page never sees it.
//! - **What the room sends.** Every other text frame is numbered (`index`, from 1) and kept, the last [`KEPT_FRAMES`]
//!   of them within [`KEPT_BYTES`], and handed to the link's events as it comes. A page that ran again asks for what
//!   came after the last index it read ([`CallLinks::since`]): what was handed to it while it slept may not have
//!   reached it.
//! - **The end.** How the socket closed (`code`, `reason`, whether the close was clean) is kept with the frames until
//!   the page forgets the link.
//!
//! A link belongs to the page that opened it (its owner, the page's number): once another page is the current one,
//! the old page's links are closed as going away (1001), never as a hang-up (1000), so the room parks the call and
//! the new page may take it back with its resume ticket.
//!
//! No shell: the events go to a function the shell gives (`LinkEvents`), and the runtime is the one the app passes in.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// How many of the room's frames a link keeps for a page that comes back: the room keeps as many for a call that resumes.
pub const KEPT_FRAMES: usize = 512;
/// The most a link keeps of them, in bytes of text.
pub const KEPT_BYTES: usize = 16 * 1024 * 1024;
/// How long a link waits for the room's answer to its close before it lets the socket go.
const CLOSE_PATIENCE: Duration = Duration::from_secs(5);
/// How long a link waits for the room to answer at all: past it, the link is closed as a socket that never opened.
const CONNECT_PATIENCE: Duration = Duration::from_secs(30);
/// The close a page that goes away leaves its links with: not a hang-up, so the room parks the call.
const GOING_AWAY: u16 = 1001;
/// What a socket that ended without a close frame is said to have closed with, as a browser says it.
const ABNORMAL: u16 = 1006;
/// What a close frame without a code is said to have closed with.
const NO_STATUS: u16 = 1005;
/// The most closed links a page keeps unforgotten: a page that never forgets them does not grow the app.
const CLOSED_KEPT: usize = 8;

/// Where a link's events go, as JSON: `{type: "open"}`, `{type: "message", index, data}`, `{type: "error"}` and
/// `{type: "close", code, reason, wasClean}`, in the order they happened.
pub type LinkEvents = Arc<dyn Fn(Value) + Send + Sync>;

/// What the page asks a link to open: the socket's address and its subprotocols, as a page would open a websocket.
#[derive(Debug, Clone)]
pub struct LinkRequest {
    /// `ws://` or `wss://`.
    pub url: String,
    /// The subprotocols the socket offers (the core's `sidevoice` and the device's token, `sidevoice.token.…`).
    pub protocols: Vec<String>,
    /// The page's language, as `Accept-Language`: the room words its close reasons in it.
    pub language: Option<String>,
}

/// Why a link command was refused: a key the page reads, and an English sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRefusal {
    pub key: &'static str,
    pub message: &'static str,
}

const UNKNOWN: LinkRefusal =
    LinkRefusal { key: "call_link_unknown", message: "No call link of this page has that key." };
const TAKEN: LinkRefusal =
    LinkRefusal { key: "call_link_taken", message: "A call link with that key is open already." };
const BAD_URL: LinkRefusal =
    LinkRefusal { key: "call_link_url", message: "A call link opens a ws:// or wss:// address." };
const NOT_OPEN: LinkRefusal = LinkRefusal { key: "call_link_not_open", message: "The call link is not open." };

/// `readyState`, as a browser's websocket numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ready {
    Connecting = 0,
    Open = 1,
    Closing = 2,
    Closed = 3,
}

/// How a link ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub code: u16,
    pub reason: String,
    pub clean: bool,
}

impl Ended {
    fn json(&self) -> Value {
        json!({"type": "close", "code": self.code, "reason": self.reason, "wasClean": self.clean})
    }
}

/// What a link keeps for its page: its state, the room's frames and how it ended.
struct Kept {
    ready: Ready,
    frames: VecDeque<(u64, String)>,
    bytes: usize,
    last: u64,
    ended: Option<Ended>,
}

impl Kept {
    fn new() -> Self {
        Kept { ready: Ready::Connecting, frames: VecDeque::new(), bytes: 0, last: 0, ended: None }
    }

    /// Numbers and keeps a frame of the room's, letting the oldest go past the limits. Answers its index.
    fn keep(&mut self, text: String) -> u64 {
        self.last += 1;
        self.bytes += text.len();
        self.frames.push_back((self.last, text));
        while self.frames.len() > KEPT_FRAMES || (self.bytes > KEPT_BYTES && self.frames.len() > 1) {
            if let Some((_, old)) = self.frames.pop_front() {
                self.bytes -= old.len();
            }
        }
        self.last
    }
}

/// What a command asks of a link's task.
enum Ask {
    Send(String),
    Close(u16, String),
}

/// A link as the registry holds it.
struct Link {
    owner: u64,
    kept: Arc<Mutex<Kept>>,
    asks: mpsc::UnboundedSender<Ask>,
    /// The order it was opened in, for letting the oldest closed links go.
    opened: u64,
}

/// The app's call links: one per call socket a page opened, by the page's own key.
pub struct CallLinks {
    runtime: Handle,
    links: Mutex<HashMap<String, Link>>,
    opened: Mutex<u64>,
}

impl CallLinks {
    /// The links of an app, run on `runtime`.
    pub fn new(runtime: Handle) -> Self {
        CallLinks { runtime, links: Mutex::default(), opened: Mutex::new(0) }
    }

    /// Opens a link for page `owner` under `key`, its events to `events`. It answers at once: whether the socket opens
    /// is told by its events (`open`, or `error` then `close`), as a browser's websocket tells it.
    pub fn open(&self, owner: u64, key: String, request: LinkRequest, events: LinkEvents) -> Result<(), LinkRefusal> {
        let secure = match request.url.split_once("://").map(|(scheme, _)| scheme.to_ascii_lowercase()) {
            Some(scheme) if scheme == "wss" => true,
            Some(scheme) if scheme == "ws" => false,
            _ => return Err(BAD_URL),
        };
        let mut links = self.links.lock().unwrap();
        if links.contains_key(&key) {
            return Err(TAKEN);
        }
        let opened = {
            let mut count = self.opened.lock().unwrap();
            *count += 1;
            *count
        };
        let kept = Arc::new(Mutex::new(Kept::new()));
        let (asks, asked) = mpsc::unbounded_channel();
        links.insert(key, Link { owner, kept: Arc::clone(&kept), asks, opened });
        Self::forget_oldest_closed(&mut links, owner);
        self.runtime.spawn(run(request, secure, kept, asked, events));
        Ok(())
    }

    /// Sends `text` on the link, after whatever was asked of it before. Refused before the link opened (a browser throws
    /// then); once it is closing or closed the text is let go, as a browser lets it go.
    pub fn send(&self, owner: u64, key: &str, text: String) -> Result<(), LinkRefusal> {
        let links = self.links.lock().unwrap();
        let link = Self::owned(&links, owner, key)?;
        match link.kept.lock().unwrap().ready {
            Ready::Connecting => return Err(NOT_OPEN),
            Ready::Open => {}
            Ready::Closing | Ready::Closed => return Ok(()),
        }
        let _ = link.asks.send(Ask::Send(text));
        Ok(())
    }

    /// Closes the link with `code` (1000, a hang-up, by default) and `reason`. Its `close` event comes once the room
    /// answered, or once it was given up on.
    pub fn close(&self, owner: u64, key: &str, code: Option<u16>, reason: Option<String>) -> Result<(), LinkRefusal> {
        let links = self.links.lock().unwrap();
        let link = Self::owned(&links, owner, key)?;
        Self::ask_close(link, code.unwrap_or(1000), reason.unwrap_or_default());
        Ok(())
    }

    /// What the link holds after the page's last index `after`: `{readyState, frames: [{index, data}], close}`, `close`
    /// how it ended (`{code, reason, wasClean}`) or `null` while it has not. What it let go of is not in `frames`.
    pub fn since(&self, owner: u64, key: &str, after: u64) -> Result<Value, LinkRefusal> {
        let links = self.links.lock().unwrap();
        let link = Self::owned(&links, owner, key)?;
        let kept = link.kept.lock().unwrap();
        let frames: Vec<Value> = kept
            .frames
            .iter()
            .filter(|(index, _)| *index > after)
            .map(|(index, data)| json!({"index": index, "data": data}))
            .collect();
        let close = kept.ended.as_ref().map(|ended| {
            let mut close = ended.json();
            close.as_object_mut().map(|object| object.remove("type"));
            close
        });
        Ok(json!({"readyState": kept.ready as u8, "frames": frames, "close": close}))
    }

    /// The page is done with the link: it is closed (1000) if it is still open, and nothing of it is kept.
    pub fn forget(&self, owner: u64, key: &str) -> Result<(), LinkRefusal> {
        let mut links = self.links.lock().unwrap();
        Self::owned(&links, owner, key)?;
        if let Some(link) = links.remove(key) {
            Self::ask_close(&link, 1000, String::new());
        }
        Ok(())
    }

    /// Page `current` is the current one: the links of every page before it are closed as going away (the room parks
    /// their calls, for the new page to take back), and forgotten.
    pub fn close_older_than(&self, current: u64) {
        let mut links = self.links.lock().unwrap();
        links.retain(|_, link| {
            if link.owner >= current {
                return true;
            }
            Self::ask_close(link, GOING_AWAY, String::new());
            false
        });
    }

    /// The app quits: every link is closed as going away.
    pub fn close_all(&self) {
        let mut links = self.links.lock().unwrap();
        for (_, link) in links.drain() {
            Self::ask_close(&link, GOING_AWAY, String::new());
        }
    }

    /// How many links are held, closed ones the page has not forgotten included.
    pub fn len(&self) -> usize {
        self.links.lock().unwrap().len()
    }

    /// Whether no link is held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn owned<'a>(links: &'a HashMap<String, Link>, owner: u64, key: &str) -> Result<&'a Link, LinkRefusal> {
        links.get(key).filter(|link| link.owner == owner).ok_or(UNKNOWN)
    }

    fn ask_close(link: &Link, code: u16, reason: String) {
        let mut kept = link.kept.lock().unwrap();
        if matches!(kept.ready, Ready::Connecting | Ready::Open) {
            kept.ready = Ready::Closing;
            let _ = link.asks.send(Ask::Close(code, reason));
        }
    }

    /// Lets the oldest closed links of `owner` go past [`CLOSED_KEPT`].
    fn forget_oldest_closed(links: &mut HashMap<String, Link>, owner: u64) {
        let mut closed: Vec<(u64, String)> = links
            .iter()
            .filter(|(_, link)| link.owner == owner && link.kept.lock().unwrap().ready == Ready::Closed)
            .map(|(key, link)| (link.opened, key.clone()))
            .collect();
        if closed.len() <= CLOSED_KEPT {
            return;
        }
        closed.sort();
        for (_, key) in closed.iter().take(closed.len() - CLOSED_KEPT) {
            links.remove(key);
        }
    }
}

/// A link's life: the socket opened, then the room's frames and the page's asks, until it closes.
async fn run(
    request: LinkRequest,
    secure: bool,
    kept: Arc<Mutex<Kept>>,
    mut asked: mpsc::UnboundedReceiver<Ask>,
    events: LinkEvents,
) {
    let connecting = tokio::time::timeout(CONNECT_PATIENCE, connect(&request, secure));
    // A close asked while the socket opens ends it before it is open: nothing is said to the room.
    let opened = tokio::select! {
        opened = connecting => opened.unwrap_or(Err(())),
        _ = closed_before_open(&mut asked) => Err(()),
    };
    let ended = match opened {
        Err(()) => {
            tracing::info!(secure, "call link: the socket did not open");
            events(json!({"type": "error"}));
            Ended { code: ABNORMAL, reason: String::new(), clean: false }
        }
        Ok(Socket::Plain(socket)) => serve(*socket, &kept, &mut asked, &events).await,
        Ok(Socket::Tls(socket)) => serve(*socket, &kept, &mut asked, &events).await,
    };
    tracing::info!(code = ended.code, clean = ended.clean, "call link closed");
    {
        let mut kept = kept.lock().unwrap();
        kept.ready = Ready::Closed;
        kept.ended = Some(ended.clone());
    }
    events(ended.json());
}

/// Resolves once a close is asked; sends asked before the link opens cannot be (they are refused).
async fn closed_before_open(asked: &mut mpsc::UnboundedReceiver<Ask>) {
    loop {
        match asked.recv().await {
            Some(Ask::Close(..)) | None => return,
            Some(Ask::Send(_)) => {}
        }
    }
}

enum Socket {
    Plain(Box<WebSocketStream<TcpStream>>),
    Tls(Box<WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>>),
}

/// The socket opened, with the request's subprotocols (the room answers with the first) and language.
async fn connect(request: &LinkRequest, secure: bool) -> Result<Socket, ()> {
    let mut handshake = request.url.as_str().into_client_request().map_err(|_| ())?;
    if !request.protocols.is_empty() {
        let offered = HeaderValue::from_str(&request.protocols.join(", ")).map_err(|_| ())?;
        handshake.headers_mut().insert("Sec-WebSocket-Protocol", offered);
    }
    if let Some(language) = request.language.as_deref().and_then(|language| HeaderValue::from_str(language).ok()) {
        handshake.headers_mut().insert("Accept-Language", language);
    }
    let uri = handshake.uri().clone();
    let host = uri.host().ok_or(())?.trim_start_matches('[').trim_end_matches(']').to_owned();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let tcp = TcpStream::connect((host.as_str(), port)).await.map_err(|_| ())?;
    let _ = tcp.set_nodelay(true);
    if !secure {
        let (socket, _) = tokio_tungstenite::client_async(handshake, tcp).await.map_err(|_| ())?;
        return Ok(Socket::Plain(Box::new(socket)));
    }
    let name = rustls::pki_types::ServerName::try_from(host).map_err(|_| ())?;
    let tls = tokio_rustls::TlsConnector::from(tls_config()?).connect(name, tcp).await.map_err(|_| ())?;
    let (socket, _) = tokio_tungstenite::client_async(handshake, tls).await.map_err(|_| ())?;
    Ok(Socket::Tls(Box::new(socket)))
}

/// The TLS the app's other requests use: the system's own certificate verifier (rustls-platform-verifier; set up on
/// Android by the mobile app), on aws-lc-rs.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, ()> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| ())?
        .with_platform_verifier()
        .map_err(|_| ())?
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// An open link: the room's frames kept and told, the keepalive answered, the page's asks sent, until it closes.
async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut socket: WebSocketStream<S>,
    kept: &Mutex<Kept>,
    asked: &mut mpsc::UnboundedReceiver<Ask>,
    events: &LinkEvents,
) -> Ended {
    use futures_util::{SinkExt, StreamExt};
    {
        let mut kept = kept.lock().unwrap();
        if kept.ready == Ready::Connecting {
            kept.ready = Ready::Open;
        }
    }
    events(json!({"type": "open"}));
    tracing::info!("call link open");
    // The session the room said the link is for: a ping without one is answered for it.
    let mut session: Option<String> = None;
    let mut closing: Option<tokio::time::Instant> = None;
    loop {
        let patience = closing.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            ask = asked.recv(), if closing.is_none() => match ask {
                Some(Ask::Send(text)) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return Ended { code: ABNORMAL, reason: String::new(), clean: false };
                    }
                }
                // The page's close, or the registry let the link go (no one can ask anything of it any more).
                ask => {
                    let (code, reason) = match ask {
                        Some(Ask::Close(code, reason)) => (code, reason),
                        _ => (GOING_AWAY, String::new()),
                    };
                    kept.lock().unwrap().ready = Ready::Closing;
                    let frame = CloseFrame { code: CloseCode::from(code), reason: reason.into() };
                    if socket.close(Some(frame)).await.is_err() {
                        return Ended { code: ABNORMAL, reason: String::new(), clean: false };
                    }
                    closing = Some(tokio::time::Instant::now() + CLOSE_PATIENCE);
                }
            },
            _ = tokio::time::sleep_until(patience), if closing.is_some() => {
                return Ended { code: ABNORMAL, reason: String::new(), clean: false };
            }
            frame = socket.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    let text = text.to_string();
                    if let Some(pong) = keepalive(&text, &mut session) {
                        if socket.send(Message::Text(pong.into())).await.is_err() {
                            return Ended { code: ABNORMAL, reason: String::new(), clean: false };
                        }
                        continue;
                    }
                    let index = kept.lock().unwrap().keep(text.clone());
                    events(json!({"type": "message", "index": index, "data": text}));
                }
                Some(Ok(Message::Close(frame))) => {
                    // The room's close, answered by the socket itself; what it said is how the link ended.
                    let ended = match frame {
                        Some(frame) => Ended { code: frame.code.into(), reason: frame.reason.to_string(), clean: true },
                        None => Ended { code: NO_STATUS, reason: String::new(), clean: true },
                    };
                    kept.lock().unwrap().ready = Ready::Closing;
                    let _ = tokio::time::timeout(CLOSE_PATIENCE, async { while socket.next().await.is_some() {} }).await;
                    return ended;
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => {
                    return Ended { code: ABNORMAL, reason: String::new(), clean: false };
                }
            },
        }
    }
}

/// The room's keepalive, answered: a `voice-ping` gets its `voice-pong`, for the session the ping names, else the one
/// the room last said the link is for (its `voice-session`, which is recorded here and handed on). `None` for any other
/// frame, which is the page's. Only a frame that names one of the two is read as JSON: the rest are handed on unread.
fn keepalive(text: &str, session: &mut Option<String>) -> Option<String> {
    if !text.contains("voice-ping") && !text.contains("voice-session") {
        return None;
    }
    let frame: Value = serde_json::from_str(text).ok()?;
    let named = frame.get("data").and_then(|data| data.get("session_id")).and_then(Value::as_str);
    match frame.get("type").and_then(Value::as_str) {
        Some("voice-session") => {
            *session = named.map(str::to_owned);
            None
        }
        Some("voice-ping") => {
            let for_session = named.map(str::to_owned).or_else(|| session.clone());
            Some(json!({"type": "voice-pong", "data": {"session_id": for_session}}).to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    use super::*;

    /// What a link told its page, in order.
    fn recorder() -> (LinkEvents, Arc<Mutex<Vec<Value>>>) {
        let told = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&told);
        (Arc::new(move |event| kept.lock().unwrap().push(event)), told)
    }

    async fn until(what: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !what() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("in time");
    }

    fn types(told: &Mutex<Vec<Value>>) -> Vec<String> {
        told.lock().unwrap().iter().map(|event| event["type"].as_str().unwrap().to_owned()).collect()
    }

    /// A stand-in room on a local port: it answers the subprotocol the core answers (the first offered), records what
    /// was offered and what the link sends, and sends what the test hands it. Answers its address, the offers, what it
    /// received and the sender of its frames.
    #[expect(clippy::result_large_err, reason = "the handshake callback's error type is tungstenite's own")]
    async fn room() -> (String, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<Message>>>, mpsc::UnboundedSender<Message>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/api/presentation/ws", listener.local_addr().unwrap());
        let offers = Arc::new(Mutex::new(Vec::new()));
        let received = Arc::new(Mutex::new(Vec::new()));
        let (frames, mut to_send) = mpsc::unbounded_channel::<Message>();
        let (offered, got) = (Arc::clone(&offers), Arc::clone(&received));
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let callback = |request: &Request, mut response: Response| {
                let header = request.headers().get("Sec-WebSocket-Protocol").map(|v| v.to_str().unwrap().to_owned());
                if let Some(header) = header {
                    offered.lock().unwrap().push(header.clone());
                    let first = header.split(',').next().unwrap().trim().to_owned();
                    response.headers_mut().insert("Sec-WebSocket-Protocol", HeaderValue::from_str(&first).unwrap());
                }
                if let Some(language) = request.headers().get("Accept-Language") {
                    offered.lock().unwrap().push(format!("lang={}", language.to_str().unwrap()));
                }
                Ok(response)
            };
            let mut socket = tokio_tungstenite::accept_hdr_async(tcp, callback).await.unwrap();
            loop {
                tokio::select! {
                    out = to_send.recv() => match out {
                        Some(message) => { if socket.send(message).await.is_err() { return; } }
                        None => return,
                    },
                    incoming = socket.next() => match incoming {
                        Some(Ok(message)) => got.lock().unwrap().push(message),
                        _ => return,
                    },
                }
            }
        });
        (url, offers, received, frames)
    }

    fn request(url: &str) -> LinkRequest {
        LinkRequest {
            url: url.to_owned(),
            protocols: vec!["sidevoice".into(), "sidevoice.token.secret".into()],
            language: Some("es-ES".into()),
        }
    }

    fn texts(received: &Mutex<Vec<Message>>) -> Vec<String> {
        received
            .lock()
            .unwrap()
            .iter()
            .filter_map(|message| match message {
                Message::Text(text) => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    static KEYS: AtomicU64 = AtomicU64::new(0);
    fn key() -> String {
        format!("link-{}", KEYS.fetch_add(1, Ordering::SeqCst))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_link_opens_with_the_subprotocols_sends_in_order_and_hands_on_the_rooms_frames() {
        let (url, offers, received, room_says) = room().await;
        let links = CallLinks::new(Handle::current());
        let (events, told) = recorder();
        let key = key();
        links.open(1, key.clone(), request(&url), events).unwrap();
        assert_eq!(links.send(1, &key, "too early".into()), Err(NOT_OPEN), "a browser throws before open too");
        until(|| types(&told) == ["open"]).await;
        assert_eq!(*offers.lock().unwrap(), ["sidevoice, sidevoice.token.secret", "lang=es-ES"]);
        for text in ["hello", "turn 1", "turn 2"] {
            links.send(1, &key, text.into()).unwrap();
        }
        until(|| texts(&received).len() == 3).await;
        assert_eq!(texts(&received), ["hello", "turn 1", "turn 2"]);
        let session = r#"{"type":"voice-session","data":{"session_id":"s1"}}"#;
        room_says.send(Message::Text(session.into())).unwrap();
        room_says.send(Message::Text(r#"{"type":"voice-reply","seq":1}"#.into())).unwrap();
        until(|| told.lock().unwrap().len() == 3).await;
        let told = told.lock().unwrap().clone();
        assert_eq!(told[1], json!({"type": "message", "index": 1, "data": session}));
        assert_eq!(told[2]["index"], 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_keepalive_is_answered_by_the_link_and_never_reaches_the_page() {
        let (url, _, received, room_says) = room().await;
        let links = CallLinks::new(Handle::current());
        let (events, told) = recorder();
        let key = key();
        links.open(1, key.clone(), request(&url), events).unwrap();
        until(|| types(&told) == ["open"]).await;
        room_says.send(Message::Text(r#"{"type":"voice-session","data":{"session_id":"s9"}}"#.into())).unwrap();
        room_says.send(Message::Text(r#"{"type":"voice-ping","data":{"session_id":"s9"}}"#.into())).unwrap();
        room_says.send(Message::Text(r#"{"type":"voice-ping","data":{}}"#.into())).unwrap();
        until(|| texts(&received).len() == 2).await;
        let pongs: Vec<Value> = texts(&received).iter().map(|text| serde_json::from_str(text).unwrap()).collect();
        assert_eq!(pongs, vec![json!({"type": "voice-pong", "data": {"session_id": "s9"}}); 2]);
        assert_eq!(types(&told), ["open", "message"], "only the session reached the page");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_page_that_comes_back_reads_what_came_after_its_last_index_and_how_the_link_ended() {
        let (url, _, _, room_says) = room().await;
        let links = CallLinks::new(Handle::current());
        let (events, told) = recorder();
        let key = key();
        links.open(1, key.clone(), request(&url), events).unwrap();
        until(|| types(&told) == ["open"]).await;
        for n in 1..=3 {
            room_says.send(Message::Text(format!("frame {n}").into())).unwrap();
        }
        room_says
            .send(Message::Close(Some(CloseFrame { code: CloseCode::from(4401), reason: "unpaired".into() })))
            .unwrap();
        until(|| types(&told).last().map(String::as_str) == Some("close")).await;
        let caught = links.since(1, &key, 1).unwrap();
        assert_eq!(caught["readyState"], 3);
        assert_eq!(caught["frames"], json!([{"index": 2, "data": "frame 2"}, {"index": 3, "data": "frame 3"}]));
        assert_eq!(caught["close"], json!({"code": 4401, "reason": "unpaired", "wasClean": true}));
        assert_eq!(links.since(2, &key, 0), Err(UNKNOWN), "another page's link is not this page's");
        links.forget(1, &key).unwrap();
        assert!(links.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_page_hangs_up_with_its_code_and_a_replaced_pages_links_go_away_without_hanging_up() {
        let (url, _, received, _room_says) = room().await;
        let links = CallLinks::new(Handle::current());
        let (events, told) = recorder();
        let key = key();
        links.open(1, key.clone(), request(&url), events).unwrap();
        until(|| types(&told) == ["open"]).await;
        links.close_older_than(2);
        until(|| received.lock().unwrap().iter().any(|m| matches!(m, Message::Close(_)))).await;
        let code = received.lock().unwrap().iter().find_map(|m| match m {
            Message::Close(Some(frame)) => Some(u16::from(frame.code)),
            _ => None,
        });
        assert_eq!(code, Some(GOING_AWAY), "the room parks the call for the next page");
        assert!(links.is_empty());

        let (url, _, received, _room_says) = room().await;
        let (events, told) = recorder();
        let key = super::tests::key();
        links.open(2, key.clone(), request(&url), events).unwrap();
        until(|| types(&told) == ["open"]).await;
        links.close(2, &key, Some(1000), None).unwrap();
        until(|| received.lock().unwrap().iter().any(|m| matches!(m, Message::Close(_)))).await;
        let code = received.lock().unwrap().iter().find_map(|m| match m {
            Message::Close(Some(frame)) => Some(u16::from(frame.code)),
            _ => None,
        });
        assert_eq!(code, Some(1000));
        until(|| types(&told).last().map(String::as_str) == Some("close")).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_link_that_cannot_open_says_error_then_an_abnormal_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        drop(listener);
        let links = CallLinks::new(Handle::current());
        let (events, told) = recorder();
        let key = key();
        links.open(1, key.clone(), request(&url), events).unwrap();
        until(|| types(&told) == ["error", "close"]).await;
        assert_eq!(told.lock().unwrap()[1], json!({"type": "close", "code": 1006, "reason": "", "wasClean": false}));
        assert_eq!(links.open(1, "x".into(), request("https://example.com"), recorder().0), Err(BAD_URL));
        assert_eq!(links.open(1, key, request(&url), recorder().0), Err(TAKEN));
    }

    #[test]
    fn the_kept_frames_stay_within_their_limits() {
        let mut kept = Kept::new();
        for n in 0..(KEPT_FRAMES + 10) {
            kept.keep(format!("{n}"));
        }
        assert_eq!(kept.frames.len(), KEPT_FRAMES);
        assert_eq!(kept.frames.front().unwrap().0, 11);
        let mut big = Kept::new();
        big.keep("a".repeat(KEPT_BYTES));
        big.keep("b".repeat(10));
        assert_eq!(big.frames.len(), 1, "the oldest goes past the bytes");
        assert_eq!(big.bytes, 10);
    }
}
