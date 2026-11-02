// Sidevoice mobile bridge — injected by the mobile app into the page its window loads (docs/MOBILE.md -> "The bridge").
//
// This is the mobile app's part of the page's script: the assembly of `host` from the members every app shares
// (sidevoice-shared-tauri/js/members.js), reached through the shell's transport (sidevoice-shared-tauri/js/transport.js). The three
// are one script, composed by the app (mobile/tauri/src/bridge.rs, docs/BRIDGE.md -> "Parts"): this part is evaluated where
// `transportOf` and `membersOf`, the other two, are in scope.
//
// The phone offers the voice call it runs itself, its engine, its microphone permission and its pairings, and owns the
// call's media: the page registers no Media Session of its own (`presence.media: "app"`). Nothing of the desktop's
// (the local host, the reset, the mute shortcut, the call controls card, the updater, `debug`) is offered:
// a member that is not there is not offered. On any origin other than the app's own it does nothing at all.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_APP_ORIGIN__", "__SIDEVOICE_MICROPHONE_SETTINGS__", "__SIDEVOICE_MICROPHONE_HELP__", "__SIDEVOICE_MESSAGES__", "__SIDEVOICE_DEVICE_NAME__", "__SIDEVOICE_BUILD__");
})(function installMobileBridge(win, appOrigin, microphoneSettingsOffered, microphoneHelp, messageTables, deviceName, build) {
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
  const reporting = members.room({
    version: 2,
    publish: (snapshot) => {
      numbered().then((number) => transport.own.send("call_state", { page: number, snapshot }), () => {});
    },
    extend: () => ({}),
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
    },
  });

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
      // The pairings, in the app's private storage (mobile/tauri/src/pairings.rs), not the webview's.
      pairings: members.pairings(),
      /** The app owns the call's audio and the system's media on a phone: the page must not register Media Session
       *  handlers or play a Now Playing element of its own. The app answers the headset's buttons and the lock screen
       *  (sidevoice-shared-tauri's platform/media_controls). */
      presence: Object.freeze({
        media: "app",
        /** The screen kept on during a call (sidevoice-desktop#384): the app's own, since a phone's webview has no Screen Wake
         *  Lock to rely on (none in an insecure context, none that holds in WKWebView). iOS `isIdleTimerDisabled` and Android's
         *  `FLAG_KEEP_SCREEN_ON` (mobile/tauri/plugin `keep_screen_on`) stay set until released: nothing takes them back. */
        screenAwake: Object.freeze({
          supported: true,
          acquire: () => transport.own.call("keep_screen_on", { on: true }).then(() => "held", () => "failed"),
          release: () => { transport.own.call("keep_screen_on", { on: false }).catch(() => {}); },
          onReleased: () => () => {},
        }),
      }),
      /** The webview sends plain http to a private address; to whom, and with whose consent, is the page's policy. */
      reach: Object.freeze({ plainHttpPrivate: true }),
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
