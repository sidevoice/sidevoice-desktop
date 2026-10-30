// S0 spike launcher: starts and ends runs (the app's `s0_begin`/`s0_end`), opens the bundled room page, and sends the
// runs' logs to the paired machine over the LAN (`tools/receive-logs.mjs` there).
const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
const $ = (id) => document.getElementById(id);
const show = (id, value) => { $(id).textContent = typeof value === "string" ? value : JSON.stringify(value, null, 2); };

async function refresh() {
  try {
    const status = await invoke("s0_status");
    // A mode left from a run this process no longer has (the app was killed) is no run.
    if (!status.run) localStorage.removeItem("s0.mode");
    show("status", status);
    $("open").disabled = $("end").disabled = !status.run;
  } catch (error) { show("status", `status failed: ${error}`); }
  try {
    const runs = await invoke("s0_runs");
    show("runs", runs.length ? runs.map((r) => `${r.id}  ${Object.entries(r.files).map(([f, n]) => `${f} ${n} B`).join(", ")}`).join("\n") : "no runs yet");
  } catch (error) { show("runs", `runs failed: ${error}`); }
}

async function begin(mode) {
  show("status", `starting run ${mode}…`);
  try {
    const every = Math.max(10, Number($("every").value) || 60);
    await invoke("s0_begin", { mode, syntheticEvery: every, device: { ua: navigator.userAgent, screen: [screen.width, screen.height] } });
    localStorage.setItem("s0.mode", mode);
    location.href = "/voice/";
  } catch (error) {
    show("status", `run ${mode} did not start: ${error}`);
  }
}

$("run-n").onclick = () => begin("N");
$("run-p").onclick = () => begin("P");
$("open").onclick = () => { location.href = "/voice/"; };
$("end").onclick = async () => { await invoke("s0_end").catch(() => {}); localStorage.removeItem("s0.mode"); refresh(); };

$("url").value = localStorage.getItem("s0.results-url") || "";
$("send").onclick = async () => {
  const base = $("url").value.trim().replace(/\/+$/, "");
  if (!base) return;
  localStorage.setItem("s0.results-url", base);
  const lines = [];
  try {
    for (const run of await invoke("s0_runs")) {
      const files = await invoke("s0_read", { run: run.id });
      for (const [name, text] of Object.entries(files)) {
        const res = await fetch(`${base}/s0/${encodeURIComponent(run.id)}/${encodeURIComponent(name)}`, { method: "POST", body: text, headers: { "content-type": "text/plain" } });
        lines.push(`${run.id}/${name}: ${res.status}`);
      }
    }
  } catch (error) { lines.push(`failed: ${error}`); }
  show("runs", lines.join("\n") || "nothing to send");
};

refresh();
setInterval(refresh, 5000);
