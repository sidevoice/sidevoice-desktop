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
    r.push({ t: T0 + t, src: "page", k: "ws-in", type: "voice-ping" }, { t: T0 + t + 20, src: "page", k: "ws-out", type: "voice-pong" });
  }
  for (let seq = 1, t = 30_000; t < end - 10_000; seq++, t += MIN) {
    r.push({ t: T0 + t, src: "native", k: "synthetic-emitted", seq },
      { t: T0 + t + 10, src: "page", k: "synthetic-received", seq },
      { t: T0 + t + 20, src: "page", k: "ws-out", type: "voice-user-turn", text: `S0 synthetic turn ${seq}, emitted…` },
      { t: T0 + t + 4000, src: "page", k: "ws-in", type: "voice-reply", text: `S0 echo ${seq}` },
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

test("a LAN loss the page recovers from within the lock still passes", () => {
  const down = [T0 + 4 * MIN, T0 + 5 * MIN];
  const result = run({ edit: (r) => [...r.filter((e) => !(e.t > down[0] && e.t < down[1] && (e.k === "ws-in" || e.k === "ws-out"))),
    { t: down[0], src: "page", k: "ws-close", code: 1006 }, { t: down[1], src: "page", k: "ws-open", id: 2 }] });
  assert.equal(result.outcome, "pass", result.why.join("; "));
});

test("a socket that never comes back fails", () => {
  const result = run({ edit: (r) => [...r.filter((e) => !(e.t > T0 + 4 * MIN && (e.k === "ws-in" || e.k === "ws-out" || e.k === "say"))),
    { t: T0 + 4 * MIN, src: "page", k: "ws-close", code: 1006 }] });
  assert.equal(result.outcome, "fail");
});
