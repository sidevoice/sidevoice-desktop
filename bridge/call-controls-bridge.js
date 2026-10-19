// Sidevoice call controls bridge — injected by the desktop app into its call controls card window, before the page's
// own scripts (docs/BRIDGE.md → "The call controls card").
//
//   app → card : window.__sidevoiceDesktop.receive({ call, level, playback, place, size, pointerInside, outsideClicks,
//                alwaysExpanded, muteShortcut }) (any subset: what changed), evaluated by the app
//   card → app : invoke('call_controls_run' | 'call_controls_layout' | 'call_controls_drag' | 'call_controls_resize' |
//                'call_controls_ready')
//
// The card's page (the web build's call-controls.html) finds it at window.__sidevoiceDesktop.host.callControls. On any
// origin but the app's own it does nothing at all.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_APP_ORIGIN__", "__SIDEVOICE_RESIZE__");
})(function installCallControlsBridge(win, appOrigin, resizeOffered) {
  "use strict";
  // scheme://host rather than location.origin: the app's own pages (tauri://localhost) may have an opaque origin.
  if (win.location.protocol + "//" + win.location.host !== appOrigin) return null;
  if (win.__sidevoiceDesktop) return win.__sidevoiceDesktop;

  let state = null;
  const listeners = new Set();
  /** A device the card chose, until the app next tells the call: its devices are shown busy meanwhile, so the card
   *  never reads the device still chosen as the room refusing the new one. Given up after a while, should the room
   *  never answer (it ignores a command it cannot run). */
  let choosing = null;
  const CHOOSING_MS = 3000;

  /** The state as the card hears it: the app's, with the devices busy while a choice is on its way. */
  function view() {
    if (!choosing || !state || !state.call || !state.call.devices) return state;
    return Object.freeze(Object.assign({}, state, {
      call: Object.assign({}, state.call, { devices: Object.assign({}, state.call.devices, { busy: true }) }),
    }));
  }

  function publish() {
    if (!state || !state.call) return; // nothing to show until the app has said what the call is
    const heard = view();
    for (const listener of listeners) listener(heard);
  }

  function stopChoosing() {
    if (choosing && typeof win.clearTimeout === "function") win.clearTimeout(choosing);
    choosing = null;
  }

  function invoke(command, args) {
    const internals = win.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== "function") return;
    Promise.resolve()
      .then(() => internals.invoke(command, args))
      .catch(() => {});
  }

  const number = (value) => (typeof value === "number" || typeof value === "string") && value !== "" ? Number(value) : NaN;
  const EDGES = ["nw", "ne", "sw", "se"];

  const callControls = Object.freeze({
    /** Called with the whole state, and again on every change; returns the function that stops it. */
    subscribe(listener) {
      listeners.add(listener);
      if (state) listener(view());
      else invoke("call_controls_ready");
      return () => listeners.delete(listener);
    },
    /** One of the card's commands, `{command, …arguments}`; the app checks it and carries it to the room. */
    run(command) {
      if (command && command.command === "select-audio-device" && state && state.call && state.call.devices) {
        stopChoosing();
        choosing = typeof win.setTimeout === "function" ? win.setTimeout(() => { choosing = null; publish(); }, CHOOSING_MS) : true;
        publish();
      }
      invoke("call_controls_run", { command });
    },
    /** The card's size with its margin, in CSS pixels: the app sizes the window to it. `card`: where the card's body is
     *  in it, which the app places and keeps where it rests as the window grows around it. `resizable`: the picture,
     *  which the person may resize (`resize`) down to `minWidth`. */
    layout(size) {
      const width = number(size && size.width), height = number(size && size.height);
      if (!(width > 0 && height > 0)) return;
      const args = { width, height };
      const box = size.card;
      const body = box ? [box.x, box.y, box.width, box.height].map(number) : [];
      if (body.length && body.every(Number.isFinite)) args.card = { x: body[0], y: body[1], width: body[2], height: body[3] };
      const minWidth = number(size.resizable && size.resizable.minWidth);
      if (minWidth > 0) args.resizable = { minWidth };
      invoke("call_controls_layout", args);
    },
    /** Moving the card: "start", every "move", "end". The app follows the pointer itself. */
    drag(phase) {
      if (["start", "move", "end"].includes(phase)) invoke("call_controls_drag", { phase });
    },
    // Resizing the picture by its grip: "start", every "move", "end", by the grip's corner. The app follows the pointer
    // itself, keeps the opposite corner still and tells the card its size (`size`). Not where the app cannot see the
    // pointer (Wayland): the card then offers its preset sizes only.
    ...(resizeOffered === true ? {
      resize(phase, edge) {
        if (["start", "move", "end"].includes(phase) && EDGES.includes(edge)) invoke("call_controls_resize", { phase, edge });
      },
    } : {}),
  });

  const api = {
    host: Object.freeze({ app: "sidevoice-desktop", callControls }),
    /** The app pushes what changed; the card hears the whole state. */
    receive(patch) {
      if (!patch || typeof patch !== "object") return;
      state = Object.freeze(Object.assign({}, state, patch));
      if (patch.call) stopChoosing(); // the room's own word on the devices from here on
      publish();
    },
  };
  win.__sidevoiceDesktop = api;
  return api;
});
