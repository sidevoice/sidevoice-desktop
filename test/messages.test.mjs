// The app's overlay of the web's texts (shared/tauri/i18n, mobile/tauri/i18n/<os>) against the web interface the app bundles
// (web.pin.json, ui/voice/): an overlay key the pinned web no longer has would silently stop applying, so it fails here,
// naming the stale key. Every catalogue of the web has every key of English's (the web types them so), so a key the
// catalogues carry is English's.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";

const assets = new URL("../ui/voice/assets/", import.meta.url);
const web = readdirSync(assets).filter((name) => name.endsWith(".js")).map((name) => readFileSync(new URL(name, assets), "utf8")).join("\n");
const count = (key) => web.split(`${JSON.stringify(key)}:`).length - 1;

const LANGUAGES = ["en", "de", "es", "fr", "it", "ja", "pl", "zh-Hans"];
const TABLES = ["shared/tauri/i18n", "mobile/tauri/i18n/ios", "mobile/tauri/i18n/android"];
const table = (dir, language) => JSON.parse(readFileSync(new URL(`../${dir}/${language}.json`, import.meta.url), "utf8"));

test("the web interface at the pin carries every language's catalogue", () => {
  assert.ok(count("devices.mic") >= LANGUAGES.length, "the pinned web's catalogues were not found in ui/voice");
});

for (const dir of TABLES) {
  test(`${dir}: every language is there with English's keys, each key one the pinned web has, and each a text`, () => {
    const english = table(dir, "en");
    const catalogues = count("devices.mic");
    for (const language of LANGUAGES) {
      const texts = table(dir, language);
      assert.deepEqual(Object.keys(texts).sort(), Object.keys(english).sort(), language);
      for (const [key, text] of Object.entries(texts)) {
        assert.ok(typeof text === "string" && text !== "", `${language}: ${key}`);
        assert.equal(count(key), catalogues, `${dir}/${language}.json: "${key}" is not a key of the pinned web's English catalogue (stale?)`);
      }
    }
  });
}

test("no overlay text says browser, where the app is not one", () => {
  for (const dir of TABLES) assert.doesNotMatch(table(dir, "en")["devices.speakerFixed"] ?? "", /browser/i);
});
