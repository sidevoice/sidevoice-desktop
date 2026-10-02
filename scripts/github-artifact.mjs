import { t } from "./build-i18n.mjs";
import { createWriteStream } from "node:fs";
import { Readable, Transform } from "node:stream";
import { pipeline } from "node:stream/promises";

const API = "https://api.github.com/repos/sidevoice/sidevoice-connector";
const API_HOST = "api.github.com";

export function artifactId(url) {
  const match = /^https:\/\/api\.github\.com\/repos\/sidevoice\/sidevoice-connector\/actions\/artifacts\/([1-9][0-9]*)\/zip$/.exec(url);
  return match?.[1] ?? null;
}

function allowedUrl(value) {
  let url;
  try { url = new URL(value); }
  catch { throw new Error(t("artifact.unsafeUrl")); }
  const host = url.hostname;
  if (url.protocol !== "https:" || url.username || url.password || url.port || url.hash
      || !(host === API_HOST || host === "github.com" || host === "objects.githubusercontent.com"
        || host === "release-assets.githubusercontent.com" || host === "results-receiver.actions.githubusercontent.com"
        || host.endsWith(".blob.core.windows.net"))) {
    throw new Error(t("artifact.unsafeUrl"));
  }
  return url;
}

// Authenticate only the original API request. Redirect locations can contain signed credentials:
// neither forward Authorization nor include those URLs in errors or build evidence.
export async function githubResponse(url, { token, fetchImpl = fetch, signal } = {}) {
  let current = allowedUrl(url);
  for (let redirects = 0; redirects <= 5; redirects++) {
    const headers = { "user-agent": "sidevoice-desktop-build" };
    if (redirects === 0 && current.hostname === API_HOST) {
      if (!token) throw new Error(t("artifact.tokenRequired"));
      headers.authorization = `Bearer ${token}`;
      headers.accept = "application/vnd.github+json";
      headers["x-github-api-version"] = "2022-11-28";
    }
    let response;
    try { response = await fetchImpl(current.href, { headers, redirect: "manual", signal }); }
    catch { throw new Error(t("artifact.network")); }
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      const location = response.headers.get("location");
      await response.body?.cancel();
      if (!location) throw new Error(t("artifact.redirectMissing"));
      let redirected;
      try { redirected = new URL(location, current).href; }
      catch { throw new Error(t("artifact.unsafeUrl")); }
      current = allowedUrl(redirected);
      continue;
    }
    if (!response.ok || !response.body) {
      await response.body?.cancel();
      throw new Error(t("artifact.http", { status: response.status }));
    }
    return response;
  }
  throw new Error(t("artifact.redirectLimit"));
}

function limitBytes(maxBytes) {
  let size = 0;
  return new Transform({
    transform(chunk, _encoding, done) {
      size += chunk.length;
      done(size > maxBytes ? new Error(t("artifact.sizeLimit")) : null, chunk);
    },
    flush(done) { done(size ? null : new Error(t("artifact.empty"))); },
  });
}

export async function download(url, output, maxBytes, options = {}) {
  const response = await githubResponse(url, { signal: AbortSignal.timeout(300_000), ...options });
  if (Number(response.headers.get("content-length")) > maxBytes) {
    await response.body.cancel();
    throw new Error(t("artifact.declaredSize"));
  }
  await pipeline(Readable.fromWeb(response.body), limitBytes(maxBytes),
    createWriteStream(output, { flags: "wx", mode: 0o600 }));
}

async function apiJson(url, options) {
  const response = await githubResponse(url, { signal: AbortSignal.timeout(30_000), ...options });
  const chunks = [];
  await pipeline(Readable.fromWeb(response.body), limitBytes(1024 * 1024), async (source) => {
    for await (const chunk of source) chunks.push(chunk);
  });
  return JSON.parse(Buffer.concat(chunks));
}

export function verifyArtifactProvenance(pin, artifact, run) {
  const provenance = pin.provenance;
  const id = artifactId(pin.asset_url);
  const [workflow, ref] = provenance.workflow.split("@");
  const repositoryId = provenance.repository_id;
  if (!id || String(artifact.id) !== id || artifact.name !== provenance.artifact_name
      || artifact.expired !== false || artifact.archive_download_url !== pin.asset_url
      || artifact.workflow_run?.id !== provenance.run_id
      || String(artifact.workflow_run?.repository_id) !== repositoryId
      || String(artifact.workflow_run?.head_repository_id) !== repositoryId
      || artifact.workflow_run?.head_sha !== pin.connector_sha
      || run.id !== provenance.run_id || run.head_sha !== pin.connector_sha
      || run.repository?.full_name !== provenance.repository || String(run.repository?.id) !== repositoryId
      || String(run.head_repository?.id) !== repositoryId
      || run.path !== workflow || `refs/heads/${run.head_branch}` !== ref
      || run.run_number !== pin.build_seq || run.status !== "completed" || run.conclusion !== "success"
      || !["push", "workflow_dispatch"].includes(run.event)) {
    throw new Error(t("artifact.provenanceMismatch"));
  }
  return { artifact_id: id, run_id: run.id, connector_sha: run.head_sha };
}

export async function verifyRemoteProvenance(pin, options = {}) {
  const id = artifactId(pin.asset_url);
  if (!id) throw new Error(t("artifact.artifactUrl"));
  const artifact = await apiJson(`${API}/actions/artifacts/${id}`, options);
  const run = await apiJson(`${API}/actions/runs/${pin.provenance.run_id}`, options);
  return verifyArtifactProvenance(pin, artifact, run);
}
