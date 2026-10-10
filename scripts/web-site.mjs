// Puts a built Sidevoice web site into the app's ui/ (`installSite`): each directory the site serves goes to ui/ at
// the same path, and every directory the previous site put there and this one does not have is removed, so ui/ holds
// exactly the pinned build and the app's own files. Which directories were the site's is kept in
// ui/.web-site.json; nothing else in ui/ is touched.
import { cpSync, existsSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";

const RECORD = ".web-site.json";

/** Copies the directories of `site` into `ui`, after removing those the last site copied; answers their names. */
export function installSite(site, ui) {
  const recorded = path.join(ui, RECORD);
  const previous = existsSync(recorded) ? JSON.parse(readFileSync(recorded, "utf8")).directories : [];
  const served = readdirSync(site, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name);
  for (const name of new Set([...previous, ...served])) {
    // Only plain names: the record is ui/'s own, but it never reaches above ui/.
    if (name && !name.includes("/") && !name.includes("\\") && name !== "." && name !== "..") {
      rmSync(path.join(ui, name), { recursive: true, force: true });
    }
  }
  for (const name of served) cpSync(path.join(site, name), path.join(ui, name), { recursive: true });
  writeFileSync(recorded, JSON.stringify({ directories: served }, null, 2) + "\n");
  return served;
}
