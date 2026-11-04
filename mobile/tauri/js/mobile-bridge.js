// Sidevoice mobile bridge — injected by the mobile app into the page its window loads (docs/MOBILE.md -> "The bridge").
//
// This is the mobile app's part of the page's script: the assembly of `host` from the members every app shares
// (sidevoice-shared-tauri/js/members.js), reached through the shell's transport (sidevoice-shared-tauri/js/transport.js). The three
// are one script, composed by the app (mobile/tauri/src/bridge.rs, docs/BRIDGE.md -> "Parts"): this part is evaluated where
// `transportOf` and `membersOf`, the other two, are in scope.
//
// The phone offers the voice call it runs itself, its engine, its microphone permission, its pairings and the camera that scans a pairing code, and owns the
// call's media: the page registers no Media Session of its own (`presence.media: "app"`). Nothing of the desktop's
// (the local host, the reset, the mute shortcut, the call controls card, the updater) is offered, and its `debug` has the
// phone's one switch, in a nightly or a dev build:
// a member that is not there is not offered. On any origin other than the app's own it does nothing at all.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_APP_ORIGIN__", "__SIDEVOICE_MICROPHONE_SETTINGS__", "__SIDEVOICE_MICROPHONE_HELP__", "__SIDEVOICE_MESSAGES__", "__SIDEVOICE_DEVICE_NAME__", "__SIDEVOICE_BUILD__", "__SIDEVOICE_DEBUG__");
})(function installMobileBridge(win, appOrigin, microphoneSettingsOffered, microphoneHelp, messageTables, deviceName, build, debugState) {
  "use strict";
  // scheme://host rather than location.origin: the app's own pages (tauri://localhost) may have an opaque origin.
  if (win.location.protocol + "//" + win.location.host !== appOrigin) return null;
  // The app never tells the interface where to look first (sidevoice-web's `__SIDEVOICE_TARGET__`): pairing does.
  win.__SIDEVOICE_TARGET__ = null;
  if (win.__sidevoiceHost) return win.__sidevoiceHost;

  const transport = transportOf(win);
  const members = membersOf(win, transport);
  // Its events come over the channel the members open for it (`voice_events`), not through the global.
  const voiceCall = members.voice();

  /** The call as the room reports it (the members' `room`), sent to the app's own `call_state` whenever it changes; the
   *  app runs the shared commands (`hang-up`, `toggle-mute`, …) on the room through `run`: the call's service hangs up
   *  through it when the person ends the call from its notification. */
  // The page's number, asked once (`call_page`): every report carries it, so that one of this page reaching the app after
  // a newer page has loaded changes nothing of the newer page's call. Reports keep their order: each waits on the same
  // answer.
  let pageNumber = null;
  let page = null; // the number, once known
  const numbered = () => (pageNumber ??= transport.own.call("call_page").then((number) => (page = number), (error) => {
    pageNumber = null;
    throw error;
  }));
  // The mute the system took for an interruption (a phone call, an alarm) when the person was not muted: `{since, seen}`, for
  // the call it was taken in; `seen` once the page has been seen muted. It is the system's only while nobody has touched the
  // mute since: a person who unmutes (and perhaps mutes again) has taken it over, and nothing is given back.
  let owned = null;
  /** Whatever happens to the mute while it is ours: the person taking it over, or the call ending. */
  const follow = (ui) => {
    if (!owned) return;
    const view = (ui.store && ui.store.getState()) || {};
    const enabled = (view.mic || {}).enabled !== false;
    if (!(view.call || {}).joined) {
      // The call ended while interrupted: the mute goes back as it was, for the next one.
      const mine = owned;
      owned = null;
      if (mine.seen && !enabled && !(view.mic || {}).disabled && win.sidevoiceActions) win.sidevoiceActions.toggleMic();
    } else if (!enabled) owned.seen = true;
    else if (owned.seen) owned = null; // unmuted by the person
  };
  /** The call's surfaces (the Live Activity, the call's notification; docs/MOBILE.md -> "The call surface"): the room's
   *  `callSurface` rides in every report as `surface`, its creatures' stills by id (`{id, k}`). Their bytes are handed over
   *  by the page itself, once per id and before the report that names it (`host.callSurface.still`), and forgotten when
   *  the page shows them no more (`forget`). */
  const surfaceOf = (view) => {
    const surface = view && view.callSurface;
    return surface && typeof surface === "object" ? { surface } : {};
  };
  /** What goes to the app about the call (its reports, the stills, the forgetting), one after the other, on the page's
   *  number: a still is answered once its file is kept (`call_still`), and nothing after it goes before that, so a report
   *  never names a still the app does not have yet. A refusal holds nothing up. */
  let line = Promise.resolve();
  const inOrder = (command, args, answered = false) => {
    line = line
      .then(() => numbered())
      .then((number) => (answered ? transport.own.call : transport.own.send)(command, { page: number, ...args }))
      .catch(() => {});
  };
  const reporting = members.room({
    version: 2,
    publish: (snapshot) => inOrder("call_state", { snapshot }),
    extend: surfaceOf,
    attached: (ui) => ui.store.subscribe(() => follow(ui)),
    commands: {
      // The system took the audio away: the call pauses as the page's own mute does (the page shows it muted). A person who
      // had muted themselves stays as they were, and is not unmuted when it ends.
      interrupt: (_request, state, actions) => {
        if (!state.joined) return false;
        if (state.micEnabled && !state.micDisabled) {
          // Ours before it is muted: the store notifies as the mute is made, and the watcher must find it ours.
          owned = { since: state.since, seen: false };
          actions.toggleMic();
        }
        return true;
      },
      // The system gave it back: the mute goes back as it was, if it is still the system's (the person has not taken it over),
      // unless the system advises not to resume (`restoreMic` false: the mute stays, the person's from now on).
      resume: (request, state, actions) => {
        const mine = owned;
        owned = null;
        if (mine && mine.since === state.since && request.restoreMic !== false && !state.micEnabled && !state.micDisabled) {
          actions.toggleMic();
        }
        return true;
      },
      // The system's own call (CallKit, Android's Telecom) was muted or unmuted from outside the page: its in-call screen, a
      // Bluetooth or car kit. The page's mute is set to that, as the person's own (it is the person who asked); asking for
      // what it is already does nothing, so the app hearing back its own change is harmless.
      mute: (request, state, actions) => {
        if (!state.joined || state.micDisabled || typeof request.muted !== "boolean") return false;
        if (state.micEnabled === request.muted) actions.toggleMic();
        return true;
      },
      // The stack of conversations on the call's surfaces was tapped: the app is in front, and the page shows its list
      // of conversations (the room's `openConversations`, where the page has it).
      "open-conversations": () => {
        const ui = win.sidevoiceUI;
        if (!ui || typeof ui.openConversations !== "function") return false;
        ui.openConversations();
        return true;
      },
    },
  });

  /** The developer options (nightly and dev builds): sidevoice-web's hidden "Developer options" section, onto the
   *  `developer_options` command (docs/BRIDGE.md -> "The developer options"), as the desktop's `host.debug`. The one switch
   *  a phone has is `systemCall`: whether the call is a system's call (CallKit, Telecom). It reads live: what the app kept (asked
   *  again as each page loads, the injected snapshot being from the app's start), then what the last `set` took. */
  function debug(initial) {
    const SWITCHES = ["systemCall"];
    const available = Object.freeze(SWITCHES.filter((name) => (initial.available || []).includes(name)));
    const state = {};
    const take = (from) => {
      for (const name of SWITCHES) state[name] = !!(from && from[name]) && available.includes(name);
    };
    take(initial);
    // The script is built once, when the app starts, and a page that loads anew gets the same text: its snapshot is what the
    // switch was then. Read the app's own state as the page loads, so that a reload shows what the next call will use.
    if (available.length > 0) transport.own.call("developer_options", { action: "get" }).then(take, () => {});
    return Object.freeze({
      available,
      get systemCall() {
        return state.systemCall;
      },
      set: (name, on) =>
        available.includes(name) && typeof on === "boolean"
          ? transport.own.call("developer_options", { action: "set", name, enabled: on }).then(take)
          : Promise.reject({ key: "bad_request", message: "set(name, on) takes an available switch and a boolean." }),
    });
  }

  /** The camera that scans the pairing code (docs/MOBILE.md -> "Scanning the pairing code"): Tauri's barcode scanner plugin
   *  (CameraX and ML Kit on Android, AVFoundation on iOS) behind four calls. The plugin's own words (its permission states, its
   *  rejection strings) stay here: the page gets `granted | prompt | denied` for the permission and one result of a scan, and
   *  nothing of a scan ever rejects. The page draws the viewfinder: the scan is windowed, the camera is behind the web view and
   *  the page shows it through (a transparent page with its own cancel), so `cancel()` is the page's. What the QR holds is the
   *  pairing code as the machine prints it (`SV1.…`): the page reads it, as it reads a typed one. */
  function codeScanner() {
    const PLUGIN = "plugin:barcode-scanner|";
    const call = (command, args) => transport.own.call(PLUGIN + command, args);
    const stateOf = (answer) => {
      const camera = answer && answer.camera;
      return camera === "granted" || camera === "denied" ? camera : "prompt";
    };
    const permission = () => call("check_permissions").then(stateOf, () => "denied");
    /** Asks for the camera if the person has not decided yet (the system's prompt); a decided answer is not asked again. */
    const request = () =>
      call("request_permissions").then(stateOf, () => "denied").then((state) => (state === "granted" ? "granted" : "denied"));
    /** Whether the phone has a camera to scan with. The plugin's scan never answers on an Android phone without one, so the
     *  page is told first. Only a list that was read and holds no video input says so; anything else (no API, a refusal) is
     *  left to the plugin, which refuses a missing camera itself on iOS. */
    const hasCamera = () => {
      const devices = win.navigator && win.navigator.mediaDevices;
      if (!devices || typeof devices.enumerateDevices !== "function") return Promise.resolve(true);
      return devices.enumerateDevices().then((list) => !Array.isArray(list) || list.some((device) => device.kind === "videoinput"), () => true);
    };
    let pending = null;
    // `cancel()` (or the page going away) before the camera is started: nothing is started.
    let cancelled = false;
    const scanOnce = () => {
      cancelled = false;
      return request().then((state) => {
        if (state !== "granted") return { status: "denied" };
        if (cancelled) return { status: "cancelled" };
        return hasCamera().then((camera) => (cancelled ? { status: "cancelled" } : !camera ? { status: "unavailable" } : started()));
      });
    };
    const started = () =>
        call("scan", { windowed: true, formats: ["QR_CODE"] }).then(
          (found) => ({ status: "scanned", text: found && typeof found.content === "string" ? found.content : "" }),
          (error) => {
            // The plugin rejects "cancelled" when `cancel()` ends the scan. Anything else: the camera was taken away since the
            // answer above (denied), or there is none to scan with (a simulator, a device without a camera).
            if (String((error && error.message) || error) === "cancelled") return { status: "cancelled" };
            return permission().then((now) => ({ status: now === "denied" ? "denied" : "unavailable" }));
          },
        );
    const scan = () => {
      if (!pending) pending = scanOnce().finally(() => { pending = null; });
      return pending;
    };
    // A page that goes away with the camera on: the camera is let go.
    if (typeof win.addEventListener === "function") win.addEventListener("pagehide", () => { if (pending) { cancelled = true; call("cancel").catch(() => {}); } });
    return Object.freeze({
      permission,
      request,
      /** One scan at a time (a second call answers the first's result). Resolves
       *  `{status: "scanned", text} | {status: "cancelled"} | {status: "denied"} | {status: "unavailable"}`; asks for the camera
       *  first, so a scan without `request()` is safe. */
      scan,
      /** Ends the scan in progress (it resolves `cancelled`); nothing when there is none. */
      cancel: () => {
        if (!pending) return Promise.resolve();
        cancelled = true;
        return call("cancel").then(() => {}, () => {});
      },
      /** The app's page of the system's settings, where a denied camera is allowed again. Rejects when the system will not
       *  open it (the page catches that). */
      openSettings: () => call("open_app_settings").then(() => {}),
    });
  }

  const api = {
    /** The contract this app speaks: the desktop's (docs/BRIDGE.md). */
    version: 2,
    /** What this app offers the page besides the webview itself, one optional member per thing (docs/BRIDGE.md -> "The
     *  host"). */
    host: Object.freeze({
      engine: Object.freeze(members.engine()),
      // The voice call the app runs: the page neither opens the microphone nor runs models.
      voice: voiceCall.host,
      // The system's own microphone permission (shared/tauri platform/microphone: AVFoundation's on iOS, the runtime
      // permission on Android), never the webview's; its settings page where it has one, and the app's words for a
      // blocked one on this phone (mobile/tauri/src/i18n.rs).
      microphone: members.microphone(microphoneSettingsOffered === true, microphoneHelp),
      // The app's wording of the web's texts that a phone changes (the speaker is chosen from the system, there is no browser):
      // mobile/tauri/i18n and shared/tauri/i18n, over the web's catalogue.
      ...(members.messages(messageTables) ? { messages: members.messages(messageTables) } : {}),
      // The name the person gave this phone ("Pixel de Ana"): the page proposes it as the device's name, instead of naming
      // it from the webview's user agent ("Chrome on Android"). Only where the system has one.
      ...(typeof deviceName === "string" && deviceName ? { deviceName } : {}),
      /** What this build is, decided when it was built (docs/BRIDGE.md -> "The build"): the page shows it, and unlocks the
       *  developer options by its channel (a nightly or a dev build). */
      build: Object.freeze({ ...build }),
      // Only in a nightly or a dev build (the app tells the state in a build that offers it, `null` in a release).
      ...(debugState && typeof debugState === "object" ? { debug: debug(debugState) } : {}),
      // The pairings, in the app's private storage (mobile/tauri/src/pairings.rs), not the webview's.
      pairings: members.pairings(),
      /** The camera that scans the pairing code a machine shows (the interface choice of sidevoice-desktop#405). */
      codeScanner: codeScanner(),
      /** The app owns the call's audio and the system's media on a phone: the page must not register Media Session
       *  handlers or play a Now Playing element of its own. The app answers the headset's buttons and the lock screen
       *  (sidevoice-shared-tauri's platform/media_controls). */
      presence: Object.freeze({ media: "app" }),
      /** The webview sends plain http to a private address; to whom, and with whose consent, is the page's policy. */
      reach: Object.freeze({ plainHttpPrivate: true }),
      /** The app draws the call's own surfaces (iOS's Live Activity, Android's call notification) from the room's
       *  `callSurface`: the page publishes what costs it work (the stills, the spoken page) while `active()`, and hands
       *  the creatures' stills over here. */
      callSurface: Object.freeze({
        active: () => true,
        /** A still's PNG (`png`, base64), kept by the app under `id` where the surfaces read it. */
        still: ({ id, png, px } = {}) => {
          if (typeof id === "string" && id && typeof png === "string" && png && Number.isInteger(px) && px > 0) {
            inOrder("call_still", { id, png, px }, true);
          }
        },
        /** The stills the page shows no more. */
        forget: (ids) => {
          const strings = Array.isArray(ids) ? ids.filter((id) => typeof id === "string" && id) : [];
          if (strings.length) inOrder("call_forget", { ids: strings });
        },
      }),
    }),
    snapshot: reporting.snapshot,
    /** Runs one command from the app (the call's notification, a headset) through the web UI's own actions:
     *  `{command, …}`. Returns whether it ran. */
    run(request) {
      // A command for a given page's call (`page`, `since`: the call's service hanging up the call it was started for)
      // runs only on that call: the app's command can reach the page after another page or another call took its place.
      if (request && typeof request === "object") {
        if ("page" in request && request.page !== page) return false;
        if ("since" in request && reporting.snapshot().since !== request.since) return false;
      }
      return reporting.run(request);
    },
  };
  win.__sidevoiceHost = api;
  reporting.start();
  return api;
});
