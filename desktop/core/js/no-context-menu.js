// Sidevoice desktop: no default WebView context menu ("Reload", "Back", …). Injected into every window of the app
// (not the pages of other origins the room never leaves for). Text fields and selected text keep the native menu
// (copy / paste); the page's own menus (Radix ContextMenu) open from their own handler, which this one never stops.
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window);
})(function installNoContextMenu(win) {
  "use strict";
  const EDITABLE = 'input, textarea, [contenteditable=""], [contenteditable="true"]';
  win.document.addEventListener("contextmenu", (event) => {
    const target = event.target;
    if (target && typeof target.closest === "function" && target.closest(EDITABLE)) return;
    const selection = win.getSelection && win.getSelection();
    if (selection && String(selection).length > 0) return;
    event.preventDefault();
  });
});
