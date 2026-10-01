//! The local host, tied together: the poll that finds the core and keeps the app paired with it, the proxy, and the
//! actions the bridge offers (`localHost.*`, SEAMS §5).
//!
//! **Startup cases** (design §4.1), on each core launch the app sees:
//! - the socket healthy and no stored token → pair (`POST /api/device/local/pair`);
//! - a stored fingerprint that is not the core's → pair (the core was reset);
//! - the same fingerprint → the core signs a fresh nonce with the pinned key, then the stored token is tried; refused
//!   → `refused`, and only [`LocalHost::reconnect`] pairs again;
//! - the token cannot be stored → `DELETE /api/device/local` and `failed` with `app.storage`.
//!
//! A pairing the core answers is checked before it is kept: the node it names is the core that answered health
//! (fingerprint, and the fingerprint is its key's), and that key signs a fresh nonce.
//!
//! **The poll** (every [`POLL_EVERY`] while the app runs): `node.status` on the connector's socket; when no connector
//! answers, `service status --json` through the CLI (at most every [`FALLBACK_EVERY`], or right after an action: it
//! starts a process); the core's health; and while paired, whether the core still accepts the token — refused, the
//! proxy stops carrying it and closes its tunnels.

use crate::cli::Cli;
use crate::connector;
use crate::core_socket::{CoreError, CoreSocket, Health, Paired};
use crate::identity;
use crate::paths::DataDirs;
use crate::proxy::Proxy;
use crate::state::{self, Link, Observed, Report, State};
use crate::store::{self, Pairing};
use crate::Refusal;
use serde_json::{json, Value};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const POLL_EVERY: Duration = Duration::from_secs(2);
pub const FALLBACK_EVERY: Duration = Duration::from_secs(10);

/// What the bridge's actions run (SEAMS §3), and how long each may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    ServiceInstall,
    ServiceUninstall,
}

impl Action {
    pub fn parse(name: &str) -> Option<Action> {
        Some(match name {
            "start" => Action::Start,
            "stop" => Action::Stop,
            "restart" => Action::Restart,
            "service-install" => Action::ServiceInstall,
            "service-uninstall" => Action::ServiceUninstall,
            _ => return None,
        })
    }

    fn argv(self) -> [&'static str; 3] {
        let verb = match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Restart => "restart",
            Action::ServiceInstall => "install",
            Action::ServiceUninstall => "uninstall",
        };
        ["service", verb, "--json"]
    }

    /// The launcher waits ≤ 10 s for a socket; stop and uninstall wait ≤ 15 s for the supervisor and core, then kill.
    fn timeout(self) -> Duration {
        match self {
            Action::Start | Action::Restart => Duration::from_secs(30),
            Action::Stop | Action::ServiceInstall | Action::ServiceUninstall => Duration::from_secs(45),
        }
    }
}

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const PAIR_DEVICE_TIMEOUT: Duration = Duration::from_secs(30);
const PAIR_ROOM_TIMEOUT: Duration = Duration::from_secs(60);

/// Where the app keeps its side, and what it calls itself.
#[derive(Debug, Clone)]
pub struct Config {
    pub dirs: DataDirs,
    /// The app's config directory: `local-host.json` goes here.
    pub app_dir: PathBuf,
    /// The device name the core lists this app under (the computer's name).
    pub name: String,
}

struct Inner {
    link: Link,
    /// The core launch `link` was established against (launch id and fingerprint): a new one checks again.
    launch: Option<(Option<String>, String)>,
    /// The pairing in use, once proven this launch.
    pairing: Option<Pairing>,
    report: Report,
    /// `service status --json`'s last answer and when, while no connector answers.
    fallback: Option<(Instant, Option<Value>)>,
    /// `reconnect()`: pair anew on the next pass, whatever is stored.
    force_pair: bool,
}

pub struct LocalHost {
    config: Config,
    core: CoreSocket,
    proxy: Proxy,
    inner: Mutex<Inner>,
    /// One pass at a time: the poll thread's and an action's.
    passes: Mutex<()>,
    stopped: AtomicBool,
    log: Arc<dyn Fn(&str) + Send + Sync>,
}

impl LocalHost {
    /// Starts the proxy (for the app's lifetime) and returns the host; nothing is polled until [`Self::poll`] or
    /// [`Self::watch`].
    pub fn start(config: Config, log: impl Fn(&str) + Send + Sync + 'static) -> io::Result<Arc<LocalHost>> {
        let log: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(log);
        let core = CoreSocket::new(&config.dirs);
        let proxy_log = log.clone();
        let proxy = Proxy::start(core.clone(), move |line| proxy_log(line))?;
        log(&format!("local-host proxy on {}", proxy.url()));
        Ok(Arc::new(LocalHost {
            config,
            core,
            proxy,
            inner: Mutex::new(Inner {
                link: Link::Unpaired,
                launch: None,
                pairing: None,
                report: Report::new(State::Absent),
                fallback: None,
                force_pair: false,
            }),
            passes: Mutex::new(()),
            stopped: AtomicBool::new(false),
            log,
        }))
    }

    /// Polls every [`POLL_EVERY`] on a thread of its own until [`Self::shutdown`].
    pub fn watch(self: &Arc<Self>) {
        let host = self.clone();
        let _ = std::thread::Builder::new().name("local-host-poll".into()).spawn(move || {
            while !host.stopped.load(Ordering::SeqCst) {
                host.poll();
                std::thread::sleep(POLL_EVERY);
            }
        });
    }

    /// Stops the poll and the proxy; every tunnel closes.
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.proxy.stop();
    }

    pub fn proxy(&self) -> &Proxy {
        &self.proxy
    }

    pub fn state(&self) -> Report {
        self.inner.lock().unwrap().report.clone()
    }

    /// The local host as the page uses it (design §4.1): the pinned identity, the proxy's URL and this launch's
    /// secret in place of a token. Whenever a core answers and the pairing works (`reachable`), whatever `state`
    /// says (a core with no service is `not-installed` and still usable); `None` otherwise.
    pub fn pairing(&self) -> Option<Value> {
        let inner = self.inner.lock().unwrap();
        let pairing = inner.pairing.as_ref().filter(|_| inner.report.reachable)?;
        Some(json!({
            "fp": pairing.fp,
            "public_key": pairing.public_key,
            "device_id": pairing.device_id,
            "token": self.proxy.secret(),
            "urls": [self.proxy.url()],
            "rv": null,
            "host": pairing.host,
            "local": true,
        }))
    }

    /// One pass: observe, keep the pairing, decide the state.
    pub fn poll(&self) -> Report {
        let _pass = self.passes.lock().unwrap();
        // A supervisor's `node.status` is the service's state. A plain connector (`supervisor: false`) speaks only for the
        // core it started on demand: the service's state is then the CLI's (SEAMS §5).
        let status = match connector::node_status(&self.config.dirs) {
            Some(status) if status.get("supervisor") == Some(&Value::Bool(false)) => self.fallback_status(),
            Some(status) => Some(status),
            None => self.fallback_status(),
        };
        let health = self.core.health();
        if let Ok(health) = &health {
            self.keep_link(health);
        } else {
            // No core to carry requests to; a core that comes back is checked again before the token goes out.
            self.proxy.set_token(None);
        }
        let mut inner = self.inner.lock().unwrap();
        let report = state::report(Observed { status: status.as_ref(), core: health.as_ref(), link: &inner.link });
        if report.state != inner.report.state {
            (self.log)(&format!("local-host state {}", json!(report.state).as_str().unwrap_or("?")));
        }
        inner.report = report.clone();
        report
    }

    /// `service status --json` when no connector answers, reused for [`FALLBACK_EVERY`]. No install, no answer.
    fn fallback_status(&self) -> Option<Value> {
        if let Some((at, answer)) = &self.inner.lock().unwrap().fallback {
            if at.elapsed() < FALLBACK_EVERY {
                return answer.clone();
            }
        }
        let answer = match Cli::installed(&self.config.dirs) {
            Err(refusal) if refusal.key == "cli.unavailable" => None,
            Err(refusal) => Some(json!({ "state": "failed", "failure": refusal })),
            Ok(cli) => match cli.run(&["service", "status", "--json"], STATUS_TIMEOUT) {
                Ok(answer) => Some(answer),
                Err(refusal) => Some(json!({ "state": "failed", "failure": refusal })),
            },
        };
        self.inner.lock().unwrap().fallback = Some((Instant::now(), answer.clone()));
        answer
    }

    /// The startup cases and the check of a pairing in use, against the core that answered `health`.
    fn keep_link(&self, health: &Health) {
        if !state::API.contains(&health.api.unwrap_or(i64::MIN)) {
            self.proxy.set_token(None);
            return;
        }
        let launch = (health.launch_id.clone(), health.fingerprint.clone());
        let (link, force) = {
            let mut inner = self.inner.lock().unwrap();
            let new_launch = inner.launch.as_ref() != Some(&launch);
            if new_launch {
                inner.launch = Some(launch);
                // A refusal holds until `reconnect()` while the identity is the same; anything else is checked again.
                let same_identity = inner.pairing.as_ref().map(|p| &p.fp) == Some(&health.fingerprint)
                    || store::load(&self.config.app_dir).map(|p| p.fp) == Some(health.fingerprint.clone());
                if !(inner.link == Link::Refused && same_identity) {
                    inner.link = Link::Unpaired;
                    inner.pairing = None;
                }
            }
            (inner.link.clone(), std::mem::take(&mut inner.force_pair))
        };
        let next = match link {
            _ if force => self.pair(health),
            Link::Unpaired => self.establish(health),
            Link::Paired => self.recheck(),
            Link::Refused | Link::Failed(_) => None,
        };
        let mut inner = self.inner.lock().unwrap();
        if let Some((link, pairing)) = next {
            inner.link = link;
            inner.pairing = pairing;
        }
        let token = inner.pairing.as_ref().filter(|_| inner.link == Link::Paired).map(|p| p.token.clone());
        self.proxy.set_token(token);
    }

    /// Unpaired with this launch: use the stored pairing if it is this core's and still accepted, else pair. `None`:
    /// nothing decided (the core did not answer in time); the next pass tries again.
    fn establish(&self, health: &Health) -> Option<(Link, Option<Pairing>)> {
        let stored = match store::load(&self.config.app_dir) {
            Some(stored) if stored.fp == health.fingerprint => stored,
            // None stored, or another core's (it was reset): pair.
            _ => return self.pair(health),
        };
        if identity::fingerprint(&stored.public_key).as_deref() != Some(stored.fp.as_str())
            || health.public_key != stored.public_key
        {
            return Some((Link::Failed(mismatch()), None));
        }
        match self.core.proves(&stored.public_key) {
            Ok(true) => {}
            Ok(false) => return Some((Link::Failed(mismatch()), None)),
            Err(_) => return None,
        }
        match self.core.accepts(&stored.token) {
            Ok(true) => {
                (self.log)(&format!("local-host paired (stored) {}", stored.redacted()));
                Some((Link::Paired, Some(stored)))
            }
            Ok(false) => {
                (self.log)("local-host token refused by the core");
                Some((Link::Refused, None))
            }
            Err(_) => None,
        }
    }

    /// Paired: the core still accepts the token (a revocation from another device shows here).
    fn recheck(&self) -> Option<(Link, Option<Pairing>)> {
        let token = self.inner.lock().unwrap().pairing.as_ref()?.token.clone();
        match self.core.accepts(&token) {
            Ok(false) => {
                (self.log)("local-host token refused by the core");
                Some((Link::Refused, None))
            }
            _ => None,
        }
    }

    /// A new local device for this app, checked to be the core that answered, then stored.
    fn pair(&self, health: &Health) -> Option<(Link, Option<Pairing>)> {
        let paired: Paired = match self.core.pair(&self.config.name) {
            Ok(paired) => paired,
            Err(CoreError::Failed(refusal)) => return Some((Link::Failed(refusal), None)),
            Err(_) => return None,
        };
        let node = &paired.node;
        let genuine = node.fingerprint == health.fingerprint
            && identity::fingerprint(&node.public_key).as_deref() == Some(node.fingerprint.as_str())
            && matches!(self.core.proves(&node.public_key), Ok(true));
        if !genuine {
            let _ = self.core.unpair();
            return Some((Link::Failed(mismatch()), None));
        }
        let pairing = Pairing {
            fp: node.fingerprint.clone(),
            public_key: node.public_key.clone(),
            device_id: paired.device_id,
            token: paired.token,
            host: node.host.clone().or_else(|| health.host.clone()).unwrap_or_default(),
            paired_at: store::now_iso(),
        };
        if let Err(e) = store::save(&self.config.app_dir, &pairing) {
            // Never a device the app cannot remember: revoke it at once.
            let _ = self.core.unpair();
            (self.log)(&format!("local-host could not store its pairing: {e}"));
            let refusal = Refusal::new("app.storage", format!("The app could not store its pairing: {e}"));
            return Some((Link::Failed(refusal), None));
        }
        (self.log)(&format!("local-host paired {}", pairing.redacted()));
        Some((Link::Paired, Some(pairing)))
    }

    /// «Volver a conectar»: pair anew (the core revokes the app's previous local device), then the state.
    pub fn reconnect(&self) -> Report {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.force_pair = true;
            inner.link = Link::Unpaired;
            inner.pairing = None;
        }
        self.poll()
    }

    fn cli(&self) -> Result<Cli, Refusal> {
        Cli::installed(&self.config.dirs)
    }

    /// Runs a service action, then a fresh pass (the CLI's view refreshed too): the state after it.
    pub fn act(&self, action: Action) -> Result<Report, Refusal> {
        let result = self.cli()?.run(&action.argv(), action.timeout());
        self.inner.lock().unwrap().fallback = None;
        result?;
        Ok(self.poll())
    }

    /// `pair-device --json` (only on the person's click): `{code, expires_in, reach}`.
    pub fn pairing_code(&self) -> Result<Value, Refusal> {
        let answer = self.cli()?.run(&["pair-device", "--json"], PAIR_DEVICE_TIMEOUT)?;
        Ok(json!({ "code": answer["code"], "expires_in": answer["expires_in"], "reach": answer["reach"] }))
    }

    /// `pair <room-url> <code> --json`: this machine paired with a room (the connector restarts the core): `{room}`.
    pub fn pair_room(&self, url: &str, code: &str) -> Result<Value, Refusal> {
        let plain = |text: &str, limit: usize| {
            !text.is_empty()
                && text.len() <= limit
                && text.chars().all(|c| c.is_ascii_graphic())
                && !text.starts_with('-')
        };
        if !plain(url, 2048) || !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(Refusal::new("bad_request", "The room's address is not an http(s) URL."));
        }
        if !plain(code, 512) {
            return Err(Refusal::new("bad_request", "That is not a pairing code."));
        }
        let answer = self.cli()?.run(&["pair", url, code, "--json"], PAIR_ROOM_TIMEOUT)?;
        self.inner.lock().unwrap().fallback = None;
        Ok(json!({ "room": answer["room"] }))
    }

    /// The log «Ver registro» shows.
    pub fn log_path(&self) -> Option<PathBuf> {
        self.config.dirs.log()
    }
}

impl Drop for LocalHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn mismatch() -> Refusal {
    Refusal::new("identity.mismatch", "The core on this computer did not prove the identity the app paired with.")
}

/// The computer's name, for the core's device list.
pub fn computer_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname NUL-terminates within it or truncates.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = if ok { String::from_utf8_lossy(&buf[..end]).into_owned() } else { String::new() };
    let name = name.strip_suffix(".local").unwrap_or(&name).to_string();
    if name.is_empty() {
        "Sidevoice".into()
    } else {
        name
    }
}
