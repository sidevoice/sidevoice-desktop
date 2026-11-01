// Unit tests of desktop/core/js/no-context-menu.js against a fake window.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../desktop/core/js/no-context-menu.js", import.meta.url), "utf8");

function press({ editable = false, selected = "" } = {}) {
  const module = { exports: {} };
  vm.runInNewContext(source, { module });
  let listener;
  const win = {
    document: { addEventListener: (type, fn) => type === "contextmenu" && (listener = fn) },
    getSelection: () => selected,
  };
  module.exports(win);
  let prevented = false;
  listener({ target: { closest: () => editable }, preventDefault: () => (prevented = true) });
  return prevented;
}

test("blocks the default menu on plain page content", () => assert.equal(press(), true));
test("keeps the native menu in text fields", () => assert.equal(press({ editable: true }), false));
test("keeps the native menu on selected text", () => assert.equal(press({ selected: "hola" }), false));
