#!/usr/bin/env node
// S0 spike: reads one run's logs (app.jsonl, holder.jsonl, system.jsonl, all timestamped on the phone with the wall
// clock `t`) and prints, per locked interval, whether each component stayed alive: the page's 1 s timer (through the
// bridge), this process's 5 s tick, the native capture and render, the socket and its heartbeat (voice-ping in,
// voice-pong out), and the synthetic turns' round trips. It ends with a suggested outcome by the §F.1 rules; the
// suggestion is a heuristic over the logs, and the timeline is what decides.
//
//   node analyze.mjs <run directory>
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

const dir = process.argv[2];
if (!dir) { console.error("usage: node analyze.mjs <run directory>"); process.exit(2); }
const read = (name) => {
  const file = path.join(dir, name);
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf8").split("\n").filter(Boolean).flatMap((line) => { try { return [JSON.parse(line)]; } catch { return []; } });
};
const all = [...read("app.jsonl"), ...read("holder.jsonl"), ...read("system.jsonl")].sort((a, b) => a.t - b.t);
if (!all.length) { console.error(`no logs in ${dir}`); process.exit(1); }
const begin = all.find((e) => e.k === "run-begin") ?? {};
const mode = begin.mode ?? "?";
const t0 = all[0].t, tEnd = all.at(-1).t;
const at = (t) => `+${((t - t0) / 1000).toFixed(1)}s`;
const of = (src, k) => all.filter((e) => e.src === src && (!k || e.k === k));

// Locked intervals: Android screen off → user present; iOS protected data unavailable → available.
const locks = [];
let open = null;
for (const e of all.filter((x) => x.src === "system" || x.src === "holder")) {
  if ((e.k === "screen-off" || e.k === "locked") && open === null) open = e.t;
  if (e.k === "unlocked" && open !== null) { locks.push([open, e.t]); open = null; }
}
if (open !== null) locks.push([open, tEnd]);

const gaps = (times, from, to) => {
  const points = [from, ...times.filter((t) => t >= from && t <= to), to];
  let max = 0;
  for (let i = 1; i < points.length; i++) max = Math.max(max, points[i] - points[i - 1]);
  return max;
};
const pageTicks = of("page", "tick").map((e) => e.t);
const nativeTicks = of("native", "native-tick").map((e) => e.t);
const holderTicks = of("holder", "holder-tick");
const wsIn = of("page", "ws-in"), wsOut = of("page", "ws-out");
const media = of("page", "page-media");
const loads = of("page", "page-load");

console.log(`run ${begin.id ?? path.basename(dir)}  mode ${mode}  ${new Date(t0).toISOString()}  ${((tEnd - t0) / 60000).toFixed(1)} min`);
const holderStart = all.find((e) => e.k === "holder-start");
console.log(`device: ${JSON.stringify(begin.device ?? {})}${holderStart ? `  holder: ${holderStart.device ?? ""} ${holderStart.system ?? holderStart.sdk ?? ""}` : ""}`);
console.log(`page loads: ${loads.length}  page media calls: ${media.length}${media.length ? " " + JSON.stringify(media.map((m) => m.api)) : ""}`);

// Synthetic turns: emitted natively → received by the page → sent to the room → reply in → said (logged natively).
const synthetic = new Map();
for (const e of all) {
  const seq = e.seq;
  if (e.k === "synthetic-emitted") synthetic.set(seq, { seq, emitted: e.t });
  else if (e.k === "synthetic-received" && synthetic.has(seq)) synthetic.get(seq).received = e.t;
  else if (e.k === "synthetic-dropped" && synthetic.has(seq)) synthetic.get(seq).dropped = e.reason;
}
for (const e of wsOut) {
  const m = /S0 synthetic turn (\d+)/.exec(e.text ?? "");
  if (m && synthetic.has(Number(m[1]))) synthetic.get(Number(m[1])).sent ??= e.t;
}
for (const e of wsIn) {
  const m = /S0 echo (\d+)/.exec(e.text ?? "");
  if (m && synthetic.has(Number(m[1]))) synthetic.get(Number(m[1])).reply ??= e.t;
}
for (const e of of("page", "say")) {
  const m = /S0 echo (\d+)/.exec(e.text ?? "");
  if (m && synthetic.has(Number(m[1]))) synthetic.get(Number(m[1])).said ??= e.t;
}
const lockedAt = (t) => locks.some(([a, b]) => t >= a && t <= b);

console.log(`\nlocked intervals: ${locks.length}`);
const verdicts = [];
for (const [a, b] of locks) {
  const s = (b - a) / 1000;
  const inside = (t) => t >= a && t <= b;
  const ht = holderTicks.filter((e) => inside(e.t));
  const pings = wsIn.filter((e) => e.type === "voice-ping" && inside(e.t)).length;
  const pongs = wsOut.filter((e) => e.type === "voice-pong" && inside(e.t)).length;
  const closes = of("page", "ws-close").filter((e) => inside(e.t));
  const opens = of("page", "ws-open").filter((e) => inside(e.t));
  // A close the page recovered from (LAN loss and reconnect) is followed by an open within the interval.
  const unrecovered = closes.filter((c) => !opens.some((o) => o.t > c.t));
  const turns = [...synthetic.values()].filter((x) => inside(x.emitted));
  const roundTrips = turns.filter((x) => x.said && inside(x.said));
  const pageGap = gaps(pageTicks, a, b) / 1000, nativeGap = gaps(nativeTicks, a, b) / 1000;
  const row = {
    from: at(a), seconds: Math.round(s),
    page_ticks: `${pageTicks.filter(inside).length}/${Math.floor(s)}`, page_max_gap_s: pageGap,
    native_ticks: nativeTicks.filter(inside).length, native_max_gap_s: nativeGap,
    holder_ticks: ht.length,
    capture_alive: ht.length ? ht.every((e) => e.capture_delta > 0) : null,
    capture_silent_ticks: ht.filter((e) => !(e.capture_peak > 0)).length,
    render_alive: ht.length ? ht.every((e) => e.render_delta > 0) : null,
    pings_in: pings, pongs_out: pongs, socket_closes: closes.map((c) => `${at(c.t)} ${c.code}`),
    socket_reopens: opens.length, unrecovered_closes: unrecovered.length,
    synthetic_after_lock: turns.length, round_trips_while_locked: roundTrips.length,
  };
  console.log(JSON.stringify(row));
  verdicts.push(row);
}

console.log(`\nsynthetic turns (s from native emit): seq emitted | received | sent | reply | said`);
for (const x of synthetic.values()) {
  const d = (t) => (t ? ((t - x.emitted) / 1000).toFixed(1) : "-");
  console.log(`  ${x.seq} ${at(x.emitted)}${lockedAt(x.emitted) ? " locked" : ""} | ${d(x.received)} | ${d(x.sent)} | ${d(x.reply)} | ${d(x.said)}${x.dropped ? ` dropped:${x.dropped}` : ""}`);
}

console.log("\ntimeline (system, holder errors, sockets, lifecycle, loads):");
for (const e of all) {
  const notable = e.src === "system" || /error|start|stop|task-removed/.test(e.k) && e.src === "holder" ||
    ["ws-open", "ws-close", "ws-error", "page-load", "lifecycle", "run-begin", "run-end", "window-create", "holder-started", "holder-stopped"].includes(e.k);
  if (notable) {
    const { t, mono, up, src, k, perf, wall, page, mode: _m, ua, ...rest } = e;
    console.log(`  ${at(t)} ${src} ${k} ${JSON.stringify(rest).slice(0, 160)}`);
  }
}

// §F.1: pass = every component alive across the lock in N; fail = page execution or heartbeat stops in N while native
// continues; inconclusive = a harness or setup defect.
let outcome = "inconclusive", why = [];
if (mode !== "N") why.push("not run N (P is the control: recorded, never decides)");
else if (media.length) why.push("page media was called in run N (harness defect)");
else if (!locks.length) why.push("no locked interval in the logs");
else if (!holderTicks.length) why.push("the holder never ticked");
else if (![...synthetic.values()].some((x) => x.said)) why.push("no synthetic round trip at all (responder or pairing)");
else {
  const nativeAlive = verdicts.every((v) => v.capture_alive && v.render_alive && v.native_max_gap_s <= 15);
  const pageStopped = verdicts.some((v) => v.page_max_gap_s > 30);
  const heartbeatStopped = verdicts.some((v) => v.pings_in > v.pongs_out || v.unrecovered_closes);
  const allAlive = verdicts.every((v) => v.page_max_gap_s <= 5 && v.pings_in === v.pongs_out && !v.unrecovered_closes && v.round_trips_while_locked > 0);
  if (!nativeAlive) why.push("native audio or the app's clock did not stay alive while locked (setup)");
  else if (pageStopped || heartbeatStopped) { outcome = "fail"; why.push(pageStopped ? "page timer stopped while locked" : "heartbeat or socket stopped while locked"); }
  else if (allAlive) { outcome = "pass"; why.push("every component alive across every locked interval"); }
  else why.push("mixed: read the intervals above");
}
console.log(`\nsuggested outcome (heuristic): ${outcome} — ${why.join("; ")}`);
