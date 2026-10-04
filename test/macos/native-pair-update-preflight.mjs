import { readdir } from "node:fs/promises";

export async function assertFreshAppConfig(configDirectory) {
  let entries;
  try {
    entries = await readdir(configDirectory);
  } catch (error) {
    if (error.code === "ENOENT") return;
    throw new Error("cannot inspect runner app configuration.");
  }
  if (entries.length !== 0) throw new Error("runner app configuration is not fresh.");
}
