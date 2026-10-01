//! A stand-in for sidevoice-core's private listener (SEAMS §2), for this crate's tests, the CI example
//! (`examples/local-host-ci.rs`) and the app's macOS CI probe. Feature `fake-core`; never in the app.
//!
//! It serves `C/local.sock` (creating `D` and `C` 0700) with what the app uses — health, the identity proof with a
//! real P-256 key, local pairing (one local device at a time), its revocation — plus, with a device token, an echo of
//! any other request (`{method, path, origin, auth, body}`, with the CORS the core sends app origins) and a
//! WebSocket echo. `/api/connectors/link*` answers too, so a test sees it is never reached through the proxy. Every
//! request is recorded (`requests()`), as `METHOD /path origin=… auth=device|none|other`.

use crate::http::{self, Headers};
use crate::identity;
use crate::paths::DataDirs;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const DESKTOP_ORIGINS: [&str; 3] = ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"];

#[derive(Default)]
struct State {
    /// token → device id; `local` names the one local device.
    devices: HashMap<String, String>,
    local: Option<String>,
    launch: u32,
    api: i64,
    counter: u32,
    requests: Vec<String>,
    /// Calls (open sockets) per device.
    sockets: HashMap<String, Vec<UnixStream>>,
}

pub struct FakeCore {
    key: SigningKey,
    public_key: String,
    fingerprint: String,
    state: Mutex<State>,
    stopped: AtomicBool,
    socket: std::path::PathBuf,
    print: bool,
}

impl FakeCore {
    /// Serves `C/local.sock` under `dirs` until [`Self::stop`]; `seed` picks the identity (another seed is another
    /// machine, as after a reset). `print`: each request on stdout too (CI reads it).
    pub fn start(dirs: &DataDirs, seed: u8, print: bool) -> io::Result<Arc<FakeCore>> {
        for dir in [&dirs.data, &dirs.core()] {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let socket = dirs.core_socket();
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let key = SigningKey::from_slice(&[seed.max(1); 32]).expect("a valid scalar");
        let public_key = identity::spki(key.verifying_key());
        let fingerprint = identity::fingerprint(&public_key).expect("base64");
        let core = Arc::new(FakeCore {
            key,
            public_key,
            fingerprint,
            state: Mutex::new(State { launch: 1, api: 1, ..State::default() }),
            stopped: AtomicBool::new(false),
            socket,
            print,
        });
        let serving = core.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                if serving.stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let core = serving.clone();
                thread::spawn(move || {
                    let _ = core.serve(stream);
                });
            }
        });
        Ok(core)
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    pub fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }

    /// The local device's id, if one is paired.
    pub fn local_device(&self) -> Option<String> {
        self.state.lock().unwrap().local.clone()
    }

    pub fn set_api(&self, api: i64) {
        self.state.lock().unwrap().api = api;
    }

    /// As if the core were restarted: a new launch id, its registry kept.
    pub fn relaunch(&self) {
        self.state.lock().unwrap().launch += 1;
    }

    /// As if another device revoked every device (or the list was lost): each token refused, each call ended.
    pub fn revoke_all(&self) {
        let mut state = self.state.lock().unwrap();
        state.devices.clear();
        state.local = None;
        for (_, sockets) in state.sockets.drain() {
            sockets.iter().for_each(|s| drop(s.shutdown(std::net::Shutdown::Both)));
        }
    }

    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = UnixStream::connect(&self.socket);
        let _ = std::fs::remove_file(&self.socket);
    }

    fn record(&self, line: String) {
        if self.print {
            println!("fake-core {line}");
        }
        self.state.lock().unwrap().requests.push(line);
    }

    fn serve(&self, mut stream: UnixStream) -> io::Result<()> {
        let mut buf = Vec::new();
        let end = http::read_head(&mut stream, &mut buf)?;
        let head = http::parse_request(&buf[..end]).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let headers = &head.headers;
        let length: usize = headers.first("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut body = buf[end..].to_vec();
        while body.len() < length {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        let (path, query) = head.target.split_once('?').unwrap_or((&head.target, ""));
        let origin = headers.first("origin").unwrap_or("-").to_string();
        let socket_token = headers
            .first("sec-websocket-protocol")
            .unwrap_or("")
            .split(',')
            .find_map(|p| p.trim().strip_prefix("sidevoice.token.").map(str::to_string));
        let bearer = headers.first("authorization").and_then(|v| v.strip_prefix("Bearer ")).map(str::to_string);
        let offered = socket_token.or(bearer);
        let device = offered.as_ref().and_then(|t| self.state.lock().unwrap().devices.get(t).cloned());
        let auth = match (&offered, &device) {
            (None, _) => "none",
            (Some(_), Some(_)) => "device",
            (Some(_), None) => "other",
        };
        self.record(format!("{} {path} origin={origin} auth={auth}", head.method));

        let cors = |answer: &mut Headers| {
            if DESKTOP_ORIGINS.contains(&origin.as_str()) {
                answer.push("access-control-allow-origin", origin.as_str());
                answer.push("vary", "Origin");
            }
        };
        let reply = |stream: &mut UnixStream, status: u16, body: Value| -> io::Result<()> {
            let data = serde_json::to_vec(&body).expect("serialises");
            let mut answer = Headers::default();
            answer.push("content-type", "application/json");
            answer.push("content-length", data.len().to_string());
            cors(&mut answer);
            let mut out = http::response_head(status, http::reason(status), &answer);
            out.extend_from_slice(&data);
            stream.write_all(&out)
        };

        // As the core on its socket (SEAMS §2): loopback names only, and what only the socket serves is refused to
        // anything carrying an Origin — native's own calls carry none.
        let host = headers.first("host").unwrap_or("");
        let name = match host.rsplit_once(':') {
            Some((name, port)) if !host.starts_with('[') && port.bytes().all(|b| b.is_ascii_digit()) => name,
            _ => host.trim_start_matches('[').split(']').next().unwrap_or(""),
        };
        if !["localhost", "127.0.0.1", "::1"].contains(&name.to_ascii_lowercase().as_str()) {
            return reply(&mut stream, 421, json!({"detail": "This node answers to its own name only."}));
        }
        let socket_only =
            ["/api/local", "/api/device/local", "/api/connectors/link"].iter().any(|p| path.starts_with(p));
        if socket_only && headers.has("origin") {
            return reply(&mut stream, 404, json!({"detail": "Not Found"}));
        }

        let upgrade = headers.first("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
        match (head.method.as_str(), path) {
            ("GET", "/api/local/health") => {
                let state = self.state.lock().unwrap();
                let calls: usize = state.sockets.values().map(Vec::len).sum();
                let health = json!({
                    "launch_id": format!("launch-{}", state.launch), "pid": std::process::id(), "version": "0.0.0-fake",
                    "api": state.api, "fingerprint": self.fingerprint, "public_key": self.public_key,
                    "host": "fake-machine", "calls": calls,
                });
                drop(state);
                reply(&mut stream, 200, health)
            }
            ("GET", "/api/device/identity") => {
                let nonce = query.split('&').find_map(|kv| kv.strip_prefix("nonce=")).unwrap_or("");
                let signature: Signature = self.key.sign(format!("{}{nonce}", identity::CONTEXT).as_bytes());
                let proof = json!({
                    "fingerprint": self.fingerprint, "public_key": self.public_key,
                    "signature": identity::base64url(&signature.to_bytes()),
                });
                reply(&mut stream, 200, proof)
            }
            ("POST", "/api/device/local/pair") => {
                let mut state = self.state.lock().unwrap();
                if let Some(previous) = state.local.take() {
                    state.devices.retain(|_, id| *id != previous);
                    for socket in state.sockets.remove(&previous).unwrap_or_default() {
                        let _ = socket.shutdown(std::net::Shutdown::Both);
                    }
                }
                state.counter += 1;
                let device_id = format!("local-{}", state.counter);
                let token = identity::random(24);
                state.devices.insert(token.clone(), device_id.clone());
                state.local = Some(device_id.clone());
                drop(state);
                let node =
                    json!({"fingerprint": self.fingerprint, "public_key": self.public_key, "host": "fake-machine"});
                reply(&mut stream, 200, json!({"device_id": device_id, "token": token, "node": node}))
            }
            ("DELETE", "/api/device/local") => {
                let mut state = self.state.lock().unwrap();
                let revoked = state.local.take();
                if let Some(previous) = &revoked {
                    state.devices.retain(|_, id| id != previous);
                }
                drop(state);
                reply(&mut stream, 200, json!({"ok": true, "revoked": revoked.is_some()}))
            }
            (_, p) if p.starts_with("/api/connectors/link") => reply(&mut stream, 200, json!({"link": true})),
            _ if device.is_none() => reply(&mut stream, 401, json!({"detail": "unpaired"})),
            ("GET", "/api/device/devices") => reply(&mut stream, 200, json!({"devices": []})),
            ("GET", _) if upgrade => self.echo_socket(stream, headers, device.unwrap_or_default()),
            (method, _) => {
                let echoed = json!({
                    "method": method, "path": path, "origin": origin, "auth": auth,
                    "body": String::from_utf8_lossy(&body[..length.min(body.len())]),
                });
                reply(&mut stream, 200, echoed)
            }
        }
    }

    /// Accepts the socket (with `sidevoice` when offered, as the core does) and echoes every frame until close.
    fn echo_socket(&self, mut stream: UnixStream, headers: &Headers, device: String) -> io::Result<()> {
        let key = headers.first("sec-websocket-key").unwrap_or("");
        let accept = STANDARD.encode(Sha1::digest(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes()));
        let mut answer = Headers::default();
        answer.push("upgrade", "websocket");
        answer.push("connection", "Upgrade");
        answer.push("sec-websocket-accept", accept);
        if headers.lists("sec-websocket-protocol", "sidevoice") {
            answer.push("sec-websocket-protocol", "sidevoice");
        }
        stream.write_all(&http::response_head(101, "Switching Protocols", &answer))?;
        if let Ok(clone) = stream.try_clone() {
            self.state.lock().unwrap().sockets.entry(device.clone()).or_default().push(clone);
        }
        let result = echo_frames(&mut stream);
        if let Some(sockets) = self.state.lock().unwrap().sockets.get_mut(&device) {
            sockets.pop();
        }
        result
    }
}

impl Drop for FakeCore {
    fn drop(&mut self) {
        self.stop();
    }
}

fn echo_frames(stream: &mut UnixStream) -> io::Result<()> {
    loop {
        let (opcode, payload) = read_frame(stream)?;
        write_frame(stream, opcode, &payload)?;
        if opcode == 8 {
            return Ok(());
        }
    }
}

/// One frame, unmasked (a client's frames are masked).
pub fn read_frame(stream: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head)?;
    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let length = match head[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            stream.read_exact(&mut b)?;
            u16::from_be_bytes(b) as usize
        }
        127 => {
            let mut b = [0u8; 8];
            stream.read_exact(&mut b)?;
            u64::from_be_bytes(b) as usize
        }
        n => n as usize,
    };
    let mut mask = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload)?;
    if masked {
        payload.iter_mut().enumerate().for_each(|(i, b)| *b ^= mask[i % 4]);
    }
    Ok((opcode, payload))
}

/// One final frame, unmasked (a server's).
pub fn write_frame(stream: &mut impl Write, opcode: u8, payload: &[u8]) -> io::Result<()> {
    write_frame_masked(stream, opcode, payload, None)
}

/// One final frame, masked with `mask` when given (a client's).
pub fn write_frame_masked(
    stream: &mut impl Write,
    opcode: u8,
    payload: &[u8],
    mask: Option<[u8; 4]>,
) -> io::Result<()> {
    let mut out = vec![0x80 | opcode];
    let bit = if mask.is_some() { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => out.push(bit | n as u8),
        n if n <= u16::MAX as usize => {
            out.push(bit | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(bit | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    match mask {
        Some(mask) => {
            out.extend_from_slice(&mask);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        }
        None => out.extend_from_slice(payload),
    }
    stream.write_all(&out)
}
