// The settings / first-run window. Local page (bundled with the app); talks to the app through
// the `get_settings` and `save_settings` commands, which only this window is granted.
"use strict";

const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
const $ = (id) => document.getElementById(id);

function say(text, isError) {
  $("message").textContent = text;
  $("message").className = isError ? "error" : "";
}

async function load() {
  const { settings, firstRun } = await invoke("get_settings");
  $("room-url").value = settings.roomUrl;
  $("mute-shortcut").value = settings.muteShortcut;
  if (!firstRun) {
    $("heading").textContent = "Ajustes de Sidevoice";
    $("save").textContent = "Guardar";
  }
  $("room-url").focus();
  $("room-url").select();
}

$("settings").addEventListener("submit", async (event) => {
  event.preventDefault();
  $("save").disabled = true;
  say("Guardando…");
  try {
    const result = await invoke("save_settings", {
      settings: { roomUrl: $("room-url").value, muteShortcut: $("mute-shortcut").value },
    });
    $("room-url").value = result.settings.roomUrl;
    // The app opens (or reloads) the room itself; a shortcut that could not be registered is not fatal.
    say(result.warning || "Guardado.", !!result.warning);
  } catch (error) {
    say(String(error), true);
  } finally {
    $("save").disabled = false;
  }
});

// What this webview offers the in-browser models. The room page runs in the same engine, so this
// is what it will find too (docs/MODELS.md).
async function diagnostics() {
  const rows = [];
  let gpu = "no (usará WebAssembly en la CPU)";
  try {
    const adapter = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
    if (adapter) gpu = adapter.features && adapter.features.has("shader-f16") ? "sí (con fp16)" : "sí";
  } catch { /* stays "no" */ }
  rows.push(["WebGPU", gpu]);
  rows.push(["WebAssembly", typeof WebAssembly === "object" ? "sí" : "no"]);
  rows.push(["Micrófono (API)", navigator.mediaDevices && navigator.mediaDevices.getUserMedia ? "sí" : "no"]);
  try {
    const info = await invoke("get_settings");
    rows.push(["Versión", info.appVersion]);
  } catch { /* optional */ }
  const dl = $("diag");
  for (const [k, v] of rows) {
    const dt = document.createElement("dt");
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.textContent = v;
    dl.append(dt, dd);
  }
}

load().catch((error) => say(String(error), true));
diagnostics();
