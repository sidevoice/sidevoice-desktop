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

use crate::cli::Cli;
use crate::connector;
use crate::core_socket::{CoreError, CoreSocket, Health, Paired};
use crate::identity;
use crate::install::{self, Failure, Job, Step};
use crate::npm::Fetch;
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
            }),
            passes: Mutex::new(()),
            install: Mutex::new(None),
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
        let product_version = self.product_version();
        let installing = self.install.lock().unwrap().is_some();
        let mut inner = self.inner.lock().unwrap();
        let mut report = state::report(Observed {
            status: status.as_ref(),
            core: health.as_ref(),
            link: &inner.link,
            product_version: product_version.as_deref(),
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

    /// Runs a service action, then a fresh pass (the CLI's view and the installed version refreshed too): the state
    /// after it.
    pub fn act(&self, action: Action) -> Result<Report, Refusal> {
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
        let mut slot = self.install.lock().unwrap();
        if slot.is_some() {
            return Err(Failure::new(install::BUSY, None, "Sidevoice is already being installed on this computer."));
        }
        let job = Arc::new(Job::new(id));
        *slot = Some(job.clone());
        drop(slot);
        // Said at once, not at the next pass.
        let mut inner = self.inner.lock().unwrap();
        inner.report.state = State::Installing;
        inner.report.failure = None;
        drop(inner);
        (self.log)(&format!("local-host install {id} started"));
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

    /// The install's steps 1–4 ([`install::prepare`]), then `pairing`: the state after it, the install's slot freed
    /// before the last pass so that it says what is there.
    fn run_install(&self, job: &Arc<Job>, fetch: &dyn Fetch, options: &install::Options) -> Result<Report, Failure> {
        let outcome = install::prepare(job, fetch, &self.config.dirs.data, options);
        match &outcome {
            Ok(answer) => {
                (self.log)(&format!(
                    "local-host install {}: {} {}",
                    job.id(),
                    answer["action"].as_str().unwrap_or("done"),
                    answer["version"].as_str().unwrap_or("?")
                ));
                self.settle(job, options.settle_time);
            }
            Err(failure) => (self.log)(&format!("local-host install {} failed: {failure}", job.id())),
        }
        self.end_install(job);
        let pass = self.invalidate();
        let report = self.pass(&pass);
        outcome.map(|_| report)
    }

    /// `pairing`: the new core paired with as on any core launch (again if the app's pairing was refused, as
    /// [`Self::reconnect`] does: the person asked for this computer), until it is reachable or `limit` has passed.
    fn settle(&self, job: &Job, limit: Duration) {
        let _ = job.enter(Step::Pairing);
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.link == Link::Refused {
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
