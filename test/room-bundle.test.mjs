// The vendored room's native worker (ui/voice-browser/native-worker.js, built from sidevoice-web) and this
// app's bridge (bridge/desktop-bridge.js) in one page context, with only Tauri's IPC faked: the bundle the app ships
// and the bridge it injects speak the same contract. CI's macOS job runs the same flow in the real app
// (test/fixtures/room-flow.js).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const bridge = readFileSync(new URL("../bridge/desktop-bridge.js", import.meta.url), "utf8");
const worker = readFileSync(new URL("../ui/voice-browser/native-worker.js", import.meta.url), "utf8");

function room(answers) {
  const calls = [];
  const page = vm.createContext({
    module: { exports: {} },
    location: { protocol: "tauri:", host: "localhost" },
    setTimeout, clearTimeout, setInterval: () => 0, clearInterval() {}, performance,
    __TAURI_INTERNALS__: {
      invoke(cmd, args, options) {
        calls.push([cmd, args, options]);
        const answer = answers[cmd];
        return typeof answer === "function" ? answer(args, options) : Promise.resolve(answer);
      },
    },
  });
  vm.runInContext(bridge, page);
  page.module.exports(page, "tauri://localhost", "native");
  vm.runInContext(worker, page);
  return { page, calls, workers: page.sidevoiceNativeWorkers };
}

function talk(worker, message, last = ["ready", "result", "done", "error"]) {
  return new Promise((resolve) => {
    const seen = [];
    worker.onmessage = ({ data }) => {
      seen.push(data);
      if (last.includes(data.type)) resolve(seen);
    };
    worker.postMessage(message);
  });
}

function audio(rate, samples) {
  const buffer = new ArrayBuffer(4 + samples.length * 4);
  new DataView(buffer).setUint32(0, rate, true);
  new Float32Array(buffer, 4).set(samples);
  return buffer;
}

test("the vendored native worker installs, transcribes and speaks through this bridge", async () => {
  const { calls, workers } = room({
    engine_installed: [],
    engine_install: null,
    engine_progress: null,
    engine_transcribe: " hola ",
    engine_synthesize: () => Promise.resolve(audio(24000, [0.5, -0.5])),
  });
  assert.equal(workers.available(), true, "the bundle finds the bridge's nativeEngine");

  const stt = workers.transcription();
  const loaded = await talk(stt, { id: 1, type: "load", model: "whisper-small", engine: "sherpa-onnx", accelerator: "coreml" });
  assert.equal(loaded.at(-1).type, "ready");
  const install = calls.find(([cmd]) => cmd === "engine_install")[1];
  assert.deepEqual([install.model, install.engine], ["whisper-small", "sherpa-onnx"]);
  assert.match(install.job, /^install-/);

  const heard = await talk(stt, { id: 2, type: "transcribe", audio: new Float32Array(1600).buffer, language: "es" });
  assert.equal(heard.at(-1).result.text, "hola");
  const [, body, options] = calls.find(([cmd]) => cmd === "engine_transcribe");
  assert.equal(body.byteLength, 1600 * 4, "the samples, as raw bytes");
  assert.equal(JSON.stringify(options.headers), JSON.stringify({
    "x-model": "whisper-small", "x-engine": "sherpa-onnx", "x-accelerator": "coreml", "x-language": "es", "x-sample-rate": "16000",
  }));

  const tts = workers.voice();
  const spoken = await talk(tts, { id: 3, type: "speak", model: "kokoro-82m-v1.0", engine: "sherpa-onnx", accelerator: "cpu",
    text: "Hola. Esto es una prueba.", voice: "ef_dora", speed: 1 }, ["done", "error"]);
  const chunks = spoken.filter((m) => m.type === "audio");
  assert.equal(spoken.at(-1).type, "done");
  assert.ok(chunks.length >= 1 && chunks.every((c) => c.sampleRate === 24000 && c.samples.length === 2));
  const synth = calls.find(([cmd]) => cmd === "engine_synthesize")[1];
  assert.deepEqual([synth.model, synth.engine, synth.accelerator, synth.voice], ["kokoro-82m-v1.0", "sherpa-onnx", "cpu", "ef_dora"]);
});

test("a build already on disk is not installed again", async () => {
  const { calls, workers } = room({ engine_installed: [{ model: "whisper-tiny", engine: "sherpa-onnx" }] });
  const loaded = await talk(workers.transcription(), { id: 1, type: "load", model: "whisper-tiny", engine: "sherpa-onnx", accelerator: "cpu" });
  assert.equal(loaded.at(-1).runtime.cached, true);
  assert.ok(!calls.some(([cmd]) => cmd === "engine_install"));
});

test("an app refusal reaches the room as its message", async () => {
  const refusal = { key: "not_installed", model: "whisper-tiny", engine: "sherpa-onnx", message: "whisper-tiny on sherpa-onnx is not downloaded yet." };
  const { workers } = room({ engine_installed: [], engine_install: () => Promise.reject(refusal) });
  const failed = await talk(workers.transcription(), { id: 1, type: "load", model: "whisper-tiny", engine: "sherpa-onnx", accelerator: "cpu" });
  assert.equal(failed.at(-1).type, "error");
  assert.equal(failed.at(-1).error, refusal.message);
});
