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
//!
//! **The installed version** (`product_version`): the installation's program asked `--version --json` — the release
//! `install.json`'s command runs, so the installed `sidevoice` package's version. Neither `node.status` nor
//! `service status --json` carries it. It starts a process, so it is asked once per installed program: every poll
//! reads `install.json` and resolves its command (no process), and the program is asked again only when that resolves
//! to another file (an install, an update or a rollback switches `R/current` to another release), after each action,
//! and, while unknown, at most every [`FALLBACK_EVERY`]. No `install.json`, no version, at once.
//!
//! **The install** ([`LocalHost::begin_install`], [`Installing::run`]): one at a time ([`install::BUSY`] otherwise).
//! While it runs the state is `installing`, whatever the service passes through. Once the package's installer has
//! answered, the app pairs with the new core as on any core launch — again, as [`LocalHost::reconnect`] does, if its
//! pairing was refused: the person asked for this computer — and the install answers the state once the core is
//! reachable, or after [`install::Options::settle_time`] with whatever it is then.
//!
//! **The updates** ([`LocalHost::keep_current`], [`LocalHost::update`], [`LocalHost::version`]; the logic in [`update`]): the
//! app pins one minor of the service (`Config::pinned_minor`); a service behind the newest patch of it is installed
//! again by the install above, in the background, never while a call is on and never for a service of a newer minor.
//! The install the person asks for installs that minor's newest patch too.

use crate::cli::Cli;
use crate::connector;
use crate::core_socket::{CoreError, CoreSocket, Health, Paired};
use crate::endpoints;
use crate::identity;
use crate::install::{self, Failure, Gate, Job, Options, Step};
use crate::npm::{self, Fetch, Minor};
use crate::paths::DataDirs;
use crate::proxy::Proxy;
use crate::state::{self, Link, Observed, Report, State};
use crate::store::{self, Pairing};
use crate::update::{self, Facts, Update, Updates};
use crate::Refusal;
use secrecy::ExposeSecret;
use serde_json::{json, Value};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const POLL_EVERY: Duration = Duration::from_secs(2);
pub const FALLBACK_EVERY: Duration = Duration::from_secs(10);

/// The app's own update of the service is an install under this id (it is the install's one slot).
const UPDATE_JOB: &str = "service-update";

/// The connector's subcommand that turns start at sign-in on or off, and only that: `service <LOGIN_VERB> on|off --json`
/// (a running service keeps running, a stopped one stays stopped). The one place its name is written.
///
/// Depends on the connector's login-only toggle (in progress); until the installed connector knows it, the call is
/// refused with [`LOGIN_UNSUPPORTED`].
pub const LOGIN_VERB: &str = "login";

/// What a start-at-sign-in action is refused with when the connector does not know [`LOGIN_VERB`] (its command line
/// does not parse: `connector.usage`).
pub const LOGIN_UNSUPPORTED: &str = "service.login-unsupported";

/// What the bridge's actions run (SEAMS §3), and how long each may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    ServiceInstall,
    ServiceUninstall,
    /// Start at sign-in on, without starting the service now.
    LoginOn,
    /// Start at sign-in off, without stopping the service now.
    LoginOff,
}

impl Action {
    pub fn parse(name: &str) -> Option<Action> {
        Some(match name {
            "start" => Action::Start,
            "stop" => Action::Stop,
            "restart" => Action::Restart,
            "service-install" => Action::ServiceInstall,
            "service-uninstall" => Action::ServiceUninstall,
            "start-at-login-on" => Action::LoginOn,
            "start-at-login-off" => Action::LoginOff,
            _ => return None,
        })
    }

    pub fn argv(self) -> Vec<&'static str> {
        match self {
            Action::Start => vec!["service", "start", "--json"],
            Action::Stop => vec!["service", "stop", "--json"],
            Action::Restart => vec!["service", "restart", "--json"],
            Action::ServiceInstall => vec!["service", "install", "--json"],
            Action::ServiceUninstall => vec!["service", "uninstall", "--json"],
            Action::LoginOn => vec!["service", LOGIN_VERB, "on", "--json"],
            Action::LoginOff => vec!["service", LOGIN_VERB, "off", "--json"],
        }
    }

    /// Start and restart wait for the manager; stop and uninstall wait for the jobs to exit (≤ 15 s), then kill. The
    /// start-at-sign-in toggle only changes the manager's record, but, as every service command, may wait up to 30 s
    /// for the install lock first.
    pub fn timeout(self) -> Duration {
        match self {
            Action::Start | Action::Restart => Duration::from_secs(30),
            Action::Stop | Action::ServiceInstall | Action::ServiceUninstall | Action::LoginOn | Action::LoginOff => {
                Duration::from_secs(45)
            }
        }
    }

    /// The CLI's refusal as the page gets it: for start at sign-in, a connector whose command line does not take
    /// [`LOGIN_VERB`] (`connector.usage`) is [`LOGIN_UNSUPPORTED`]; anything else as it is.
    pub fn refusal(self, refusal: Refusal) -> Refusal {
        match self {
            Action::LoginOn | Action::LoginOff if refusal.key == "connector.usage" => Refusal::new(
                LOGIN_UNSUPPORTED,
                format!(
                    "The installed Sidevoice cannot turn start at sign-in on or off on its own: {}",
                    refusal.message
                ),
            ),
            _ => refusal,
        }
    }
}

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const PAIR_DEVICE_TIMEOUT: Duration = Duration::from_secs(30);

/// Where the app keeps its side, and what it calls itself.
#[derive(Debug, Clone)]
pub struct Config {
    pub dirs: DataDirs,
    /// The app's config directory: `local-host.json` goes here.
    pub app_dir: PathBuf,
    /// The device name the core lists this app under (the computer's name).
    pub name: String,
    /// The service minor the bundled page pins (`service.pin.json`); `None` when it declares none: nothing is then
    /// compared, and the service is never updated by the app on its own (docs/LOCAL_HOST.md → Updates).
    pub pinned_minor: Option<Minor>,
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
    /// The installed version last asked, the core launch it was asked under and when.
    product: Option<Product>,
    /// `reconnect()`: pair anew on the next pass, whatever is stored.
    force_pair: bool,
    /// The registry's newest patch of the pinned minor, and how the app's own updates went.
    updates: Updates,
    /// Whether a core answered at the last pass: with none, there is no call to count.
    core_up: bool,
}

pub struct LocalHost {
    config: Config,
    core: CoreSocket,
    proxy: Proxy,
    inner: Mutex<Inner>,
    /// One pass at a time: the poll thread's and an action's.
    passes: Mutex<()>,
    /// The install under way, if any: one at a time.
    install: Mutex<Option<Arc<Job>>>,
    /// How many service actions of the person's are under way (`act`): no update of the app's own begins, or goes on to
    /// its installer, while there is one.
    acting: AtomicUsize,
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
                product: None,
                force_pair: false,
                updates: Updates::default(),
                core_up: false,
            }),
            passes: Mutex::new(()),
            install: Mutex::new(None),
            acting: AtomicUsize::new(0),
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

    /// Stops the poll and the proxy; every tunnel closes. An install or an update already running is not stopped: it
    /// ends on its own (its installer rolls back what it did not finish verifying), as it would with the app killed.
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
            "host": pairing.host,
            "local": true,
        }))
    }

    /// One pass: observe, keep the pairing, decide the state.
    pub fn poll(&self) -> Report {
        let pass = self.passes.lock().unwrap();
        self.pass(&pass)
    }

    /// After something changed what `service status --json` or the installed version would say, both cached answers
    /// are dropped under the pass lock: a poll already under way finishes first, so what it read before is never
    /// stored after the drop. The guard is returned for a fresh pass under the same lock.
    fn invalidate(&self) -> std::sync::MutexGuard<'_, ()> {
        let pass = self.passes.lock().unwrap();
        let mut inner = self.inner.lock().unwrap();
        inner.fallback = None;
        inner.product = None;
        drop(inner);
        pass
    }

    /// The pass itself; the caller holds `passes` (the guard proves it).
    fn pass(&self, _pass: &std::sync::MutexGuard<'_, ()>) -> Report {
        // Any connector's `node.status` is the derived service status (SEAMS rev. 2 §4); the CLI's only when none answers.
        let status = connector::node_status(&self.config.dirs).or_else(|| self.fallback_status());
        let health = self.core.health();
        if let Ok(health) = &health {
            self.keep_link(health);
        } else {
            // No core to carry requests to; a core that comes back is checked again before the token goes out.
            self.proxy.set_token(None);
        }
        // The port the core that answered bound, from its record, if the record is that launch's: asked only while one
        // answers, and before `inner` is held.
        let core_port = health
            .as_ref()
            .ok()
            .and_then(|health| endpoints::core_port(&self.config.dirs, health.launch_id.as_deref()));
        let product_version = self.product_version();
        // Only the person's install says `installing`: the app's own update leaves the service's state as it is.
        let installing = self.install.lock().unwrap().as_ref().is_some_and(|job| !job.is_background());
        let mut inner = self.inner.lock().unwrap();
        let mut report = state::report(Observed {
            status: status.as_ref(),
            core: health.as_ref(),
            link: &inner.link,
            product_version: product_version.as_deref(),
            core_port,
        });
        if installing {
            // What the service passes through meanwhile is the install's, not a condition to act on.
            report.state = State::Installing;
            report.failure = None;
        }
        if report.state != inner.report.state {
            (self.log)(&format!("local-host state {}", json!(report.state).as_str().unwrap_or("?")));
        }
        inner.report = report.clone();
        inner.core_up = health.is_ok();
        report
    }

    /// The installed Sidevoice's version: what `install.json`'s command answers to `--version --json`, kept while that
    /// command resolves to the same program; unknown is asked again after [`FALLBACK_EVERY`]. No install, no version.
    fn product_version(&self) -> Option<String> {
        // Reading the record and resolving its command starts nothing; only a program not asked yet is run.
        let cli = Cli::installed(&self.config.dirs).ok();
        let program = cli.as_ref().map(|cli| cli.prefix.clone());
        if let Some(known) = &self.inner.lock().unwrap().product {
            if known.program == program && (known.version.is_some() || known.at.elapsed() < FALLBACK_EVERY) {
                return known.version.clone();
            }
        }
        let version = cli
            .and_then(|cli| cli.run(&["--version", "--json"], VERSION_TIMEOUT).ok())
            .and_then(|answer| answer.get("version").and_then(Value::as_str).filter(|v| semver(v)).map(str::to_string));
        self.inner.lock().unwrap().product = Some(Product { program, at: Instant::now(), version: version.clone() });
        version
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
        #[allow(clippy::disallowed_methods, reason = "handed to the proxy, which sends it to the core")]
        let token =
            inner.pairing.as_ref().filter(|_| inner.link == Link::Paired).map(|p| p.token.expose_secret().to_owned());
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
        #[allow(clippy::disallowed_methods, reason = "sent to the core to check it")]
        let accepted = self.core.accepts(stored.token.expose_secret());
        match accepted {
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
        #[allow(clippy::disallowed_methods, reason = "sent to the core to check it")]
        let token = self.inner.lock().unwrap().pairing.as_ref()?.token.expose_secret().to_owned();
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
            (self.log)(&format!("local-host could not store its pairing: {:?}", e.kind()));
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

    /// Runs a service action, then a fresh pass (the CLI's view and the installed version refreshed too): the state
    /// after it.
    pub fn act(&self, action: Action) -> Result<Report, Refusal> {
        // Held until the state after the action is read: while the connector is still stopping, removing or restarting
        // the service, the state an update's checks read is not yet the person's, and an update that began now would
        // restart what they just stopped.
        let _acting = Acting::begin(&self.acting);
        self.make_way()?;
        let result = self.cli()?.run(&action.argv(), action.timeout());
        let pass = self.invalidate();
        result.map_err(|refusal| action.refusal(refusal))?;
        Ok(self.pass(&pass))
    }

    /// `pair-device --json` (only on the person's click): `{code, expires_in, reach}`.
    pub fn pairing_code(&self) -> Result<Value, Refusal> {
        let answer = self.cli()?.run(&["pair-device", "--json"], PAIR_DEVICE_TIMEOUT)?;
        Ok(json!({ "code": answer["code"], "expires_in": answer["expires_in"], "reach": answer["reach"] }))
    }

    /// The log «Ver registro» shows.
    pub fn log_path(&self) -> Option<PathBuf> {
        self.config.dirs.log()
    }

    /// Starts install `id` (the page's id for this call: 1–128 printable ASCII characters), refused [`install::BUSY`]
    /// while another runs. Its progress and its cancel go by that id from now on; [`Installing::run`] runs it.
    pub fn begin_install(self: &Arc<Self>, id: &str) -> Result<Installing, Failure> {
        if id.is_empty() || id.len() > 128 || !id.chars().all(|c| c.is_ascii_graphic()) {
            return Err(Failure::new("bad_request", None, "That is not an install's id."));
        }
        self.begin(Job::new(id))
    }

    /// Takes the install's slot for `job`: one install or update at a time. The person's install says `installing` at
    /// once, not at the next pass; the app's own update leaves the state as it is.
    fn begin(self: &Arc<Self>, job: Job) -> Result<Installing, Failure> {
        let mut slot = self.install.lock().unwrap();
        if slot.is_some() {
            return Err(Failure::new(install::BUSY, None, "Sidevoice is already being installed on this computer."));
        }
        // Checked under the slot, as `make_way` looks at the slot after raising `acting`: one sees the other.
        if job.is_background() && self.acting.load(Ordering::SeqCst) > 0 {
            return Err(Failure::new(install::BUSY, None, "A service action is under way."));
        }
        let job = Arc::new(job);
        *slot = Some(job.clone());
        drop(slot);
        if !job.is_background() {
            let mut inner = self.inner.lock().unwrap();
            inner.report.state = State::Installing;
            inner.report.failure = None;
        }
        (self.log)(&format!("local-host install {} started", install::loggable_job(job.id())));
        Ok(Installing { host: self.clone(), job })
    }

    /// Install `id`'s `{step, done, total, cancellable}` while it runs; `None` for any other id, or once it has ended.
    pub fn install_progress(&self, id: &str) -> Option<install::Progress> {
        self.install.lock().unwrap().as_ref().filter(|job| job.id() == id).map(|job| job.progress())
    }

    /// Asks install `id` to stop: true while that is honoured (until its installer starts), false after, or for an id
    /// not under way.
    pub fn cancel_install(&self, id: &str) -> bool {
        let job = self.install.lock().unwrap().as_ref().filter(|job| job.id() == id).cloned();
        job.is_some_and(|job| job.cancel())
    }

    fn end_install(&self, job: &Arc<Job>) {
        let mut slot = self.install.lock().unwrap();
        if slot.as_ref().is_some_and(|running| Arc::ptr_eq(running, job)) {
            *slot = None;
        }
    }

    /// The service minor the bundled page pins, if it declares one.
    pub fn pinned_minor(&self) -> Option<Minor> {
        self.config.pinned_minor
    }

    /// What `localHost.version()` says of the service against the pinned minor (the page's bridge adds its own
    /// `bridge` number): `{bundled, installed, core_api, update, latest, progress}` (docs/LOCAL_HOST.md → Updates).
    pub fn version(&self) -> Value {
        let working = self.install.lock().unwrap().as_ref().map(|job| update::progress(&job.progress()));
        let inner = self.inner.lock().unwrap();
        json!({
            "bundled": self.config.pinned_minor.map_or_else(|| json!({}), |minor| json!({ "minor": minor.to_string() })),
            "installed": inner.report.product_version.as_ref().map(|version| json!({ "version": version })),
            "core_api": inner.report.core.as_ref().and_then(|core| core.get("api")).and_then(Value::as_i64),
            "update": self.update_state(&inner, working.is_some()),
            "latest": inner.updates.latest,
            "progress": working,
        })
    }

    /// How many calls are on, as the last pass saw: `None` is not known — a core answers and does not say — and is
    /// never taken for none. With no core answering there is no call to count.
    fn calls_of(inner: &Inner) -> Option<u64> {
        match inner.report.calls {
            Some(calls) => Some(calls),
            None if inner.core_up => None,
            None => Some(0),
        }
    }

    fn update_state(&self, inner: &Inner, working: bool) -> Update {
        update::state(Facts {
            pinned: self.config.pinned_minor,
            installed: inner.report.product_version.as_deref(),
            latest: inner.updates.latest.as_deref(),
            incompatible: inner.report.state == State::Incompatible,
            calls: Self::calls_of(inner),
            working,
        })
    }

    /// Asks the registry for the newest patch of the pinned minor when that is due, or `force`d. No pin, nothing to ask.
    fn refresh_latest(&self, fetch: &dyn Fetch, force: bool) {
        let Some(pinned) = self.config.pinned_minor else { return };
        if !force && !self.inner.lock().unwrap().updates.check_due(Instant::now()) {
            return;
        }
        let answer = npm::latest_in_minor(fetch, pinned);
        if let Err(failure) = &answer {
            (self.log)(&format!("local-host service check failed: {}", failure.log_report()));
        }
        self.inner.lock().unwrap().updates.checked(Instant::now(), answer.ok());
    }

    /// Whether the app is to update the service by itself now: it is behind the pinned minor's newest patch the registry
    /// named, nothing is installing, no call is on (and the count is known), the person has not stopped it (the
    /// installer would start it), and an earlier try at this release did not just fail.
    fn update_due(&self) -> bool {
        if self.acting.load(Ordering::SeqCst) > 0 {
            return false;
        }
        let working = self.install.lock().unwrap().is_some();
        let inner = self.inner.lock().unwrap();
        self.update_state(&inner, working) == Update::Available
            && inner.updates.latest.is_some()
            && inner.report.state != State::StoppedByPerson
            && !inner.updates.holding_off(Instant::now())
    }

    /// The last check before an update's installer restarts the service, on a fresh pass (the CLI's view and the installed
    /// version asked again, not the last poll's answer: a call may have begun, or the person stopped or removed the
    /// service, during the download): `None` to go on, or why not — a call is on or the count is not known
    /// ([`install::WAITING_CALL`]), the person stopped it ([`install::STOPPED`]), it is no longer installed
    /// ([`install::GONE`]). Nothing is changed by an update stopped here, and it is not a failure.
    pub fn update_hold(&self) -> Option<Failure> {
        if self.acting.load(Ordering::SeqCst) > 0 {
            return Some(Failure::new(install::BUSY, Step::Staging, "A service action is under way."));
        }
        let report = {
            let pass = self.invalidate();
            self.pass(&pass)
        };
        let held = |key: &str, message: &str| Some(Failure::new(key, Step::Staging, message));
        if report.state == State::StoppedByPerson {
            return held(install::STOPPED, "The service was stopped; the update leaves it so.");
        }
        if report.product_version.is_none() {
            return held(install::GONE, "Sidevoice is no longer installed; there is nothing to update.");
        }
        if Self::calls_of(&self.inner.lock().unwrap()) != Some(0) {
            return held(install::WAITING_CALL, "A call is on, or may be; the update waits for it to end.");
        }
        None
    }

    /// Keeps the service current, on the caller's thread until [`Self::shutdown`]: the registry asked at launch and every
    /// [`update::CHECK_EVERY`], and a service behind the pinned minor's newest patch updated in the background, as the
    /// install does it (same download, verification and installer, with the installer's rollback), once no call is on.
    /// A service of a newer minor than the pin is never touched; with no pin nothing is asked and nothing is updated.
    /// `admit` is asked right before each update, and what it returns is held until the update ends; `None` skips it
    /// for now (the app uses it to keep a data reset out). `base` is how the install goes ([`Options::standard`]).
    pub fn keep_current<G>(self: &Arc<Self>, fetch: &dyn Fetch, base: &Options, admit: impl Fn() -> Option<G>) {
        while !self.stopped.load(Ordering::SeqCst) {
            self.refresh_latest(fetch, false);
            if self.update_due() {
                if let Some(_admitted) = admit() {
                    if let Err(failed) = self.run_update(fetch, base) {
                        if failed.is_failure() {
                            (self.log)(&format!("local-host service update failed: {}", failed.failure.log_report()));
                        }
                    }
                }
            }
            std::thread::sleep(POLL_EVERY);
        }
    }

    /// The update as a background install of the pinned minor's newest patch, with the last check
    /// ([`Self::update_hold`]) right before its installer restarts the service. The state after it, or why it did not
    /// happen. A failure (not a hold, not a busy slot) is remembered, so that the same release is not tried again at
    /// once ([`Failed::hold_off`]), by whoever asked.
    fn run_update(self: &Arc<Self>, fetch: &dyn Fetch, base: &Options) -> Result<Report, Failed> {
        let installing = self.begin(Job::background(UPDATE_JOB)).map_err(Failed::before_installer)?;
        let job = installing.job.clone();
        let host = self.clone();
        let options = Options { gate: Some(Gate::new(move || host.update_hold())), ..base.clone() };
        let outcome = installing
            .run(fetch, &options)
            .map_err(|failure| Failed { failure, installer_ran: !job.progress().cancellable })
            .and_then(|report| {
                // The installer said it did what it was asked: the service must say so too, or it would be asked again
                // at every pass for ever.
                let behind =
                    self.config.pinned_minor.zip(report.product_version.as_deref()).is_some_and(|(pinned, v)| {
                        update::is_behind(v, pinned, self.inner.lock().unwrap().updates.latest.as_deref())
                    });
                if behind {
                    let failure = Failure::new(
                        "update.unchanged",
                        None,
                        "The installer finished, but the installed Sidevoice is still the older version.",
                    );
                    return Err(Failed { failure, installer_ran: true });
                }
                Ok(report)
            });
        if let Err(failed) = &outcome {
            if let Some(hold) = failed.hold_off() {
                self.inner.lock().unwrap().updates.update_failed(Instant::now(), hold);
            }
        }
        outcome
    }

    /// Whether an install or an update holds the slot.
    fn installing(&self) -> bool {
        self.install.lock().unwrap().is_some()
    }

    /// Waits until the slot is free and no service action of the person's is under way (or the host stops).
    fn wait_for_slot(&self) {
        while (self.installing() || self.acting.load(Ordering::SeqCst) > 0) && !self.stopped.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// What the person does to the service (a service action) comes before the app's own update: that is cancelled while
    /// that is honoured — before its installer, nothing changed — and the action goes on at once: a cancelled update can
    /// no longer reach its installer (the cancel and the step that starts it are one atomic choice, [`Job::cancel`] and
    /// its `commit`), wherever it is in its download or unpacking, and ends on its own. Once its installer runs (it
    /// restarts the service: a stop or an uninstall would be undone by it) the action is refused as an install is
    /// ([`install::BUSY`]).
    fn make_way(&self) -> Result<(), Refusal> {
        let Some(job) = self.install.lock().unwrap().clone().filter(|job| job.is_background()) else { return Ok(()) };
        if job.cancel() {
            Ok(())
        } else {
            Err(Refusal::new(install::BUSY, "Sidevoice is being updated; try again when that is done."))
        }
    }

    /// `localHost.update()`: the registry asked now, and, if the service is behind, the update run to its end as the app's
    /// own would, and the state after it. Whatever else holds the install's slot (the app's own update, another call of
    /// this, the person's install) is waited for first and the state read again, never refused as busy. While a call is on
    /// (or the count is not known) it changes nothing and says so (`version()`'s `update` is `waiting-call`: it runs once
    /// the call ends). Refused `update.unknown` with no pin or no installed version, `update.newer-installed` for a newer
    /// minor than the pin, [`install::STOPPED`] for a service the person stopped, [`install::GONE`] for one removed
    /// meanwhile, and `update.unchanged` when the installer said it was done and the version did not change.
    pub fn update(self: &Arc<Self>, fetch: &dyn Fetch, base: &Options) -> Result<Report, Failure> {
        if self.config.pinned_minor.is_none() {
            return Err(Failure::new(
                "update.unknown",
                None,
                "This app does not say which Sidevoice version it works with.",
            ));
        }
        self.refresh_latest(fetch, true);
        loop {
            self.wait_for_slot();
            let working = self.installing();
            if working && self.stopped.load(Ordering::SeqCst) {
                return Ok(self.state());
            }
            let (state, person_stopped) = {
                let inner = self.inner.lock().unwrap();
                (self.update_state(&inner, working), inner.report.state == State::StoppedByPerson)
            };
            match state {
                Update::Unknown => {
                    return Err(Failure::new("update.unknown", None, "No Sidevoice version is installed to update."))
                }
                Update::NewerInstalled => {
                    return Err(Failure::new(
                        "update.newer-installed",
                        None,
                        "The installed Sidevoice is newer than the version this app works with; it is left as it is.",
                    ))
                }
                // Something took the slot since it was free: wait for it and look again.
                Update::Updating => continue,
                // Nothing is downloaded for a service the person stopped: the installer would start it.
                Update::Available if person_stopped => {
                    return Err(Failure::new(
                        install::STOPPED,
                        None,
                        "The service was stopped; the update leaves it so.",
                    ))
                }
                Update::Available => match self.run_update(fetch, base) {
                    Err(failed) if failed.failure.key == install::BUSY && !self.stopped.load(Ordering::SeqCst) => {
                        continue
                    }
                    Err(failed) if failed.failure.key == install::WAITING_CALL => return Ok(self.state()),
                    done => return done.map_err(|failed| failed.failure),
                },
                Update::Current | Update::WaitingCall | Update::Incompatible => return Ok(self.state()),
            }
        }
    }

    /// The minor an install is to take: the pinned one, or, when what is installed is of a newer minor, that one — never
    /// a downgrade, and never another minor than one of the two (the newest patch of the installed minor; none in the
    /// registry is `install.no-bundle`, not a move to the latest). A version that cannot be read is no reason to
    /// leave the pin. No pin, none (the latest, as before the pin).
    fn minor_for_install(&self) -> Option<Minor> {
        let pinned = self.config.pinned_minor?;
        let installed = self.inner.lock().unwrap().report.product_version.as_deref().and_then(Minor::of);
        Some(installed.filter(|installed| *installed > pinned).unwrap_or(pinned))
    }

    /// The install's steps 1–4 ([`install::prepare`]), then `pairing`: the state after it, the install's slot freed
    /// before the last pass so that it says what is there.
    fn run_install(&self, job: &Arc<Job>, fetch: &dyn Fetch, options: &Options) -> Result<Report, Failure> {
        // The app works with one minor of the service: what it installs, for the person or by itself, is that minor's
        // newest patch.
        let options = Options { minor: options.minor.or(self.minor_for_install()), ..options.clone() };
        let outcome = install::prepare(job, fetch, &self.config.dirs.data, &options);
        // A failure is logged in full, at error level, by the app (`run_install` in `src/local_host.rs`).
        if let Ok(answer) = &outcome {
            (self.log)(&format!(
                "local-host install {}: {} {}",
                install::loggable_job(job.id()),
                answer["action"].as_str().unwrap_or("done"),
                answer["version"].as_str().unwrap_or("?")
            ));
            self.settle(job, options.settle_time);
        }
        self.end_install(job);
        let pass = self.invalidate();
        let report = self.pass(&pass);
        outcome.map(|_| report)
    }

    /// `pairing`: the new core paired with as on any core launch (again if the app's pairing was refused, as
    /// [`Self::reconnect`] does: the person asked for this computer; never so for the app's own update, which pairs
    /// again nothing that was refused), until it is reachable or `limit` has passed.
    fn settle(&self, job: &Job, limit: Duration) {
        let _ = job.enter(Step::Pairing);
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.link == Link::Refused && !job.is_background() {
                inner.force_pair = true;
                inner.link = Link::Unpaired;
                inner.pairing = None;
            }
        }
        drop(self.invalidate());
        let deadline = Instant::now() + limit;
        while !self.poll().reachable && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250).min(limit));
        }
    }
}

/// A service action of the person's under way: counted while this lives.
struct Acting<'a>(&'a AtomicUsize);

impl<'a> Acting<'a> {
    fn begin(count: &'a AtomicUsize) -> Acting<'a> {
        count.fetch_add(1, Ordering::SeqCst);
        Acting(count)
    }
}

impl Drop for Acting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An update that did not happen, and whether its installer had started (it restarts the service: it changed things).
struct Failed {
    failure: Failure,
    installer_ran: bool,
}

impl Failed {
    /// One that stopped before anything started.
    fn before_installer(failure: Failure) -> Failed {
        Failed { failure, installer_ran: false }
    }

    /// Whether it is a failure, and not the update being held (a call, a stop, a removal) or the slot being another
    /// install's, which are no fault of the release.
    fn is_failure(&self) -> bool {
        !install::is_held(&self.failure.key) && self.failure.key != install::BUSY
    }

    /// How long the same release is left alone after it: none when the slot is another install's or a service action is
    /// under way (the state is read again at the next pass), [`update::RETRY_GATE`] when the last check held it (a call, a
    /// stop, a removal: so that a cached poll that disagrees with the fresh pass does not download it again every
    /// pass), [`update::RETRY_SOON`] for a failure before the installer — the registry or the download, a cancel — that
    /// changed nothing here, and [`update::RETRY_AFTER`] for anything else, since each try restarts the service.
    fn hold_off(&self) -> Option<Duration> {
        let key = self.failure.key.as_str();
        if key == install::BUSY {
            None
        } else if install::is_held(key) {
            Some(update::RETRY_GATE)
        } else if !self.installer_ran && matches!(key, "install.network" | "install.no-bundle" | install::CANCELLED) {
            Some(update::RETRY_SOON)
        } else {
            Some(update::RETRY_AFTER)
        }
    }
}

/// An install begun ([`LocalHost::begin_install`]); dropped unrun, it frees the slot.
pub struct Installing {
    host: Arc<LocalHost>,
    job: Arc<Job>,
}

impl Installing {
    pub fn id(&self) -> &str {
        self.job.id()
    }

    /// Runs it to the end (blocking: minutes at worst): the state after it, or why it failed.
    pub fn run(self, fetch: &dyn Fetch, options: &install::Options) -> Result<Report, Failure> {
        self.host.run_install(&self.job, fetch, options)
    }
}

impl Drop for Installing {
    fn drop(&mut self) {
        self.host.end_install(&self.job);
    }
}

impl Drop for LocalHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The installed version as last asked.
struct Product {
    /// The program asked (`install.json`'s command, resolved); `None` with no installation.
    program: Option<Vec<String>>,
    at: Instant,
    version: Option<String>,
}

/// A version as a release names it, and as the page accepts it (sidevoice-web `local-host-status.ts`):
/// `X.Y.Z[-pre][+build]`, the suffixes of ASCII letters, digits, `.` and `-`; at most 64 characters.
pub(crate) fn semver(text: &str) -> bool {
    let suffix = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    let (rest, build) = match text.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (text, None),
    };
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };
    let numbers: Vec<&str> = core.split('.').collect();
    text.len() <= 64
        && numbers.len() == 3
        && numbers.iter().all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        && pre.is_none_or(suffix)
        && build.is_none_or(suffix)
}

fn mismatch() -> Refusal {
    Refusal::new("identity.mismatch", "The core on this computer did not prove the identity the app paired with.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_at_sign_in_is_its_own_action_on_the_login_verb() {
        assert_eq!(Action::parse("start-at-login-on"), Some(Action::LoginOn));
        assert_eq!(Action::parse("start-at-login-off"), Some(Action::LoginOff));
        assert_eq!(Action::parse("start-at-login"), None);
        assert_eq!(Action::LoginOn.argv(), ["service", LOGIN_VERB, "on", "--json"]);
        assert_eq!(Action::LoginOff.argv(), ["service", LOGIN_VERB, "off", "--json"]);
        assert_eq!(LOGIN_VERB, "login");
        assert_eq!(Action::LoginOn.timeout(), Duration::from_secs(45));
        assert_eq!(Action::LoginOff.timeout(), Duration::from_secs(45));
    }

    #[test]
    fn the_other_actions_keep_their_verbs() {
        for (name, verb) in [
            ("start", "start"),
            ("stop", "stop"),
            ("restart", "restart"),
            ("service-install", "install"),
            ("service-uninstall", "uninstall"),
        ] {
            assert_eq!(Action::parse(name).unwrap().argv(), ["service", verb, "--json"], "{name}");
        }
        assert_eq!(Action::parse("login"), None);
    }

    #[test]
    fn a_connector_without_the_login_verb_is_a_keyed_refusal() {
        let usage = || Refusal::new("connector.usage", "unrecognized subcommand");
        for action in [Action::LoginOn, Action::LoginOff] {
            let refused = action.refusal(usage());
            assert_eq!(refused.key, LOGIN_UNSUPPORTED);
            assert!(refused.message.contains("unrecognized subcommand"), "{}", refused.message);
            let other = Refusal::new("service.busy", "m");
            assert_eq!(action.refusal(other.clone()), other, "any other refusal as it is");
        }
        assert_eq!(Action::Start.refusal(usage()), usage(), "only start at sign-in is renamed");
    }

    #[test]
    fn what_is_not_a_failure_is_not_held_off_and_the_rest_by_how_much_it_changed() {
        let failed = |key: &str, installer_ran| Failed { failure: Failure::new(key, None, "m"), installer_ran };
        // Another install's slot, or an action of the person's under way: nothing to hold, the state is read again.
        assert_eq!(failed(install::BUSY, false).hold_off(), None);
        assert!(!failed(install::BUSY, false).is_failure());
        // What the last check held is no failure, and is left alone only a little: it must run soon after a call ends.
        for held in [install::WAITING_CALL, install::STOPPED, install::GONE] {
            let held = failed(held, false);
            assert!(!held.is_failure(), "{}", held.failure.key);
            assert_eq!(held.hold_off(), Some(update::RETRY_GATE), "{}", held.failure.key);
        }
        assert!(failed("install.checksum", false).is_failure());
        // Before the installer ran, nothing changed here: the registry, the download, a cancel are tried again soon.
        for soon in ["install.network", "install.no-bundle", install::CANCELLED] {
            assert_eq!(failed(soon, false).hold_off(), Some(update::RETRY_SOON), "{soon}");
        }
        // A tarball that is not the registry's, an unsafe directory, the installer's own failures: much later.
        for later in
            ["install.checksum", "install.authenticity", "install.unsafe", "install.failed", "install.rollback"]
        {
            assert_eq!(failed(later, false).hold_off(), Some(update::RETRY_AFTER), "{later}");
        }
        // The installer ran: it restarted the service, whatever the key says.
        for key in ["install.network", install::CANCELLED, "service.start.failed", "update.unchanged"] {
            assert_eq!(failed(key, true).hold_off(), Some(update::RETRY_AFTER), "{key}");
        }
        assert!(update::RETRY_SOON < update::RETRY_AFTER);
    }

    #[test]
    fn a_version_is_what_the_page_accepts() {
        for good in ["0.8.1", "10.20.30", "0.9.0-rc.1", "0.9.0-rc.1+abc", "0.9.0+0123abcdef01", "1.2.3-a-b.c+d-e.f"] {
            assert!(semver(good), "{good}");
        }
        let long = format!("1.2.3-{}", "a".repeat(60));
        for bad in [
            "",
            " ",
            "unknown",
            "0.8",
            "0.8.1.2",
            "v0.8.1",
            "0.8.1_dirty",
            "0.8.1 ",
            " 0.8.1",
            "0.8.1-",
            "0.8.1+",
            "0.8.1-rc 1",
            "0.8.1+a+b",
            "0.8.1-ré",
            "０.8.1",
            "0..1",
            long.as_str(),
            "0.8.1\n",
        ] {
            assert!(!semver(bad), "{bad:?}");
        }
    }
}
