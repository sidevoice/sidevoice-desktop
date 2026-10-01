// Keyed messages for the settings window (AGENTS.md): one bundle per language in i18n/<language>.js, English the
// fallback for a missing key or a language without a bundle; the system's language when there is a bundle for it.
// Only the texts written since that rule use it; the window's older literals await sidevoice/sidevoice-web#17.
"use strict";

const i18n = (() => {
  const bundles = window.sidevoiceMessages || {};
  const wanted = navigator.languages && navigator.languages.length ? navigator.languages : [navigator.language || "en"];
  const language = wanted.map((tag) => String(tag).toLowerCase().split("-")[0]).find((code) => bundles[code]) || "en";
  /** The message for `key` with its `{name}` placeholders filled from `params`. */
  function t(key, params) {
    const own = bundles[language] && bundles[language][key];
    const text = own !== undefined ? own : bundles.en && bundles.en[key] !== undefined ? bundles.en[key] : key;
    return text.replace(/\{(\w+)\}/g, (_, name) => (params && params[name] !== undefined ? String(params[name]) : ""));
  }
  return { language, t };
})();
