import { lstat, readFile, readdir } from "node:fs/promises";
import { resolve } from "node:path";

const ENTRY_LIMIT = 128;
const RECORD_SIZE_LIMIT = 64 * 1024;

function count(value) {
  return value > ENTRY_LIMIT ? `${ENTRY_LIMIT}+` : String(value);
}

function diagnostic(shape) {
  return `expected one complete committed pair release before commit (entries=${count(shape.entries)}; matches=${count(shape.matches)}; releases=${count(shape.releases)}; temporary=${count(shape.temporary)}; symlinks=${count(shape.symlinks)}; other=${count(shape.other)}; invalid_records=${count(shape.invalidRecords)})`;
}

/** Find the immutable release produced by stage(); its .tmp-* directory is renamed before install quiesces. */
export async function findCommittedPairRelease(releasesDirectory, expectedPairId) {
  const shape = { entries: 0, matches: 0, releases: 0, temporary: 0, symlinks: 0, other: 0, invalidRecords: 0 };
  let entries;
  try { entries = await readdir(releasesDirectory); }
  catch { throw new Error(diagnostic(shape)); }
  shape.entries = entries.length;
  if (entries.length > ENTRY_LIMIT || typeof expectedPairId !== "string" || expectedPairId.length > 512) {
    throw new Error(diagnostic(shape));
  }

  let match = null;
  for (const name of entries) {
    if (name.includes(".tmp-")) { shape.temporary++; continue; }
    const directory = resolve(releasesDirectory, name);
    let directoryInfo;
    try { directoryInfo = await lstat(directory); }
    catch { shape.other++; continue; }
    if (directoryInfo.isSymbolicLink()) { shape.symlinks++; continue; }
    if (!directoryInfo.isDirectory()) { shape.other++; continue; }
    shape.releases++;

    const recordPath = resolve(directory, "release.json");
    try {
      const recordInfo = await lstat(recordPath);
      if (!recordInfo.isFile() || recordInfo.isSymbolicLink() || recordInfo.size > RECORD_SIZE_LIMIT) {
        shape.invalidRecords++;
        continue;
      }
      const release = JSON.parse(await readFile(recordPath, "utf8"));
      if (release?.id !== name) {
        shape.invalidRecords++;
        continue;
      }
      if (release?.pair_id === expectedPairId) {
        shape.matches++;
        match = directory;
      }
    } catch { shape.invalidRecords++; }
  }

  if (shape.matches !== 1 || shape.temporary || shape.symlinks || shape.other || shape.invalidRecords) {
    throw new Error(diagnostic(shape));
  }
  return match;
}
