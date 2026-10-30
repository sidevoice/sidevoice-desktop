// S0 spike (sidevoice-desktop#290, architecture-v3.md §F.1): injected into every page of the spike's window, before
// the page's own scripts. On the bundled room page (/voice/) during a run it:
//   - reports through the bridge (`s0_log`, written natively with the app's clock): a 1 s page timer, page lifecycle,
//     the room socket's state and every frame's type, and the page-media counters;
//   - counts AudioContext, getUserMedia, HTMLMediaElement.play and mediaSession.setActionHandler. In run N it also
//     refuses them (page media disabled, page capture denied): the counters must stay at 0;
//   - in run N only, installs the old-contract global `__sidevoiceDesktop` with `mediaKeys: "native"` (no Media Session,
//     no Now Playing loop) and a diagnostic `voice`/`engine` adapter, so the page never selects the browser voice path.
//     The adapter turns the app's synthetic native events into turns; what the room asks it to say is logged natively
//     and reported heard. Everything it makes up is labelled `synthetic`.
// Run P installs no global: the page's own browser voice path, its own capture and its default Media Session.
(() => {
  if (window.__s0) return;
  const internals = () => window.__TAURI_INTERNALS__;
  const invoke = (cmd, args) => {
    try { return internals()?.invoke(cmd, args)?.catch?.(() => {}); } catch { return undefined; }
  };
  const page = location.pathname;
  const mode = (() => { try { return localStorage.getItem("s0.mode"); } catch { return null; } })();
  const counters = { audioContext: 0, getUserMedia: 0, mediaPlay: 0, mediaSessionHandler: 0 };
  const send = (k, data = {}) => invoke("s0_log", { entry: { ...data, k, page, mode, perf: Math.round(performance.now()), wall: Date.now() } });
  const s0 = (window.__s0 = { mode, counters, send, synthetic: () => {} });
  if (!mode || !page.startsWith("/voice")) return;
  const denied = mode === "N";
  send("page-load", { timeOrigin: Math.round(performance.timeOrigin), visibility: document.visibilityState, ua: navigator.userAgent });

  // ----- page media: counted (both runs), refused (run N) -----
  const refusal = (what) => new DOMException(`S0 run N: ${what} is disabled in the page`, "NotAllowedError");
  for (const name of ["AudioContext", "webkitAudioContext"]) {
    const Original = window[name];
    if (typeof Original !== "function") continue;
    window[name] = new Proxy(Original, {
      construct(target, args, newTarget) {
        counters.audioContext++;
        send("page-media", { api: name, denied, counters });
        if (denied) throw refusal(name);
        return Reflect.construct(target, args, newTarget);
      },
    });
  }
  const devices = window.MediaDevices?.prototype;
  if (devices?.getUserMedia) {
    const original = devices.getUserMedia;
    devices.getUserMedia = function (constraints) {
      counters.getUserMedia++;
      send("page-media", { api: "getUserMedia", denied, counters });
      if (denied) return Promise.reject(refusal("getUserMedia"));
      return original.call(this, constraints);
    };
  }
  const media = window.HTMLMediaElement?.prototype;
  if (media?.play) {
    const original = media.play;
    media.play = function () {
      counters.mediaPlay++;
      send("page-media", { api: "HTMLMediaElement.play", denied, counters });
      if (denied) return Promise.reject(refusal("HTMLMediaElement.play"));
      return original.call(this);
    };
  }
  const session = window.MediaSession?.prototype;
  if (session?.setActionHandler) {
    const original = session.setActionHandler;
    session.setActionHandler = function (action, handler) {
      counters.mediaSessionHandler++;
      send("page-media", { api: "mediaSession.setActionHandler", action, denied, counters });
      if (denied) return undefined;
      return original.call(this, action, handler);
    };
  }

  // ----- the presence sound would be page audio: off in run N, the person's own setting back in run P -----
  try {
    const KEY = "sidevoice.call-preferences", SAVED = "s0.presence-saved";
    const stored = JSON.parse(localStorage.getItem(KEY) || "{}") || {};
    if (denied && stored.presence !== "off") {
      localStorage.setItem(SAVED, JSON.stringify(stored.presence ?? null));
      localStorage.setItem(KEY, JSON.stringify({ ...stored, presence: "off" }));
      send("presence-sound", { set: "off", was: stored.presence ?? null });
    } else if (!denied && localStorage.getItem(SAVED) !== null) {
      const was = JSON.parse(localStorage.getItem(SAVED));
      const { presence: _off, ...rest } = stored;
      localStorage.setItem(KEY, JSON.stringify(was ? { ...rest, presence: was } : rest));
      localStorage.removeItem(SAVED);
      send("presence-sound", { restored: was });
    }
  } catch { /* the page's default stands */ }

  // ----- the room socket: state and the type of every frame -----
  const sockets = new Set();
  let socketIds = 0, binaryIn = 0, binaryOut = 0;
  const brief = (d = {}, extra = {}) => {
    const out = {};
    for (const key of ["session_id", "turn_id", "phase", "revision", "utterance_id", "thread_id", "client_msg_id", "status", "key", "reason", "epoch"]) {
      if (d[key] !== undefined && typeof d[key] !== "object") out[key] = d[key];
    }
    return { ...out, ...extra };
  };
  const frame = (direction, id, data) => {
    if (typeof data !== "string") {
      if (direction === "in") binaryIn++; else binaryOut++;
      return;
    }
    let message;
    try { message = JSON.parse(data); } catch { send(`ws-${direction}`, { id, type: "?", bytes: data.length }); return; }
    const type = message?.type, d = message?.data || {};
    const text = typeof d.text === "string" ? { text: d.text.slice(0, 160) } : {};
    send(`ws-${direction}`, { id, type, seq: message?.seq, ...brief(d, text) });
  };
  const NativeSocket = window.WebSocket;
  if (typeof NativeSocket === "function") {
    class S0Socket extends NativeSocket {
      constructor(url, protocols) {
        super(url, protocols);
        const id = ++socketIds;
        this.__s0 = id;
        sockets.add(this);
        let where = "?";
        try { const u = new URL(url, location.href); where = u.origin + u.pathname; } catch { /* unreadable */ }
        send("ws-new", { id, url: where });
        this.addEventListener("open", () => send("ws-open", { id }));
        this.addEventListener("error", () => send("ws-error", { id }));
        this.addEventListener("close", (e) => { sockets.delete(this); send("ws-close", { id, code: e.code, reason: e.reason, clean: e.wasClean }); });
        this.addEventListener("message", (e) => frame("in", id, e.data));
      }
      // Logged once the socket took the frame: an attempt the socket refused is logged as such. Taken is still not
      // received; analyze.mjs counts a heartbeat only when the core's next frame follows (a confirmed exchange).
      send(data) {
        let result;
        try { result = super.send(data); } catch (error) { send("ws-out-failed", { id: this.__s0, error: String(error) }); throw error; }
        frame("out", this.__s0, data);
        return result;
      }
    }
    window.WebSocket = S0Socket;
  }

  // ----- the page's own clock, and its lifecycle -----
  let ticks = 0;
  setInterval(() => {
    ticks++;
    send("tick", {
      n: ticks, visibility: document.visibilityState, focus: document.hasFocus(), online: navigator.onLine,
      sockets: [...sockets].map((s) => s.readyState), binary_in: binaryIn, binary_out: binaryOut,
      ...(ticks % 10 === 0 ? { counters } : {}),
    });
  }, 1000);
  for (const [target, events] of [[document, ["visibilitychange", "freeze", "resume"]], [window, ["pagehide", "pageshow", "online", "offline", "focus", "blur"]]]) {
    for (const name of events) target.addEventListener(name, () => send("lifecycle", { event: name, visibility: document.visibilityState }), { capture: true });
  }

  // ----- the S0 badge: the run's mode and counters, a way back to the launcher -----
  const badge = () => {
    const el = document.createElement("a");
    el.href = "/index.html";
    el.id = "s0-badge";
    el.style.cssText = "position:fixed;z-index:2147483647;top:calc(env(safe-area-inset-top) + 2px);left:50%;transform:translateX(-50%);" +
      "font:600 11px/1.6 -apple-system,system-ui,sans-serif;padding:0 8px;border-radius:9px;background:rgba(0,0,0,.6);color:#fff;text-decoration:none";
    const paint = () => { el.textContent = `S0 ${mode} · media ${Object.values(counters).reduce((a, b) => a + b, 0)} · t${ticks}`; };
    paint();
    setInterval(paint, 1000);
    document.body.appendChild(el);
  };
  if (document.body) badge(); else document.addEventListener("DOMContentLoaded", badge, { once: true });

  if (!denied) return;

  // ----- run N: the diagnostic voice and engine adapter -----
  const listeners = () => {
    const set = new Set();
    return {
      on: (fn) => { if (typeof fn !== "function") throw new TypeError("a listener"); set.add(fn); return () => set.delete(fn); },
      emit: (value) => { for (const fn of [...set]) { try { fn(value); } catch { /* the page's own */ } } },
    };
  };
  let routed = null, calls = 0;
  const deviceListeners = listeners();
  const deviceList = {
    inputs: [{ id: "", label: "S0 native holder" }], outputs: [{ id: "", label: "S0 native holder" }],
    input: null, output: null, outputSelectable: false,
  };
  function start(settings, options) {
    if (!settings || typeof settings !== "object" || (!settings.input && !settings.output)) {
      throw new TypeError("voice settings with an input or an output half");
    }
    const id = `s0-call-${++calls}`;
    const turn = listeners(), state = listeners(), level = listeners(), playback = listeners(), error = listeners(), take = listeners();
    let muted = false, stopped = false, saying = 0;
    let now = { listening: settings.input ? "listening" : "off", recognising: 0, playback: "idle", microphone: "live" };
    const set = (patch) => { now = { ...now, ...patch }; state.emit(now); };
    const ready = Promise.resolve().then(() => {
      set({});
      send("voice-call", { id, input: !!settings.input, output: !!settings.output, take: options?.take === true, synthetic: true });
    });
    const call = {
      id,
      ready,
      update: async () => {},
      async stop() {
        if (stopped) return;
        stopped = true;
        if (routed === call) routed = null;
        set({ listening: "idle", playback: "idle" });
        send("voice-call-stop", { id });
      },
      say(text, sayOptions) {
        const sayId = `${id}-say-${++saying}`;
        const heard = new Set();
        let settle, done = false;
        const outcome = new Promise((resolve) => { settle = resolve; });
        const step = (event) => { for (const fn of [...heard]) { try { fn(event); } catch { /* the page's own */ } } };
        const finish = (result) => {
          if (done) return;
          done = true;
          step({ type: "done", outcome: result });
          settle(result);
          set({ playback: "idle" });
          send("say-done", { id: sayId, status: result.status, reason: result.reason ?? null, synthetic: true });
        };
        send("say", { id: sayId, call: id, text: String(text).slice(0, 200), language: sayOptions?.language ?? null, synthetic: true });
        queueMicrotask(() => {
          if (done) return;
          if (stopped) { finish({ status: "not-played", reason: "stopped" }); return; }
          set({ playback: "playing" });
          step({ type: "playing" });
          queueMicrotask(() => finish({ status: "heard" }));
        });
        return Object.freeze({
          id: sayId,
          cancel: () => finish({ status: "not-played", reason: "cancelled" }),
          onEvent(fn) { if (typeof fn !== "function") throw new TypeError("a listener"); heard.add(fn); },
          outcome,
        });
      },
      mute(value) {
        muted = !!value;
        if (!stopped && settings.input) set({ listening: muted ? "muted" : "listening" });
        send("voice-mute", { id, muted });
      },
      cancelInput() {},
      endInput: async () => {},
      onTurn: turn.on, onState: state.on, onLevel: level.on, onPlaybackLevel: playback.on, onError: error.on, onTake: take.on,
    };
    Object.defineProperty(call, "__s0", { value: { turn, muted: () => muted, stopped: () => stopped, input: !!settings.input } });
    return Object.freeze(call);
  }
  const voice = Object.freeze({
    start,
    preload: async () => {},
    async routeInput(target) { routed = target; send("voice-route", { call: target?.id ?? null }); },
    audioDevices: async () => deviceList,
    setAudioDevices: async () => deviceList,
    onAudioDevices: deviceListeners.on,
  });
  const notHere = () => Promise.reject(Object.assign(new Error("the S0 diagnostic engine has no models"), { code: "s0-diagnostic" }));
  const engine = Object.freeze({
    catalogs: async () => [],
    setCredential: notHere,
    hasCredential: async () => false,
    testCredential: async () => "s0-diagnostic",
    refresh: async () => {},
    capabilities: async () => ({ os: "s0", arch: "s0", accelerators: [] }),
    installed: async () => [],
    install: notHere,
    uninstall: async () => {},
    memory: async () => null,
  });
  window.__sidevoiceDesktop = { host: { mediaKeys: "native", voice, engine } };

  // A synthetic native event (the app's clock, `synthetic-emitted`) made into one turn of the call that has the
  // microphone: started and finished at once, its words saying what it is and the reply it asks for.
  s0.synthetic = ({ seq, t }) => {
    const target = routed;
    send("synthetic-received", { seq, native_t: t, call: target?.id ?? null, synthetic: true });
    if (!target) return send("synthetic-dropped", { seq, reason: "no-call", synthetic: true });
    const inner = target.__s0;
    if (inner.stopped()) return send("synthetic-dropped", { seq, reason: "stopped", synthetic: true });
    if (!inner.input) return send("synthetic-dropped", { seq, reason: "no-input", synthetic: true });
    if (inner.muted()) return send("synthetic-dropped", { seq, reason: "muted", synthetic: true });
    const at = Date.now(), turn_id = `s0-turn-${seq}`;
    inner.turn.emit({ phase: "started", turn_id, started_at: at });
    inner.turn.emit({
      phase: "finished", turn_id, started_at: at, ended_at: at, merged: false,
      text: `S0 synthetic turn ${seq}, emitted natively at ${new Date(t).toISOString()}. Please reply with exactly: S0 echo ${seq}.`,
      timings: { audio_ms: 0, endpoint_silence_ms: 0, recognition_ms: 0 },
    });
    send("synthetic-turn", { seq, turn_id, call: target.id, synthetic: true });
  };
})();
