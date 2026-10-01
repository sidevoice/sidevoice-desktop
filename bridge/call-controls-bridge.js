// Sidevoice call controls bridge — injected by the desktop app into its call controls card window, before the page's
// own scripts (docs/BRIDGE.md → "The call controls card").
//
//   app → card : window.__sidevoiceDesktop.receive({ call, level, pointerInside, outsideClicks, alwaysExpanded,
//                muteShortcut })
//                (any subset: what changed), evaluated by the app
//   card → app : invoke('call_controls_run' | 'call_controls_layout' | 'call_controls_drag' | 'call_controls_ready')
//
// The card's page (the web build's call-controls.html) finds it at window.__sidevoiceDesktop.host.callControls. On any
// origin but the app's own it does nothing at all.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window, "__SIDEVOICE_APP_ORIGIN__");
})(function installCallControlsBridge(win, appOrigin) {
  "use strict";
  // scheme://host rather than location.origin: the app's own pages (tauri://localhost) may have an opaque origin.
  if (win.location.protocol + "//" + win.location.host !== appOrigin) return null;
  if (win.__sidevoiceDesktop) return win.__sidevoiceDesktop;

  let state = null;
  const listeners = new Set();

  function invoke(command, args) {
    const internals = win.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== "function") return;
    Promise.resolve()
      .then(() => internals.invoke(command, args))
      .catch(() => {});
  }

  const callControls = Object.freeze({
    /** Called with the whole state, and again on every change; returns the function that stops it. */
    subscribe(listener) {
      listeners.add(listener);
      if (state) listener(state);
      else invoke("call_controls_ready");
      return () => listeners.delete(listener);
    },
    /** One of the card's commands, `{command, …arguments}`; the app checks it and carries it to the room. */
    run(command) {
      invoke("call_controls_run", { command });
    },
    /** The card's size with its margin, in CSS pixels: the app sizes and places the window to it. */
    layout(size) {
      const width = Number(size && size.width), height = Number(size && size.height);
      if (width > 0 && height > 0) invoke("call_controls_layout", { width, height });
    },
    /** Moving the card: "start", every "move", "end". The app follows the pointer itself. */
    drag(phase) {
      if (["start", "move", "end"].includes(phase)) invoke("call_controls_drag", { phase });
    },
  });

  const api = {
    host: Object.freeze({ app: "sidevoice-desktop", callControls }),
    /** The app pushes what changed; the card hears the whole state. */
    receive(patch) {
      if (!patch || typeof patch !== "object") return;
      state = Object.freeze(Object.assign({}, state, patch));
      if (!state.call) return; // nothing to show until the app has said what the call is
      for (const listener of listeners) listener(state);
    },
  };
  win.__sidevoiceDesktop = api;
  return api;
});
