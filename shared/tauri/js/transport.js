// The Tauri transport of the page's side of the shared commands (sidevoice-shared-tauri/ACL.md): how a page reaches the
// plugin's commands, and how the messages of a channel it opened reach the page. It is all the shared members
// (sidevoice-shared-tauri/js/members.js) know of Tauri, and they know it only through what this returns.
//
// This file is one function expression, `(win) => transport`, which the app that embeds the plugin evaluates (docs/BRIDGE.md ->
// "Parts"). The transport is made for a page, once, and reads `win.__TAURI_INTERNALS__` at each use: the app may be
// mid-reload, or its capability may not match yet.
//
//   call(command, args, options)   the plugin's command, as a promise; rejects when the page has no host
//   send(command, args)            the same, without an answer, and never an error (the app may be mid-reload)
//   channel(onmessage, options)    what a command that takes a channel is given in its place (`{ordered: false}`: each
//                                  message as it comes, for a caller that numbers its own and asks for what it missed)
//   own.call, own.send             the same for the app's own commands, which are not the plugin's
(function (win) {
  "use strict";
  /** The plugin's commands are invoked under its name; the app's own keep theirs. */
  const PLUGIN = "plugin:sidevoice-shared-tauri|";

  const host = () => {
    const internals = win.__TAURI_INTERNALS__;
    return internals && typeof internals.invoke === "function" ? internals : null;
  };

  /** The two ways to ask for a command of one namespace. */
  function commands(prefix) {
    return {
      call(command, args, options) {
        const internals = host();
        if (!internals) return Promise.reject(new Error("no desktop host"));
        return Promise.resolve().then(() => internals.invoke(prefix + command, args, options));
      },
      send(command, args) {
        const internals = host();
        if (!internals) return;
        // The app may be mid-reload or the capability may not match yet: never break the page for it.
        Promise.resolve()
          .then(() => internals.invoke(prefix + command, args))
          .catch(() => {});
      },
    };
  }

  /** The most messages an ordered channel holds while one before them has not come: past it, that one is given up on and
   *  the channel goes on from the next it has, instead of growing for good behind a message that was lost. */
  const MAX_WAITING = 1024;

  /** A Tauri channel without the API's module: the app sends over it `{index, message}` (and `{index, end}` last), and the
   *  messages are handed to `onmessage` in index order, as the API's `Channel` does. What this returns is what a command
   *  that takes a channel is given in its place. With `{ordered: false}` each message is handed over as it comes and none
   *  waits for the ones before it: for a caller that numbers its own messages and asks the app for any it missed (the call
   *  link, members.js), which is then the one place a gap is seen, and a lost message holds nothing back. */
  function channel(onmessage, options) {
    const internals = win.__TAURI_INTERNALS__;
    const ordered = options?.ordered !== false;
    const waiting = new Map();
    let next = 0;
    let received = 0;
    let end = null;
    const hand = (message) => {
      try { onmessage(message); } catch (_) { /* a listener's error is the page's own */ }
    };
    const id = internals.transformCallback((event) => {
      if (event && "end" in event) end = event.index;
      else if (event && !ordered) { received++; hand(event.message); }
      else if (event && event.index >= next) waiting.set(event.index, event.message);
      if (waiting.size > MAX_WAITING) next = Math.min(...waiting.keys());
      while (waiting.has(next)) {
        const message = waiting.get(next);
        waiting.delete(next);
        next++;
        hand(message);
      }
      const done = end !== null && (ordered ? next >= end : received >= end);
      if (done && typeof internals.unregisterCallback === "function") internals.unregisterCallback(id);
    });
    return `__CHANNEL__:${id}`;
  }

  return Object.freeze({ ...commands(PLUGIN), channel, own: Object.freeze(commands("")) });
})
