#!/usr/bin/env node
// S0 spike: reads one run's logs (app.jsonl, holder.jsonl, system.jsonl, all timestamped on the phone with the wall
// clock `t`) and judges each locked interval of run N by the §F.1 rules, then the run:
//
// - native: the holder's own samples (`holder-tick`, its own timestamps) at its 5 s cadence, never more than
//   HOLDER_MAX_GAP_MS apart (the interval's edges included), every sample with capture and render advancing; and this
//   process's 5 s tick. A sample of the holder marking the native audio interrupted (iOS: an interruption that began,
//   a media-services reset, an engine configuration change, a route change that left the engine stopped; Android: a
//   capture or render error) makes the interval invalid: inconclusive, "native audio interrupted", never fail.
// - page: its 1 s timer through the bridge, never more than PAGE_MAX_GAP_MS apart; a gap past PAGE_STOPPED_MS is the
//   page stopped.
// - heartbeat: only confirmed exchanges count. One is a frame from the core received after the page sent one since the
//   core's previous frame (the core's voice-ping answered and its next frame received, or a frame of ours answered).
//   Confirmed exchanges must never be more than HEARTBEAT_BUDGET_MS apart across the interval, its edges included
//   (core rust/server/call/heartbeat.rs: a 15 s interval, two misses). None at all is a stopped heartbeat. A socket
//   that closes and opens again within the interval (LAN loss and reconnect) has its outage excluded; one that does
//   not open again is a stopped heartbeat.
// - turns: every synthetic turn emitted at least TURN_DEADLINE_MS before the interval ends must have its reply said
//   within TURN_DEADLINE_MS, unless the page dropped it because the call was muted (the mute step). An interval of
//   MIN_LOCKED_MS or more needs at least one such round trip unless all its turns were dropped as muted.
// - duration: a pass needs at least one valid interval of MIN_LOCKED_MS (§F.1: 10 min locked).
//
// Outcome: fail when, in a valid interval with native alive, the page or the heartbeat stopped; otherwise inconclusive
// for a harness or setup defect (interrupted or dead native audio, page media called, no 10-min interval, no reply
// while everything else lived); pass when every interval holds. The timeline printed with it is what decides.
//
//   node analyze.mjs <run directory>
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const MIN_LOCKED_MS = 10 * 60 * 1000;
export const HOLDER_MAX_GAP_MS = 15 * 1000;
export const NATIVE_MAX_GAP_MS = 15 * 1000;
export const PAGE_MAX_GAP_MS = 5 * 1000;
export const PAGE_STOPPED_MS = 30 * 1000;
export const HEARTBEAT_BUDGET_MS = 30 * 1000;
export const TURN_DEADLINE_MS = 120 * 1000;

/** The largest step between `times` within [from, to], the edges included. */
function maxGap(times, from, to) {
  const points = [from, ...times.filter((t) => t > from && t < to).sort((a, b) => a - b), to];
  let max = 0;
  for (let i = 1; i < points.length; i++) max = Math.max(max, points[i] - points[i - 1]);
  return max;
}

/** Locked intervals: Android screen off → user present; iOS protected data unavailable → available. */
function lockedIntervals(all, end) {
  const locks = [];
  let open = null;
  for (const e of all) {
    if (e.src !== "system" && e.src !== "holder") continue;
    if ((e.k === "screen-off" || e.k === "locked") && open === null) open = e.t;
    if (e.k === "unlocked" && open !== null) { locks.push([open, e.t]); open = null; }
  }
  if (open !== null) locks.push([open, end]);
  return locks;
}

/** Times of confirmed heartbeat exchanges: a core frame received after the page sent one since the core's last. */
function confirmedExchanges(all) {
  const times = [];
  let sentSinceLastIn = false;
  for (const e of all) {
    if (e.src !== "page") continue;
    if (e.k === "ws-out") sentSinceLastIn = true;
    else if (e.k === "ws-in") {
      if (sentSinceLastIn) times.push(e.t);
      sentSinceLastIn = false;
    } else if (e.k === "ws-close" || e.k === "ws-open") sentSinceLastIn = false;
  }
  return times;
}

/** Spans a socket was down and came back, within [from, to]; and whether one went down for good. */
function outages(all, from, to) {
  const spans = [];
  let down = null, unrecovered = false;
  for (const e of all) {
    if (e.src !== "page" || e.t < from || e.t > to) continue;
    if (e.k === "ws-close" && down === null) down = e.t;
    if (e.k === "ws-open" && down !== null) { spans.push([down, e.t]); down = null; }
  }
  if (down !== null) unrecovered = true;
  return { spans, unrecovered, downSince: down };
}

/** The largest step between confirmed exchanges in [from, to], outside the recovered outages. */
function heartbeatGap(exchanges, from, to, spans, downSince) {
  const pieces = [];
  let start = from;
  for (const [a, b] of spans) { pieces.push([start, a]); start = b; }
  pieces.push([start, downSince ?? to]);
  let max = 0;
  for (const [a, b] of pieces) if (b > a) max = Math.max(max, maxGap(exchanges, a, b));
  return max;
}

export function analyze(records) {
  const all = [...records].filter((e) => Number.isFinite(e.t)).sort((a, b) => a.t - b.t);
  if (!all.length) return { outcome: "inconclusive", why: ["no logs"], intervals: [], synthetic: [] };
  const begin = all.find((e) => e.k === "run-begin") ?? {};
  const mode = begin.mode ?? "?";
  const tEnd = all.at(-1).t;
  const of = (src, k) => all.filter((e) => e.src === src && (!k || e.k === k));
  const locks = lockedIntervals(all, tEnd);
  const pageTicks = of("page", "tick").map((e) => e.t);
  const nativeTicks = of("native", "native-tick").map((e) => e.t);
  const holderTicks = of("holder", "holder-tick");
  const interruptions = all.filter((e) => e.src === "holder" && ["native-audio-interrupted", "capture-error", "render-error"].includes(e.k));
  const exchanges = confirmedExchanges(all);
  const media = of("page", "page-media");

  // Synthetic turns: emitted natively → received by the page → sent → reply in → said (logged natively).
  const synthetic = new Map();
  for (const e of all) {
    if (e.k === "synthetic-emitted") synthetic.set(e.seq, { seq: e.seq, emitted: e.t });
    else if (e.k === "synthetic-received" && synthetic.has(e.seq)) synthetic.get(e.seq).received = e.t;
    else if (e.k === "synthetic-dropped" && synthetic.has(e.seq)) synthetic.get(e.seq).dropped = e.reason;
  }
  const echo = (re, e, field) => {
    const m = re.exec(e.text ?? "");
    if (m && synthetic.has(Number(m[1]))) synthetic.get(Number(m[1]))[field] ??= e.t;
  };
  for (const e of of("page", "ws-out")) echo(/S0 synthetic turn (\d+)/, e, "sent");
  for (const e of of("page", "ws-in")) echo(/S0 echo (\d+)/, e, "reply");
  for (const e of of("page", "say")) echo(/S0 echo (\d+)/, e, "said");

  const intervals = locks.map(([a, b]) => {
    const inside = (t) => t >= a && t <= b;
    const ht = holderTicks.filter((e) => inside(e.t));
    const { spans, unrecovered, downSince } = outages(all, a, b);
    const turns = [...synthetic.values()].filter((x) => inside(x.emitted));
    const muted = turns.filter((x) => x.dropped === "muted");
    const due = turns.filter((x) => x.dropped !== "muted" && x.emitted <= b - TURN_DEADLINE_MS);
    const late = due.filter((x) => !(x.said && x.said - x.emitted <= TURN_DEADLINE_MS));
    const row = {
      from: a, to: b, seconds: Math.round((b - a) / 1000),
      interrupted: interruptions.filter((e) => inside(e.t)).map((e) => e.cause ?? e.k),
      holder_samples: ht.length,
      holder_max_gap_s: maxGap(ht.map((e) => e.t), a, b) / 1000,
      audio_advancing: ht.length > 0 && ht.every((e) => e.capture_delta > 0 && e.render_delta > 0),
      capture_silent_samples: ht.filter((e) => !(e.capture_peak > 0)).length,
      native_max_gap_s: maxGap(nativeTicks, a, b) / 1000,
      page_ticks: pageTicks.filter(inside).length,
      page_max_gap_s: maxGap(pageTicks, a, b) / 1000,
      heartbeat_exchanges: exchanges.filter(inside).length,
      heartbeat_max_gap_s: heartbeatGap(exchanges, a, b, spans, downSince) / 1000,
      reconnects: spans.length,
      socket_down_for_good: unrecovered,
      synthetic: turns.length, muted_turns: muted.length, turns_due: due.length, turns_late: late.length,
      round_trips: due.length - late.length,
    };
    row.qualifying = b - a >= MIN_LOCKED_MS;
    row.native_alive = row.holder_samples > 0 && row.holder_max_gap_s * 1000 <= HOLDER_MAX_GAP_MS && row.audio_advancing &&
      row.native_max_gap_s * 1000 <= NATIVE_MAX_GAP_MS;
    row.page_alive = row.page_max_gap_s * 1000 <= PAGE_MAX_GAP_MS;
    row.page_stopped = row.page_max_gap_s * 1000 > PAGE_STOPPED_MS;
    row.heartbeat_alive = !unrecovered && row.heartbeat_max_gap_s * 1000 <= HEARTBEAT_BUDGET_MS;
    const allMuted = turns.length > 0 && muted.length === turns.length;
    row.turns_ok = late.length === 0 && (!row.qualifying || allMuted || row.round_trips > 0);
    if (row.interrupted.length) row.verdict = "inconclusive: native audio interrupted";
    else if (!row.native_alive) row.verdict = "inconclusive: native audio or the app's clock not alive";
    else if (row.page_stopped) row.verdict = "fail: page timer stopped";
    else if (!row.heartbeat_alive) row.verdict = "fail: heartbeat stopped";
    else if (!row.page_alive) row.verdict = "inconclusive: page timer irregular";
    else if (!row.turns_ok) row.verdict = "inconclusive: turns without a reply (responder, pairing or routing)";
    else row.verdict = "pass";
    return row;
  });

  let outcome = "inconclusive";
  const why = [];
  if (mode !== "N") why.push("not run N (P is the control: recorded, never decides)");
  else if (media.length) why.push("page media was called in run N (harness defect)");
  else if (!locks.length) why.push("no locked interval in the logs");
  else if (intervals.some((v) => v.verdict.startsWith("fail"))) {
    outcome = "fail";
    why.push(...new Set(intervals.filter((v) => v.verdict.startsWith("fail")).map((v) => v.verdict.slice(6))));
  } else if (intervals.some((v) => v.interrupted.length)) why.push("native audio interrupted");
  else if (intervals.some((v) => v.verdict !== "pass")) why.push(...new Set(intervals.filter((v) => v.verdict !== "pass").map((v) => v.verdict.slice(14))));
  else if (!intervals.some((v) => v.qualifying)) why.push(`no locked interval of ${MIN_LOCKED_MS / 60000} min`);
  else { outcome = "pass"; why.push("every component alive across every locked interval"); }
  return { outcome, why, mode, begin, intervals, synthetic: [...synthetic.values()], media, all };
}

export function readRun(dir) {
  const read = (name) => {
    const file = path.join(dir, name);
    if (!existsSync(file)) return [];
    return readFileSync(file, "utf8").split("\n").filter(Boolean).flatMap((line) => { try { return [JSON.parse(line)]; } catch { return []; } });
  };
  return [...read("app.jsonl"), ...read("holder.jsonl"), ...read("system.jsonl")];
}

function print(dir) {
  const result = analyze(readRun(dir));
  const { all, begin, intervals, synthetic, media } = result;
  if (!all?.length) { console.error(`no logs in ${dir}`); process.exit(1); }
  const t0 = all[0].t, tEnd = all.at(-1).t;
  const at = (t) => `+${((t - t0) / 1000).toFixed(1)}s`;
  console.log(`run ${begin.id ?? path.basename(dir)}  mode ${result.mode}  ${new Date(t0).toISOString()}  ${((tEnd - t0) / 60000).toFixed(1)} min`);
  const holderStart = all.find((e) => e.k === "holder-start");
  console.log(`device: ${JSON.stringify(begin.device ?? {})}${holderStart ? `  holder: ${holderStart.device ?? ""} ${holderStart.system ?? holderStart.sdk ?? ""}` : ""}`);
  console.log(`page loads: ${all.filter((e) => e.k === "page-load").length}  page media calls: ${media.length}`);
  console.log(`\nlocked intervals: ${intervals.length}`);
  for (const v of intervals) console.log(JSON.stringify({ ...v, from: at(v.from), to: at(v.to) }));
  console.log(`\nsynthetic turns (s from native emit): seq emitted | received | sent | reply | said`);
  for (const x of synthetic) {
    const d = (t) => (t ? ((t - x.emitted) / 1000).toFixed(1) : "-");
    console.log(`  ${x.seq} ${at(x.emitted)} | ${d(x.received)} | ${d(x.sent)} | ${d(x.reply)} | ${d(x.said)}${x.dropped ? ` dropped:${x.dropped}` : ""}`);
  }
  console.log("\ntimeline (system, holder events, sockets, lifecycle, loads):");
  for (const e of all) {
    const notable = e.src === "system" || (e.src === "holder" && e.k !== "holder-tick") ||
      ["ws-open", "ws-close", "ws-error", "ws-out-failed", "page-load", "lifecycle", "run-begin", "run-end", "window-create", "holder-started", "holder-stopped"].includes(e.k);
    if (notable) {
      const { t, mono, up, src, k, perf, wall, page, mode: _m, ua, ...rest } = e;
      console.log(`  ${at(t)} ${src} ${k} ${JSON.stringify(rest).slice(0, 160)}`);
    }
  }
  console.log(`\nsuggested outcome: ${result.outcome} — ${result.why.join("; ")}`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  if (!process.argv[2]) { console.error("usage: node analyze.mjs <run directory>"); process.exit(2); }
  print(process.argv[2]);
}
