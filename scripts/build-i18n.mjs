import { readFileSync } from "node:fs";

// Build CLI messages follow the system language when a bundle exists, with English fallback.
const english = JSON.parse(readFileSync(new URL("./i18n/en.json", import.meta.url), "utf8"));
const language = Intl.DateTimeFormat().resolvedOptions().locale.split("-")[0];
let localized = {};
try { localized = JSON.parse(readFileSync(new URL(`./i18n/${language}.json`, import.meta.url), "utf8")); }
catch { /* A language without a bundle uses English. */ }
export function t(key, params = {}) {
  const message = localized[key] ?? english[key];
  if (typeof message !== "string") throw new Error(`Missing build message key: ${key}`);
  return message.replace(/\{([a-zA-Z]+)\}/g, (_, name) => String(params[name] ?? `{${name}}`));
}
