// Fixtures for analyze.mjs: a healthy run N passes, and each false pass the review found does not.
//   node --test tools/analyze.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { analyze } from "./analyze.mjs";

const T0 = 1_800_000_000_000;
const MIN = 60_000;

/**
 * A run N: 1 min unlocked, then each lock in `locks` ([start, length] ms from T0), then 1 min unlocked. The holder
 * samples every `holderEvery`, the page ticks every second, the core pings every 15 s and the page answers, a
 * synthetic turn every minute gets its reply 5 s later. `edit(records)` breaks it.
 */
function run({ locks = [[MIN, 11 * MIN]], holderEvery = 5000, edit = (r) => r } = {}) {
  const end = Math.max(...locks.map(([a, l]) => a + l)) + MIN;
  const r = [{ t: T0, src: "native", k: "run-begin", mode: "N", id: "N-fixture" }];
  for (const [a, l] of locks) {
    r.push({ t: T0 + a, src: "system", k: "screen-off" }, { t: T0 + a + l, src: "system", k: "unlocked" });
  }
  for (let t = 1000; t < end; t += 1000) r.push({ t: T0 + t, src: "page", k: "tick" });
  for (let t = 5000; t < end; t += 5000) r.push({ t: T0 + t, src: "native", k: "native-tick" });
  for (let t = holderEvery; t < end; t += holderEvery) {
    r.push({ t: T0 + t, src: "holder", k: "holder-tick", capture_delta: 80000, render_delta: 80000, capture_peak: 120 });
  }
  r.push({ t: T0 + 500, src: "page", k: "ws-open", id: 1 });
  for (let t = 15000; t < end; t += 15000) {
    r.push({ t: T0 + t, src: "page", k: "ws-in", id: 1, type: "voice-ping" }, { t: T0 + t + 20, src: "page", k: "ws-out", id: 1, type: "voice-pong" });
  }
  for (let seq = 1, t = 30_000; t < end - 10_000; seq++, t += MIN) {
    r.push({ t: T0 + t, src: "native", k: "synthetic-emitted", seq },
      { t: T0 + t + 10, src: "page", k: "synthetic-received", seq },
      { t: T0 + t + 20, src: "page", k: "ws-out", id: 1, type: "voice-user-turn", text: `S0 synthetic turn ${seq}, emitted…` },
      { t: T0 + t + 4000, src: "page", k: "ws-in", id: 1, type: "voice-reply", text: `S0 echo ${seq}` },
      { t: T0 + t + 4010, src: "page", k: "say", text: `S0 echo ${seq}` });
  }
  return analyze(edit(r));
}
const inLock = (e, a = MIN, l = 11 * MIN) => e.t > T0 + a && e.t < T0 + a + l;

test("a healthy run N passes", () => {
  const result = run();
  assert.equal(result.outcome, "pass", result.why.join("; "));
});

test("zero heartbeats never pass", () => {
  const result = run({ edit: (r) => r.filter((e) => !(e.type === "voice-ping" || e.type === "voice-pong")) });
  // The synthetic turns still exchange frames once a minute: past the 30 s budget.
  assert.notEqual(result.outcome, "pass");
  const quiet = run({ edit: (r) => r.filter((e) => !(e.k === "ws-in" || e.k === "ws-out")) });
  assert.notEqual(quiet.outcome, "pass");
});

test("equal ping and pong counts with one exchange in ten minutes do not pass", () => {
  let kept = 0;
  const result = run({ edit: (r) => r.filter((e) => !(inLock(e) && (e.type === "voice-ping" || e.type === "voice-pong" || e.k === "synthetic-emitted")) || kept++ < 2) });
  assert.equal(result.outcome, "fail", result.why.join("; "));
});

test("pongs the core never followed do not count", () => {
  // Pings and pongs keep coming in the log, but nothing from the core after the pongs: attempts, not exchanges.
  const result = run({ edit: (r) => r.filter((e) => !(inLock(e) && e.k === "ws-in" && e.type !== "voice-ping"))
    .map((e) => (inLock(e) && e.type === "voice-ping" ? { ...e, k: "ws-out" } : e)) });
  assert.notEqual(result.outcome, "pass");
});

test("one holder sample in ten minutes does not pass", () => {
  let kept = 0;
  const result = run({ edit: (r) => r.filter((e) => !(inLock(e) && e.k === "holder-tick") || kept++ < 1) });
  assert.equal(result.outcome, "inconclusive");
  assert.match(result.why.join(), /native audio/);
});

test("a five-second lock does not pass", () => {
  const result = run({ locks: [[MIN, 5000]] });
  assert.equal(result.outcome, "inconclusive");
  assert.match(result.why.join(), /no locked interval of 10 min/);
});

test("a window with no capture does not pass", () => {
  let hit = false;
  const result = run({ edit: (r) => r.map((e) => (!hit && inLock(e) && e.k === "holder-tick" && (hit = true) ? { ...e, capture_delta: 0 } : e)) });
  assert.equal(result.outcome, "inconclusive");
});

test("a muted lock without turns does not block a pass", () => {
  // 11 min locked and unmuted, then 2 min locked and muted: its turns are dropped as muted, nothing said.
  const result = run({
    locks: [[MIN, 11 * MIN], [13 * MIN, 2 * MIN]],
    edit: (r) => r.flatMap((e) => {
      if (!inLock(e, 13 * MIN, 2 * MIN) || e.k === "tick" || e.src !== "page" && e.k !== "synthetic-emitted") return [e];
      if (e.k === "synthetic-emitted") return [e];
      if (e.k === "synthetic-received") return [e, { t: e.t + 1, src: "page", k: "synthetic-dropped", seq: e.seq, reason: "muted" }];
      if (e.type === "voice-user-turn" || e.type === "voice-reply" || e.k === "say") return [];
      return [e];
    }),
  });
  assert.equal(result.outcome, "pass", result.why.join("; "));
});

test("an iOS interruption is inconclusive, never fail", () => {
  const result = run({ edit: (r) => [...r.filter((e) => !(e.t > T0 + 3 * MIN && e.t < T0 + 8 * MIN && (e.k === "tick" || e.k === "holder-tick"))),
    { t: T0 + 3 * MIN, src: "holder", k: "native-audio-interrupted", cause: "audio-interruption" }] });
  assert.equal(result.outcome, "inconclusive");
  assert.match(result.why.join(), /native audio interrupted/);
});

test("the page timer stopping while native lives fails", () => {
  const result = run({ edit: (r) => r.filter((e) => !(e.t > T0 + 3 * MIN && e.t < T0 + 8 * MIN && e.src === "page")) });
  assert.equal(result.outcome, "fail");
});

/** Frames between `lost` and `back` never arrive; the socket closes at `closed`, socket 2 opens at `back` and carries
 *  everything after. */
const lanLoss = (lost, closed, back) => (r) => [
  ...r.filter((e) => !(e.t > lost && e.t < back && (e.k === "ws-in" || e.k === "ws-out")))
    .map((e) => (e.t >= back && e.id === 1 ? { ...e, id: 2 } : e)),
  { t: closed, src: "page", k: "ws-close", id: 1, code: 1006 }, { t: back, src: "page", k: "ws-open", id: 2 },
];

test("a LAN loss the page recovers from within the lock still passes", () => {
  const result = run({ edit: lanLoss(T0 + 4 * MIN, T0 + 4 * MIN, T0 + 5 * MIN) });
  assert.equal(result.outcome, "pass", result.why.join("; "));
});

test("a socket that never comes back fails", () => {
  const result = run({ edit: (r) => [...r.filter((e) => !(e.t > T0 + 4 * MIN && (e.k === "ws-in" || e.k === "ws-out" || e.k === "say"))),
    { t: T0 + 4 * MIN, src: "page", k: "ws-close", id: 1, code: 1006 }] });
  assert.equal(result.outcome, "fail");
});

test("repro 1: a muted lock whose socket is down almost all of it never passes", () => {
  // 11 min locked and muted; the socket closes at +1 s and a new one opens at +659 s: no confirmed exchange.
  // The lock starts half a second off the core's 15 s beat, so no exchange sits on its edge.
  const a = MIN + 500, b = a + 11 * MIN;
  const result = run({
    locks: [[a, 11 * MIN]],
    edit: (r) => [
      ...r.filter((e) => !(e.t > T0 + a && e.t < T0 + b && (e.k === "ws-in" || e.k === "ws-out" || e.k === "say")))
        .flatMap((e) => (e.k === "synthetic-received" && e.t > T0 + a && e.t < T0 + b
          ? [e, { t: e.t + 1, src: "page", k: "synthetic-dropped", seq: e.seq, reason: "muted" }] : [e]))
        .map((e) => (e.t >= T0 + a + 659_000 && e.id === 1 ? { ...e, id: 2 } : e)),
      { t: T0 + a + 1000, src: "page", k: "ws-close", id: 1, code: 1006 },
      { t: T0 + a + 659_000, src: "page", k: "ws-open", id: 2 },
    ],
  });
  assert.notEqual(result.outcome, "pass");
  assert.equal(result.intervals[0].heartbeat_exchanges, 0);
});

test("repro 2: our frames on one socket and the core's on another are no exchange", () => {
  // Socket 2 opens beside socket 1; while locked the page answers on socket 2 only, and the core speaks on socket 1.
  const result = run({
    locks: [[MIN + 500, 11 * MIN]],
    edit: (r) => [
      ...r.map((e) => (inLock(e, MIN + 500) && e.k === "ws-out" ? { ...e, id: 2 } : e)),
      { t: T0 + 600, src: "page", k: "ws-open", id: 2 },
    ],
  });
  assert.notEqual(result.outcome, "pass");
  // Only the pong sent on socket 1 just before the lock, answered by the core on socket 1 just after it.
  assert.ok(result.intervals[0].heartbeat_exchanges <= 1);
});

test("repro 3: a 60 s LAN loss whose close arrives 45 s in, then reconnects, passes", () => {
  const lost = T0 + 4 * MIN + 1000;
  const result = run({ edit: lanLoss(lost, lost + 45_000, lost + 62_000) });
  assert.equal(result.outcome, "pass", result.why.join("; "));
});

test("a reconnect longer than the LAN-loss step is not a pass", () => {
  const lost = T0 + 4 * MIN + 1000;
  const result = run({ edit: lanLoss(lost, lost + 45_000, lost + 200_000) });
  assert.equal(result.outcome, "inconclusive");
  assert.match(result.why.join(), /longer than the LAN-loss step/);
});

test("repro 4: chained socket opens without exchanges hide nothing", () => {
  // A muted lock from 60 s to 720 s; one exchange at 60.002 s; socket 1 closes at 61 s; sockets 2–6 open at 180, 300,
  // 420, 540 and 660 s and each closes 1 s later; socket 7 opens at 719 s. Nothing else is exchanged.
  const a = MIN, b = 12 * MIN;
  const result = run({
    edit: (r) => {
      const kept = r.filter((e) => !(e.t >= T0 + a && e.t <= T0 + b && (e.k === "ws-in" || e.k === "ws-out" || e.k === "say")))
        .flatMap((e) => (e.k === "synthetic-received" && e.t > T0 + a && e.t < T0 + b
          ? [e, { t: e.t + 1, src: "page", k: "synthetic-dropped", seq: e.seq, reason: "muted" }] : [e]))
        .filter((e) => !(e.t > T0 + b && e.id === 1));
      kept.push({ t: T0 + a - 5000, src: "page", k: "ws-out", id: 1, type: "voice-pong" },
        { t: T0 + a + 2, src: "page", k: "ws-in", id: 1, type: "voice-ping" },
        { t: T0 + 61_000, src: "page", k: "ws-close", id: 1, code: 1006 });
      [180, 300, 420, 540, 660].forEach((s, i) => kept.push(
        { t: T0 + s * 1000, src: "page", k: "ws-open", id: i + 2 }, { t: T0 + s * 1000 + 1000, src: "page", k: "ws-close", id: i + 2, code: 1006 }));
      kept.push({ t: T0 + 719_000, src: "page", k: "ws-open", id: 7 });
      return kept;
    },
  });
  assert.notEqual(result.outcome, "pass", result.why.join("; "));
});

test("repro 5: exchanges on an old socket do not mask a dead current socket", () => {
  // The healthy run, with socket 2 opening at 70 s while every exchange stays on socket 1.
  const result = run({ edit: (r) => [...r, { t: T0 + 70_000, src: "page", k: "ws-open", id: 2 }] });
  assert.notEqual(result.outcome, "pass", result.why.join("; "));
});

test("repro 6: the current socket is the one opened last, whatever its id", () => {
  // The healthy run, with socket 0 opening at 70 s while every exchange stays on socket 1.
  const result = run({ edit: (r) => [...r, { t: T0 + 70_000, src: "page", k: "ws-open", id: 0 }] });
  assert.notEqual(result.outcome, "pass", result.why.join("; "));
});

test("repro 7: a recovery at the unlock instant does not count toward the lock", () => {
  // The lock ends at 720 s. The LAN goes at 631 s; socket 2 opens at 705 s and its first exchange is at 720 s.
  const result = run({ edit: lanLoss(T0 + 631_000, T0 + 660_000, T0 + 705_000) });
  assert.equal(result.intervals[0].to, T0 + 12 * MIN);
  assert.notEqual(result.outcome, "pass", result.why.join("; "));
});

test("repro 8: a send the socket refused confirms nothing", () => {
  // While locked, every frame the page sends is followed by ws-out-failed for that socket.
  const result = run({ edit: (r) => r.flatMap((e) => (inLock(e) && e.k === "ws-out"
    ? [e, { t: e.t + 1, src: "page", k: "ws-out-failed", id: e.id }] : [e])) });
  assert.notEqual(result.outcome, "pass", result.why.join("; "));
});
