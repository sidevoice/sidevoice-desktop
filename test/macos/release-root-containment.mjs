import { realpath } from "node:fs/promises";
import { isAbsolute, relative, sep } from "node:path";

async function canonicalDirectory(path, failure) {
  try {
    return await realpath(path);
  } catch {
    throw new Error(failure);
  }
}

export async function assertReleaseRootWithin(releaseRoot, expectedRoot) {
  const [canonicalReleaseRoot, canonicalExpectedRoot] = await Promise.all([
    canonicalDirectory(releaseRoot, "recorded release root cannot be resolved."),
    canonicalDirectory(expectedRoot, "smoke temp root cannot be resolved."),
  ]);
  const fromTemp = relative(canonicalExpectedRoot, canonicalReleaseRoot);
  if (fromTemp === ".." || fromTemp.startsWith(`..${sep}`) || isAbsolute(fromTemp)) {
    throw new Error("install record release root is outside the isolated test data directory.");
  }
  return canonicalReleaseRoot;
}
