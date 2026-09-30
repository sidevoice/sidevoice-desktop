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
  const { settings, debug } = await invoke("get_settings");
  $("target").value = settings.target;
  $("mute-shortcut").value = settings.muteShortcut;
  $("target").focus();
  return debug;
}

$("settings").addEventListener("submit", async (event) => {
  event.preventDefault();
  $("save").disabled = true;
  say("Guardando…");
  try {
    const result = await invoke("save_settings", {
      settings: { target: $("target").value, muteShortcut: $("mute-shortcut").value },
    });
    $("target").value = result.settings.target;
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
async function diagnostics(debug) {
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
  rows.push(["Contexto seguro", window.isSecureContext ? "sí" : "no"]);
  rows.push(["WebCrypto (emparejamiento)", window.crypto && window.crypto.subtle ? "sí" : "no"]);
  if (debug) invoke("debug_log", { line: "settings " + rows.map(([k, v]) => k + "=" + v).join(" | ") });
  const dl = $("diag");
  for (const [k, v] of rows) {
    const dt = document.createElement("dt");
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.textContent = v;
    dl.append(dt, dd);
  }
}

load()
  .then((debug) => diagnostics(debug))
  .catch((error) => say(String(error), true));

// Headset buttons: what reaches the app, refreshed every second while this window is open.
const SOURCES = {
  "remote:play": "Reproducir (botón o tecla)",
  "remote:pause": "Pausa (botón o tecla)",
  "remote:toggle": "Reproducir/pausa (botón o tecla)",
  "airpods:mute": "Gesto de silencio de los AirPods",
  "airpods:unmute": "Gesto de los AirPods: activar",
  prueba: "Prueba",
};
const ACTIONS = { mute: "micrófono silenciado", unmute: "micrófono activado" };

async function headset() {
  let report;
  try { report = await invoke("headset_report"); } catch { return; }
  const state = !report.platformSupported
    ? "En este sistema la app no escucha los botones (lo hace la propia página)."
    : report.inCall ? "En llamada: los botones actúan sobre el micrófono."
    : report.testing ? "Escuchando botones (prueba)… pulsa el botón de tu auricular."
    : "Fuera de llamada: no se escuchan botones.";
  const gesture = report.platformSupported
    ? (report.muteGestureApi ? " Gesto de silencio de AirPods: disponible (macOS 14+)." : " Gesto de silencio de AirPods: no disponible en esta versión de macOS.")
    : "";
  $("headset-state").textContent = state + gesture;
  $("headset-test").disabled = !report.platformSupported || report.testing;
  const list = $("headset-events");
  list.replaceChildren(...report.events.map((event) => {
    const li = document.createElement("li");
    const time = document.createElement("time");
    time.textContent = new Date(event.at).toLocaleTimeString();
    li.append(time, (SOURCES[event.source] || event.source) + " → " + (ACTIONS[event.action] || event.action));
    return li;
  }));
}
$("headset-test").addEventListener("click", () => invoke("headset_test").then(headset));
headset();
setInterval(headset, 1000);
