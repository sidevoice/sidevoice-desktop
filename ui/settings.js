// The settings window: diagnostics only. Local page (bundled with the app); reads the app's version through
// `get_settings`, which only this window is granted.
"use strict";

const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
const $ = (id) => document.getElementById(id);

// The mute shortcut and the call controls are set in the room's Settings (docs/BRIDGE.md); this window only diagnoses.
$("room-settings").textContent = i18n.t("roomSettings");

/** Fills a <dl> with [term, description] rows. */
function rowsInto(dl, rows) {
  for (const [k, v] of rows) {
    const dt = document.createElement("dt");
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.textContent = v;
    dl.append(dt, dd);
  }
}

// What this webview offers the room's page (it runs in the same engine). Models run in the app's native engine,
// never in the webview (below).
async function diagnostics() {
  const rows = [];
  rows.push(["Micrófono (API)", navigator.mediaDevices && navigator.mediaDevices.getUserMedia ? "sí" : "no"]);
  let debug = false;
  try {
    const info = await invoke("get_settings");
    rows.push(["Versión", info.appVersion]);
    debug = info.debug;
  } catch { /* optional */ }
  rows.push(["Contexto seguro", window.isSecureContext ? "sí" : "no"]);
  rows.push(["WebCrypto (emparejamiento)", window.crypto && window.crypto.subtle ? "sí" : "no"]);
  if (debug) invoke("debug_log", { line: "settings " + rows.map(([k, v]) => k + "=" + v).join(" | ") });
  rowsInto($("diag"), rows);
}

/** A quantity in `unit` (megabyte, gigabyte), in the window's language. */
function quantity(value, unit, digits) {
  return new Intl.NumberFormat(i18n.language, { style: "unit", unit, maximumFractionDigits: digits }).format(value);
}

/** Bytes on disk, decimal, as downloads are sized. */
function size(bytes) {
  return bytes >= 1e9 ? quantity(bytes / 1e9, "gigabyte", 1) : quantity(bytes / 1e6, "megabyte", 0);
}

// The app's native engine (docs/ENGINES.md): what this device reports to the room's page, and what is on disk.
async function engine() {
  const { t } = i18n;
  $("engine").lang = i18n.language;
  $("engine-heading").textContent = t("engine.heading");
  $("engine-hint").textContent = t("engine.hint");
  try {
    const [device, disk] = await Promise.all([invoke("engine_capabilities"), invoke("engine_on_disk")]);
    rowsInto($("engine-device"), [
      [t("engine.system"), device.os + " · " + device.arch],
      [t("engine.accelerators"), device.has.join(", ")],
      // Binary gigabytes, as memory is sold (16 GB).
      [t("engine.memory"), device.memory_mb ? quantity(device.memory_mb / 1024, "gigabyte", 1) : t("engine.unknown")],
    ]);
    const lines = [
      ...disk.engines.map((e) => t("engine.package", { engine: e.label || e.engine, version: e.version, size: size(e.bytes) })),
      ...disk.builds.map((b) => t("engine.build", {
        model: b.label || b.model, task: t("engine.task." + b.task), engine: b.engine, size: size(b.bytes),
      })),
    ];
    $("engine-downloaded").textContent = lines.length ? t("engine.downloaded") : t("engine.none");
    $("engine-builds").replaceChildren(...lines.map((text) => {
      const li = document.createElement("li");
      li.textContent = text;
      return li;
    }));
  } catch (error) {
    $("engine-downloaded").textContent = t("engine.error", { error: String((error && error.message) || error) });
  }
}

diagnostics();
engine();

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
