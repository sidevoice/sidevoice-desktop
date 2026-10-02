import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { artifactId, download, githubResponse, verifyArtifactProvenance, verifyRemoteProvenance } from "../scripts/github-artifact.mjs";

const assetUrl = "https://api.github.com/repos/sidevoice/sidevoice-connector/actions/artifacts/456/zip";
const pin = {
  asset_url: assetUrl, connector_sha: "a".repeat(40), build_seq: 42,
  provenance: { repository: "sidevoice/sidevoice-connector", repository_id: "12345", run_id: 123,
    workflow: ".github/workflows/r4-sea.yml@refs/heads/main", artifact_name: "sidevoice-connector-macos-aarch64-r4b" },
};
function records() {
  return {
    artifact: { id: 456, name: pin.provenance.artifact_name, expired: false, archive_download_url: assetUrl,
      workflow_run: { id: 123, repository_id: 12345, head_repository_id: 12345, head_sha: pin.connector_sha } },
    run: { id: 123, head_sha: pin.connector_sha, repository: { id: 12345, full_name: pin.provenance.repository },
      head_repository: { id: 12345 }, path: ".github/workflows/r4-sea.yml", head_branch: "main", run_number: 42,
      status: "completed", conclusion: "success", event: "push" },
  };
}

test("only the real artifact-scoped download endpoint is accepted", () => {
  assert.equal(artifactId(assetUrl), "456");
  for (const url of [assetUrl.replace("/artifacts/", "/runs/123/artifacts/"), `${assetUrl}?x=1`,
    `${assetUrl}#x`, assetUrl.replace("/456/", "/0/"), assetUrl.replace("/456/", "/0456/"),
    assetUrl.replace("api.github.com", "evil.example"), assetUrl.replace("https://", "https://user@")]) {
    assert.equal(artifactId(url), null, url);
  }
});

test("the authenticated API identity must bind the artifact to the pinned successful production run", () => {
  const good = records();
  assert.equal(verifyArtifactProvenance(pin, good.artifact, good.run).artifact_id, "456");
  const mutations = [
    ({ artifact }) => { artifact.expired = true; },
    ({ artifact }) => { artifact.id++; },
    ({ artifact }) => { artifact.name = "other"; },
    ({ artifact }) => { artifact.workflow_run.id++; },
    ({ artifact }) => { artifact.workflow_run.head_sha = "b".repeat(40); },
    ({ artifact }) => { artifact.workflow_run.repository_id++; },
    ({ artifact }) => { artifact.workflow_run.head_repository_id++; },
    ({ run }) => { run.repository.id++; },
    ({ run }) => { run.head_repository.id++; },
    ({ run }) => { run.head_sha = "b".repeat(40); },
    ({ run }) => { run.head_branch = "unreviewed"; },
    ({ run }) => { run.path = ".github/workflows/other.yml"; },
    ({ run }) => { run.run_number++; },
    ({ run }) => { run.status = "in_progress"; },
    ({ run }) => { run.conclusion = "failure"; },
    ({ run }) => { run.event = "pull_request"; },
  ];
  for (const mutate of mutations) {
    const record = records(); mutate(record);
    assert.throws(() => verifyArtifactProvenance(pin, record.artifact, record.run), /provenance/);
  }
});

test("provenance uses artifact and run API endpoints, with authentication", async () => {
  const record = records(); const urls = [];
  const evidence = await verifyRemoteProvenance(pin, { token: "test-token", fetchImpl: async (url, options) => {
    assert.equal(options.headers.authorization, "Bearer test-token"); urls.push(url);
    return Response.json(url.endsWith("/artifacts/456") ? record.artifact : record.run);
  } });
  assert.equal(evidence.run_id, 123);
  assert.deepEqual(urls.map((url) => url.split("/actions/")[1]), ["artifacts/456", "runs/123"]);
});

test("API authentication is required and never forwarded to redirect storage", async () => {
  let calls = 0;
  await assert.rejects(githubResponse(assetUrl, { fetchImpl: async () => { calls++; } }), /Actions: read/);
  assert.equal(calls, 0);
  const destination = "https://productionresultssa1.blob.core.windows.net/artifacts/pinned.zip?sig=temporary";
  const response = await githubResponse(assetUrl, { token: "test-token", fetchImpl: async (url, options) => {
    assert.equal(options.redirect, "manual");
    if (calls++ === 0) {
      assert.equal(options.headers.authorization, "Bearer test-token");
      return new Response(null, { status: 302, headers: { location: destination } });
    }
    assert.equal(url, destination); assert.equal(options.headers.authorization, undefined);
    return new Response("zip bytes");
  } });
  assert.equal(await response.text(), "zip bytes");
  assert.equal(calls, 2);
});

test("redirects refuse plaintext, credentials and arbitrary hosts before any request to them", async () => {
  for (const location of ["http://api.github.com/x", "https://evil.example/x", "https://user@github.com/x",
    "https://github.com:444/x", "https://blob.core.windows.net.evil.example/x"]) {
    let calls = 0;
    await assert.rejects(githubResponse(assetUrl, { token: "test-token", fetchImpl: async () => {
      calls++; return new Response(null, { status: 302, headers: { location } });
    } }), /allowed HTTPS/);
    assert.equal(calls, 1);
  }
});

test("network diagnostics do not leak signed redirect URLs or credentials", async () => {
  await assert.rejects(githubResponse(assetUrl, { token: "private", fetchImpl: async () => {
    throw new Error("network failure with private token and ?sig=secret");
  } }), (error) => !/private|secret/.test(error.message) && /failed/.test(error.message));
});

test("download bounds the stream even without Content-Length and preserves existing files", async () => {
  const directory = await mkdtemp(join(tmpdir(), "sidevoice-download-test-"));
  try {
    const path = join(directory, "asset");
    const options = { token: "test-token", fetchImpl: async () => new Response("small") };
    await download(assetUrl, path, 5, options);
    assert.equal(await readFile(path, "utf8"), "small");
    await assert.rejects(download(assetUrl, path, 5, options), /EEXIST/);
    assert.equal(await readFile(path, "utf8"), "small");
    await assert.rejects(download(assetUrl, join(directory, "large"), 4, options), /size limit/);
    await assert.rejects(download(assetUrl, join(directory, "empty"), 5,
      { ...options, fetchImpl: async () => new Response("") }), /empty/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
