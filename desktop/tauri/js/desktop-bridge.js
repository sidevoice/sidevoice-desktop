// Sidevoice desktop bridge — injected by the desktop app into every page its main window loads.
//
// The contract is documented in docs/BRIDGE.md. In short:
//   page → app : invoke('bridge_state', { snapshot })  whenever the call state the app shows changes (tray, card)
//                invoke('bridge_level', { level })     the microphone's level during a call, for the card's wave
//                invoke('bridge_playback', { level, threadId })  the voice's output level, for the card's avatar
//   app → page : window.__sidevoiceHost.run({ command: 'toggle-mute' | 'hang-up' | 'skip-reply' | … })
//                the voice call's events (host.voice) come over a channel the page opens when it loads (voice_events);
//                window.__sidevoiceHost.voiceEvent(event) is where each one is handed to the page
//                window.__sidevoiceHost.updaterEvent(status)  the updates' status changed (host.updater)
//                window.__sidevoiceHost.shortcutsEvent(status)  a global shortcut or the headset changed (host.shortcuts, host.muteShortcut)
//                window.__sidevoiceHost.recordHint(hint)  a key was ignored while a shortcut is recorded (host.shortcuts.record's onHint)
// It only reads the web UI's public seams (window.sidevoiceUI.store, .micLevel, .playbackLevel and .openSettings,
// window.sidevoiceActions) and
// never touches the DOM. On any origin other than the window's page (the room's, or the app's own for the
// bundled interface) it does nothing at all.
//
// This is the desktop's part of the bridge: what only this app has (the local host, the updater, the mute shortcut, the
// developer options, the call controls card and its projection of the call, the reset) and the assembly of `host`. What every
// app shares is the members' (sidevoice-shared-tauri/js/members.js), reached through the shell's transport
// (sidevoice-shared-tauri/js/transport.js). The three are one script, composed by the app (docs/BRIDGE.md -> "Parts"): this
// part is evaluated where `transportOf` and `membersOf`, the other two, are in scope.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_ROOM_ORIGIN__", "__SIDEVOICE_MEDIA__", "__SIDEVOICE_LOCAL_HOST__", "__SIDEVOICE_VOICE__", "__SIDEVOICE_UPDATER__", "__SIDEVOICE_BUILD__", "__SIDEVOICE_DEVICE_NAME__", "__SIDEVOICE_DEBUG__", "__SIDEVOICE_MICROPHONE_SETTINGS__", "__SIDEVOICE_UNINSTALL__", "__SIDEVOICE_SHORTCUT_MODIFIER__", "__SIDEVOICE_CAPTURES_KEYS__", "__SIDEVOICE_MICROPHONE_HELP__", "__SIDEVOICE_MESSAGES__");
})(function installDesktopBridge(win, roomOrigin, mediaOwned, localHostOffered, voiceOffered, updaterOffered, build, deviceName, debugState, microphoneSettingsOffered, uninstallOffered, shortcutModifier, capturesKeys, microphoneHelp, messageTables) {
  "use strict";
  // scheme://host rather than location.origin: the app's own pages (tauri://localhost) may have an opaque origin.
  if (win.location.protocol + "//" + win.location.host !== roomOrigin) return null;
  // The app never tells the interface where to look first (sidevoice-web's `__SIDEVOICE_TARGET__`): pairing does.
  win.__SIDEVOICE_TARGET__ = null;
  if (win.__sidevoiceHost) return win.__sidevoiceHost;

  const BRIDGE_VERSION = 2;
  const transport = transportOf(win);
  const members = membersOf(win, transport);
  // The commands that are this app's own, not the plugin's: `call` answers, `invoke` does not and never fails.
  const { call, send: invoke } = transport.own;

  // --- The call controls card's projection of the call ---

  /** The fields of the room's conversation rows the card shows (the web UI's ParticipantView). */
  const PARTICIPANT_FIELDS = ["threadId", "title", "selected", "available", "switching", "unread", "reach", "stateLabel",
    "activityNote", "detail", "subtitle", "working", "machine", "harness", "route", "machineReachable"];
  /** Choices of a device the card asked for that the room has not answered yet: the card's devices are busy meanwhile.
   *  They go to the room one after another, in the order asked (the room keeps whichever answers last). */
  let choosingDevices = 0;
  let deviceChoices = Promise.resolve();
  const pick = (object, fields) => {
    const picked = {};
    for (const field of fields) if (object && object[field] !== undefined) picked[field] = object[field];
    return picked;
  };

  /** The card's devices (the web UI's call-controls `AudioDevices`), from the room's own (`facts.audioDevices`: what the
   *  voice lists and what is chosen). The system's default comes first, as an option of its own (id `""`); a device the
   *  system lists without a name is said by its place; the speaker can be chosen only where the voice can move it. */
  function cardDevices(devices) {
    if (!devices || typeof devices !== "object") return null;
    const options = (rows) => [{ id: "", label: "", system: true }].concat((Array.isArray(rows) ? rows : [])
      .map((row, index) => row && typeof row.id === "string" && row.id
        ? (typeof row.label === "string" && row.label ? { id: row.id, label: row.label } : { id: row.id, label: "", number: index + 1 })
        : null)
      .filter(Boolean));
    const outputAvailable = !!devices.outputSelectable;
    return {
      inputs: options(devices.inputs),
      outputs: options(devices.outputs),
      inputId: typeof devices.input === "string" ? devices.input : "",
      outputId: outputAvailable && typeof devices.output === "string" ? devices.output : "",
      available: true,
      outputAvailable,
      busy: choosingDevices > 0,
    };
  }

  // The microphone's level, for the card's wave: during a call, at most every 80 ms, and only when it changes.
  let lastLevel = -1;
  let lastLevelAt = 0;
  function reportLevel(value) {
    if (!reporting.joined()) return;
    const level = Math.round(Math.max(0, Math.min(100, Number(value) || 0)));
    const now = Date.now();
    if (level === lastLevel || (level !== 0 && now - lastLevelAt < 80)) return;
    lastLevel = level;
    lastLevelAt = now;
    transport.send("bridge_level", { level });
  }

  // The voice's output level, for the card's avatar's mouth: during a call, at most every 40 ms, only when it changes,
  // and always the 0 that ends a reply.
  let lastPlayback = { level: 0, threadId: null };
  let lastPlaybackAt = 0;
  function reportPlayback(sample) {
    if (!reporting.joined() || !sample || typeof sample !== "object") return;
    const level = Math.round(Math.max(0, Math.min(1, Number(sample.level) || 0)) * 100) / 100;
    const threadId = typeof sample.threadId === "string" ? sample.threadId : null;
    const now = Date.now();
    if (level === lastPlayback.level && threadId === lastPlayback.threadId) return;
    if (level !== 0 && now - lastPlaybackAt < 40) return;
    lastPlayback = { level, threadId };
    lastPlaybackAt = now;
    transport.send("bridge_playback", { level, threadId });
  }

  // --- What only the desktop has ---

  /** This computer's own core, the local host (docs/LOCAL_HOST.md): its state, the pairing the page uses to reach it
   *  (through the app's proxy, with this launch's secret — never the device token), the service's actions, and
   *  installing Sidevoice here (`install`, `cancel`). Each action resolves the state after it (`{state, failure?, core?,
   *  product_version, service?, calls?, attempts?, limit?, installed?, reachable}`) or rejects `{key, message}`.
   *  Offered on every OS (core `bridge`). */
  function localHost() {
    const listeners = new Set();
    let last = null; // the state last delivered, as JSON
    let timer = null;
    let localInstalls = 0; // numbers each install call's job
    const poll = () =>
      call("local_host_state")
        .then((state) => {
          const key = JSON.stringify(state);
          if (key === last) return;
          last = key;
          for (const listener of listeners) {
            try { listener(state); } catch (_) { /* a listener's error is the page's own */ }
          }
        })
        .catch(() => {});
    const action = (name) => () => call("local_host_action", { action: name });
    return {
      /** `{state, failure?, core?, product_version, service?, calls?, attempts?, limit?, installed?, reachable}`; `state`
       *  one of `absent`, `installing` (while `install` runs), `not-installed`, `stopped-by-person`, `starting`,
       *  `backoff`, `running`, `failed`, `service-failed`, `refused`, `incompatible`; `product_version` the installed Sidevoice's version (the
       *  `sidevoice` package, not `core.version`), `null` when unknown or not installed; `installed` whether the service
       *  starts at the next sign-in (the connector's `start_at_login`), absent when the connector does not say. */
      state: () => call("local_host_state"),
      /** `listener(state)` with the state now, then on every change (polled every 2 s while anyone listens).
       *  Returns `stop`. */
      subscribe(listener) {
        if (typeof listener !== "function") throw new TypeError("subscribe(listener)");
        listeners.add(listener);
        if (last !== null) {
          try { listener(JSON.parse(last)); } catch (_) { /* the page's own */ }
        }
        poll();
        if (timer === null) timer = win.setInterval(poll, 2000);
        return () => {
          listeners.delete(listener);
          if (listeners.size === 0 && timer !== null) {
            win.clearInterval(timer);
            timer = null;
            last = null;
          }
        };
      },
      /** `{fp, public_key, device_id, token, urls: ["http://127.0.0.1:<port>"], host, local: true}`
       *  whenever a core answers and the app is paired (`reachable`), whatever `state` says; else `null`. `token` is
       *  the app's proxy secret for this launch. */
      pairing: () => call("local_host_pairing"),
      start: action("start"),
      stop: action("stop"),
      restart: action("restart"),
      serviceInstall: action("service-install"),
      serviceUninstall: action("service-uninstall"),
      /** Whether the service starts at the next sign-in, and only that: a running service keeps running, a stopped one
       *  stays stopped (the connector's `service login on|off`). `enabled` must be a boolean; anything else rejects
       *  `{key: "bad_request"}` without running anything. A connector that cannot do it rejects
       *  `{key: "service.login-unsupported"}`. */
      setStartAtLogin: (enabled) =>
        typeof enabled === "boolean"
          ? call("local_host_action", { action: enabled ? "start-at-login-on" : "start-at-login-off" })
          : Promise.reject({ key: "bad_request", message: "setStartAtLogin(enabled) takes a boolean." }),
      /** Pairs this app with the core again (after `refused`): never done on its own. */
      reconnect: action("reconnect"),
      /** Shows the app's and core's logs in the system file manager (else the connector's). */
      revealLog: action("reveal-log"),
      /** A code for another device, on the person's click only: `{code, expires_in, reach}`. Shown, never sent. */
      pairingCode: () => call("local_host_pairing_code"),
      /** "Prepare this computer": installs Sidevoice from the npm registry, without Node (docs/LOCAL_HOST.md → Install;
       *  sidevoice-web's `LocalHostBridge.install`). Answers at once a promise of the state after it, once the app is
       *  paired with the new core (`LocalHostInstallOperation`), carrying `job`, this call's id for `cancel`.
       *  `onProgress({step, done, total, cancellable})` about twice a second while it runs, when that changes: `step`
       *  `download` (its bytes; `total` null when unknown), `verification`, `staging`, `service-start`, `pairing`.
       *  Rejects `{key, step?, params?, log_tail?, message}`: `install.cancelled` once cancelled, `install.busy` while
       *  another install runs, the rest as docs/BRIDGE.md lists them. */
      install(onProgress) {
        const job = "local-install-" + ++localInstalls + "-" + Date.now().toString(36);
        let finished = false;
        let told = null; // the progress last handed on, as JSON
        const operation = call("local_host_install", { job }).finally(() => { finished = true; });
        const poll = () => {
          if (finished) return;
          call("local_host_install_progress", { job })
            .then((report) => {
              if (finished || !report || typeof report !== "object" || typeof report.step !== "string") return;
              const progress = {
                step: report.step,
                done: typeof report.done === "number" ? report.done : null,
                total: typeof report.total === "number" ? report.total : null,
                cancellable: report.cancellable === true,
              };
              const key = JSON.stringify(progress);
              if (key === told) return;
              told = key;
              try { onProgress(progress); } catch (_) { /* a listener's error is the page's own */ }
            })
            .catch(() => {});
          win.setTimeout(poll, 500);
        };
        if (typeof onProgress === "function") poll();
        operation.job = job;
        return operation;
      },
      /** Stops install `job` while that is still honoured (until the package's own installer starts): true when the
       *  install will reject `install.cancelled`, false once it is too late or for a job not under way. */
      cancel: (job) =>
        typeof job === "string" && job
          ? call("local_host_install_cancel", { job }).then((accepted) => accepted === true)
          : Promise.resolve(false),
    };
  }

  /** The app's updates: the `AppUpdater` seam, 1:1 onto the `updater` command's actions (docs/BRIDGE.md → "The updater").
   *  Only in a release or a nightly that can update itself. */
  function updater() {
    const listeners = listened();
    const host = {
      settings: () => call("updater", { action: "settings" }),
      setMode: (mode) => call("updater", { action: "set-mode", mode }),
      status: () => call("updater", { action: "status" }),
      /** `listener(status)` on every change. Returns `stop`. */
      subscribe: listeners.subscribe,
      check: () => call("updater", { action: "check" }),
      download: () => call("updater", { action: "download" }),
      cancel: () => call("updater", { action: "cancel" }).then(() => undefined),
      restart: () => call("updater", { action: "restart" }).then(() => undefined),
    };
    return { host: Object.freeze(host), changed: listeners.tell };
  }

  /** Listeners the app tells of a change (`webview.eval`): `subscribe(listener)` returns `stop`; `tell(value)` hands
   *  each the value, past one that throws. */
  function listened() {
    const listeners = new Set();
    return {
      subscribe(listener) {
        if (typeof listener !== "function") throw new TypeError("subscribe(listener)");
        listeners.add(listener);
        return () => { listeners.delete(listener); };
      },
      tell(value) {
        for (const listener of listeners) {
          try { listener(value); } catch (_) { /* a listener's error is the page's own */ }
        }
      },
    };
  }

  /** The global shortcuts and the headset's buttons: sidevoice-web's `ShortcutsBridge`, onto the `global_shortcuts` command
   *  (docs/BRIDGE.md → "The shortcuts"), and `host.muteShortcut`, the older seam of the mute one alone, as a thin view of it.
   *  A shortcut is `{accelerator, label, state, alternatives, voiceOver, managedBySystem}`; `get()` is the status `{shortcuts:
   *  {mute, holdToTalk, oneTurn}, permission, headset}`. */
  function shortcuts(captures) {
    const everyone = listened();
    const mute = listened();
    /** The hint callback of the recording on (a second `record` ends the first, so there is one). */
    let hinting = null;
    const ACTIONS = ["mute", "holdToTalk", "oneTurn"];
    const refusal = (key, message) => Promise.reject({ key, message });
    const action = (name) => (ACTIONS.includes(name) ? null : { key: "bad_request", message: "no such shortcut action: " + name });
    const setting = (target, accelerator) => {
      const wrong = action(target);
      if (wrong) return Promise.reject(wrong);
      return typeof accelerator === "string" || accelerator === null
        ? call("global_shortcuts", { action: "set", target, accelerator })
        : refusal("shortcut_invalid", "set(action, accelerator) takes a string or null.");
    };
    const host = {
      get: () => call("global_shortcuts", { action: "get" }),
      /** `accelerator` as the web writes it: keys together (`Ctrl+Alt+Shift+M`), or a single key with how it is pressed
       *  (`Fn:hold`, `RightControl:double`, `F13:tap`); `null` for none. Answers how the system took it (`state`,
       *  `alternatives`); rejects `{key: "shortcut_invalid"}` for one that is not a shortcut, or that the action cannot be
       *  done with. */
      set: setting,
      /** Puts `action` back on this system's default, as if the person never chose: the default is the app's state, not a value
       *  `set` could be given. Answers the shortcut. */
      restore: (target) => {
        const wrong = action(target);
        return wrong ? Promise.reject(wrong) : call("global_shortcuts", { action: "restore", target });
      },
      /** `listener(status)` on every change: set here, refused, or a permission given. Returns `stop`. */
      subscribe: everyone.subscribe,
      /** The person's own Allow for single keys (macOS: Input Monitoring); the system asks only now. */
      request: (permission) =>
        permission === undefined || typeof permission === "string"
          ? call("global_shortcuts", { action: "request", ...(permission === undefined ? {} : { permission }) })
          : refusal("bad_request", "request(permission) takes a string."),
      /** What a headset's presses do: `{press?, doublePress?}`, each `"mute" | "oneTurn" | "skip" | "none"`. */
      setHeadset: (change) =>
        change && typeof change === "object"
          ? call("global_shortcuts", { action: "headset", headset: change })
          : refusal("bad_request", "setHeadset(change) takes an object."),
      /** The app captures the next key itself, Fn included, which a page cannot see: the accelerator, or `null` for Escape. Only
       *  where the system lets the app hear keys with no permission. */
      ...(captures === true ? {
        record: (target, onHint) => {
          const wrong = action(target);
          if (wrong) return Promise.reject(wrong);
          const mine = typeof onHint === "function" ? onHint : null;
          hinting = mine;
          const done = () => { if (hinting === mine) hinting = null; };
          return call("global_shortcuts", { action: "record", target }).then((accelerator) => { done(); return accelerator; }, (failure) => { done(); throw failure; });
        },
      } : {}),
    };
    /** `host.muteShortcut`: the mute one of the same status, for a page that has only this. */
    const muteHost = {
      get: () => host.get().then((status) => status.shortcuts.mute),
      set: (accelerator) => setting("mute", accelerator),
      subscribe: mute.subscribe,
    };
    return {
      host: Object.freeze(host),
      muteHost: Object.freeze(muteHost),
      /** The app ignored a key while recording (`leftModifier`, `loneModifier`): to the recording's `onHint`. */
      hint(reason) {
        if (hinting && (reason === "leftModifier" || reason === "loneModifier")) {
          try { hinting(reason); } catch (_) { /* the page's own */ }
        }
      },
      changed(status) {
        everyone.tell(status);
        if (status && status.shortcuts) mute.tell(status.shortcuts.mute);
      },
    };
  }

  /** What happens to other apps' media while the microphone is open or the agent speaks in a call: sidevoice-web's
   *  `MediaDuckBridge`, onto the `media_duck` command (docs/MEDIA_DUCK.md). Its answer is `{mode, level, lower, pause,
   *  access?, capture?}` (`capture` may be `restart`: allowed, and only a restart applies it). */
  function mediaDuck() {
    const invalid = () => ({ key: "bad_request", message: "set(mode) takes 'lower', 'pause' or 'off'." });
    const unleveled = () => ({ key: "bad_request", message: "setLevel(level) takes 'half', 'quarter' or 'mute'." });
    const unnamed = () => ({ key: "bad_request", message: "takes the permission: 'capture' or 'automation'." });
    const named = (action, permission) => (typeof permission === "string" ? call("media_duck", { action, permission }) : Promise.reject(unnamed()));
    return Object.freeze({
      get: () => call("media_duck", { action: "get" }),
      /** The person's own request, from the list of permissions, to let the app do what `permission` names (macOS:
       *  `capture`, System Audio Recording; `automation`, controlling Music and Spotify): when the system may ask. */
      allow: (permission) => named("allow", permission),
      /** The system's page where the person turns `permission` back on (macOS); rejects `unsupported` elsewhere. */
      openSettings: (permission) => named("settings", permission),
      /** Quits and reopens the app, once `capture` says `restart` (System Audio Recording is allowed but only a new process
       *  sees it); refused with `media_duck.restart-in-call` while a call is joined, `media_duck.restart-not-needed` otherwise. */
      restart: () => call("media_duck", { action: "restart" }),
      set: (mode) => (typeof mode === "string" ? call("media_duck", { action: "set", mode }) : Promise.reject(invalid())),
      /** How far Lower takes other apps: `half`, `quarter` (the default) or `mute`; applied at once while lowered. */
      setLevel: (level) => (typeof level === "string" ? call("media_duck", { action: "level", level }) : Promise.reject(unleveled())),
    });
  }

  /** The developer options (nightly and dev builds): sidevoice-web's hidden "Developer options" section, onto the
   *  `debug_options` command (docs/BRIDGE.md -> "The developer options"). Each switch reads live: what the app kept, then
   *  what the last `set` took. */
  function debug(initial) {
    const SWITCHES = ["inspectable", "compositingBorders", "repaintCounters"];
    const available = Object.freeze(SWITCHES.filter((name) => (initial.available || []).includes(name)));
    const state = {};
    const take = (from) => {
      for (const name of SWITCHES) state[name] = !!(from && from[name]) && available.includes(name);
    };
    take(initial);
    return Object.freeze({
      /** The switches this platform has; the page hides the others. */
      available,
      get inspectable() {
        return state.inspectable;
      },
      get compositingBorders() {
        return state.compositingBorders;
      },
      get repaintCounters() {
        return state.repaintCounters;
      },
      /** Turns one switch on or off on the room and the call controls card, at once (no reload), and keeps it. Resolves
       *  when done; the switch then reads what took. A name not in `available` or a non-boolean `on` rejects
       *  `{key: "bad_request"}`. */
      set: (name, on) =>
        available.includes(name) && typeof on === "boolean"
          ? call("debug_options", { action: "set", name, enabled: on }).then(take)
          : Promise.reject({ key: "bad_request", message: "set(name, on) takes an available switch and a boolean." }),
    });
  }

  /** The floating card's settings the app keeps: sidevoice-web's `CallControlsBridge`, onto the `call_controls_settings`
   *  command (docs/BRIDGE.md → "The call controls settings"). */
  function callControls() {
    const CORNERS = ["tl", "tr", "bl", "br"];
    return Object.freeze({
      /** `{alwaysShown, enabled, corner, moved}`: whether it shows always in a call, whether it shows at all, the corner a card never moved first appears
       *  in, and whether it was moved or resized anywhere (what `resetPlace` forgets). */
      settings: () => call("call_controls_settings", { action: "settings" }),
      /** Kept by the app and applied at once. `enabled` must be a boolean, else it rejects `{key: "bad_request"}`. */
      setAlwaysShown: (enabled) =>
        typeof enabled === "boolean"
          ? call("call_controls_settings", { action: "set-always-shown", enabled })
          : Promise.reject({ key: "bad_request", message: "setAlwaysShown(enabled) takes a boolean." }),
      /** Whether the card shows at all during a call (on by default); off, no card shows, not even the moment a change in
       *  Settings shows it. `enabled` must be a boolean, else it rejects `{key: "bad_request"}`. */
      setEnabled: (enabled) =>
        typeof enabled === "boolean"
          ? call("call_controls_settings", { action: "set-enabled", enabled })
          : Promise.reject({ key: "bad_request", message: "setEnabled(enabled) takes a boolean." }),
      /** The preferred corner: `"tl"`, `"tr"` (the default), `"bl"` or `"br"`. Setting it, the one already chosen too, moves the card there
       *  now and forgets the places it was dragged to (its sizes stay); dragged after that, it stays where it is dropped. */
      setCorner: (corner) =>
        CORNERS.includes(corner)
          ? call("call_controls_settings", { action: "set-corner", corner })
          : Promise.reject({ key: "bad_request", message: "setCorner(corner) takes tl, tr, bl or br." }),
      /** Forgets where the card was left and how the picture was resized, on every display. */
      resetPlace: () => call("call_controls_settings", { action: "reset-place" }).then(() => undefined),
      /** Forgets the picture's resized size (its "Custom") on every display, and keeps where the card was left: a
       *  starting size picked in Settings applies again. */
      clearSize: () => call("call_controls_settings", { action: "clear-size" }).then(() => undefined),
      /** During a call, shows the card a moment, in front of the room too: after a change in Settings. */
      preview: () => { invoke("call_controls_settings", { action: "preview" }); },
      /** Leaving Settings: a preview showing goes at once (else it stays while the pointer is on it or it is dragged, and goes
       *  two seconds after). */
      endPreview: () => { invoke("call_controls_settings", { action: "end-preview" }); },
      /** Whether the page shows the call's stage (the conversation's creature and its balloons): false on Settings and
       *  any other screen without it (#165). `rect`, while shown: where the stage is, in CSS pixels from the top-left
       *  corner of the page's viewport, for the app to measure how much of it other apps' windows cover. The card shows
       *  while the stage is out of view; until the page first says, the app takes it as shown. Anything else is ignored. */
      setStageVisible: (visible, rect) => {
        if (typeof visible !== "boolean") return;
        const box = rect && typeof rect === "object" && ["x", "y", "width", "height"].every((k) => Number.isFinite(rect[k]))
          ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
          : null;
        invoke("call_controls_settings", box ? { action: "stage", visible, rect: box } : { action: "stage", visible });
      },
    });
  }

  /** The call as the room reports it to the app, and the commands the app runs on the room (the members' `room`), with
   *  what only the desktop shows of it: the card's participants and devices, and the card's own commands. */
  const reporting = members.room({
    version: BRIDGE_VERSION,
    publish: (snapshot) => invoke("bridge_state", { snapshot }),
    extend: (view, card) => view
      ? {
        // The room's own rows for the card, else the fields of its conversation list the card shows.
        participants: Array.isArray(card.participants) ? card.participants
          : Array.isArray(view.participants) ? view.participants.map((row) => pick(row, PARTICIPANT_FIELDS)) : [],
        devices: cardDevices(view.facts && view.facts.audioDevices),
      }
      : { participants: [], devices: null },
    // The microphone's and the voice's output levels, for the card's wave and avatar.
    attached(ui) {
      if (ui.micLevel && typeof ui.micLevel.subscribe === "function") ui.micLevel.subscribe(reportLevel);
      if (ui.playbackLevel && typeof ui.playbackLevel.subscribe === "function") ui.playbackLevel.subscribe(reportPlayback);
    },
    commands: {
      "select-participant": (request, state, actions) => {
        if (typeof request.threadId !== "string" || typeof actions.selectParticipant !== "function") return false;
        // Only a conversation the room lists. One whose agent is away is one the call can go to: what is said to it
        // waits there for its agent, as the room's own pick allows (sidevoice-web selectParticipant).
        if (!state.participants.some((row) => row.threadId === request.threadId)) return false;
        actions.selectParticipant(request.threadId);
        return true;
      },
      "select-audio-device": (request, state, actions) => {
        if (!["input", "output"].includes(request.kind) || typeof request.id !== "string") return false;
        if (typeof actions.setAudioDevices !== "function") return false;
        // The room's own choice (`setAudioDevices`), kept for this device as its device bar keeps it; `""` is the
        // system's default, which the room chooses as `null`. The card's devices are busy until the room has answered
        // every choice, each asked once the one before has been answered: the last asked is the one that stays.
        choosingDevices += 1;
        reporting.report();
        deviceChoices = deviceChoices
          .then(() => actions.setAudioDevices({ [request.kind]: request.id === "" ? null : request.id }))
          .catch(() => {})
          .finally(() => {
            choosingDevices -= 1;
            reporting.report();
          });
        return true;
      },
      // The card's gear: the room's Settings at the floating card's section (the app brings the room in front).
      "open-settings": () => {
        if (!win.sidevoiceUI || typeof win.sidevoiceUI.openSettings !== "function") return false;
        win.sidevoiceUI.openSettings("floating-card");
        return true;
      },
      // The global shortcuts' (src/shortcuts.rs): hold to talk goes down and is let go (the page's Space hold, which a page
      // losing focus does not end), and say one thing.
      "hold-start": (request, state, actions) => typeof actions.holdStart === "function" && (actions.holdStart(), true),
      "hold-end": (request, state, actions) => typeof actions.holdEnd === "function" && (actions.holdEnd(), true),
      "one-turn": (request, state, actions) => typeof actions.oneTurn === "function" && (actions.oneTurn(), true),
      // The tray's Settings: the room's Settings as it was left (the app brings the room in front).
      "show-settings": () => {
        if (!win.sidevoiceUI || typeof win.sidevoiceUI.openSettings !== "function") return false;
        win.sidevoiceUI.openSettings();
        return true;
      },
    },
  });

  // The app runs the voice call on every OS (core `bridge` always says so; the flag stays while the page reads it).
  const voiceCall = voiceOffered === true ? members.voice() : null;
  // Only in a release or a nightly that can update itself (core `updater::offered`).
  const updates = updaterOffered === true ? updater() : null;
  const shortcut = shortcuts(capturesKeys);
  // Only in a nightly or a dev build (core `debug_options::offered`): the switches' state and which this platform has.
  const debugOptions = debugState && typeof debugState === "object" && Array.isArray(debugState.available) ? debug(debugState) : null;


  const api = {
    version: BRIDGE_VERSION,
    /** What this app offers the page besides the webview itself, one optional member per thing; a member that is there is
     *  offered (docs/BRIDGE.md → "The host"). `engine`: the app's engine, its catalogues, keys and models on disk
     *  (docs/ENGINES.md); `voice`: the call the app runs; `microphone`: native permission with `voice`, or just the
     *  system settings link. */
    host: Object.freeze({
      /** The label of the modifier a shortcut is written with on this system, `⌘` or `Ctrl`. The page never asks which system
       *  this is: what a system decides about its words is told here (this, `microphone.help`), already resolved. */
      ...(typeof shortcutModifier === "string" && shortcutModifier ? { shortcutModifier } : {}),
      engine: Object.freeze(members.engine()),
      /** Only where the app owns the system's media: it answers headset buttons / media keys in a call, so the page must
       *  not register its own Media Session handlers (both would toggle the microphone on one click). Elsewhere the page
       *  does. */
      ...(mediaOwned === true ? { presence: Object.freeze({ media: "app" }) } : {}),
      /** The webview sends plain http to a private address; to whom, and with whose consent, is the page's policy. */
      reach: Object.freeze({ plainHttpPrivate: true }),
      // This computer's own core, on every OS (core `bridge` always says so).
      ...(localHostOffered === true ? { localHost: Object.freeze(localHost()) } : {}),
      // Private pairings and the reset are available on every desktop platform; the uninstall only where the app can
      // also remove itself (macOS).
      pairings: members.pairings(),
      reset: Object.freeze({
        deleteAll: () => call("data_delete_all"),
        resetReport: () => call("data_reset_report"),
        ...(uninstallOffered === true ? {
          uninstall: () => call("data_uninstall"),
        } : {}),
      }),
      // The voice call the app runs: the page neither opens the microphone nor runs models.
      ...(voiceCall ? { voice: voiceCall.host } : {}),
      // The app's native microphone access, on every OS (src/platform/microphone.rs), and its words for a blocked one.
      microphone: members.microphone(microphoneSettingsOffered === true, microphoneHelp),
      // The app's wording of the web's texts that an app changes (shared/tauri/i18n), over the web's catalogue.
      ...(members.messages(messageTables) ? { messages: members.messages(messageTables) } : {}),
      /** What this build is: a release, a nightly (both with `updater`), or a development build. */
      build: Object.freeze({ ...build }),
      // The name the person gave this computer ("Mac de Ana"): the page proposes it as the device's name. Only
      // where the system has one.
      ...(typeof deviceName === "string" && deviceName ? { deviceName } : {}),
      /** The global shortcuts (mute, hold to talk, say one thing) and the headset's press, and `muteShortcut`, the mute one alone:
       *  the app's, set from the page's Settings. */
      shortcuts: shortcut.host,
      muteShortcut: shortcut.muteHost,
      mediaDuck: mediaDuck(),
      /** Whether the person's screen is locked now (`screen_locked`): the page's "only with the screen off", which a room window
       *  hidden behind the floating card is not. `false` where the system tells the app nothing. */
      screen: Object.freeze({ locked: () => call("screen_locked").then((locked) => locked === true) }),
      callControls: callControls(),
      // Only in a nightly or a dev build: the developer options (docs/BRIDGE.md -> "The developer options").
      ...(debugOptions ? { debug: debugOptions } : {}),
      // Only in a release or a nightly that can update itself.
      ...(updates ? { updater: updates.host } : {}),
    }),
    /** An event of the voice call, from the app (`webview.eval`): to `host.voice`'s listeners. */
    voiceEvent(event) {
      if (voiceCall) voiceCall.receive(event);
    },
    /** The updates' status changed, from the app (`webview.eval`): to `host.updater.subscribe`'s listeners. */
    updaterEvent(status) {
      if (updates) updates.changed(status);
    },
    /** A global shortcut or the headset changed, from the app (`webview.eval`): to `host.shortcuts.subscribe`'s listeners,
     *  and the mute one to `host.muteShortcut.subscribe`'s. */
    shortcutsEvent(status) {
      shortcut.changed(status);
    },
    /** A key was ignored while a shortcut is recorded, from the app (`webview.eval`): to the recording's `onHint`. */
    recordHint(hint) {
      shortcut.hint(hint);
    },
    snapshot: reporting.snapshot,
    /** Runs one command from the app (tray, shortcut, headset, call controls card) through the web UI's own actions:
     *  `{command, …arguments}`. Returns whether it ran. */
    run: reporting.run,
  };
  win.__sidevoiceHost = api;

  reporting.start();
  return api;
});
