const assert = require("node:assert/strict");
const test = require("node:test");
const { appUrl, workspacePaths } = require("../next/paths.cjs");
const { workspaceApi } = require("../next/rpc.cjs");

test("the workspace proxy admits pinned pages, assets and build data only", () => {
  const routes = workspacePaths({
    entries: {
      "_app.tsx": "hash",
      "project/[projectId]/index.tsx": "hash",
      "project/[projectId]/prompts/[[...folder]].tsx": "hash",
    },
    publicAssets: ["fonts/IBMPlexMono-Regular.woff2"],
  }, "test-build");

  assert(routes.page("/project/local"));
  assert(routes.page("/project/local/prompts/folder/nested"));
  assert(routes.render("/fonts/IBMPlexMono-Regular.woff2"));
  assert(routes.render("/_next/static/chunks/pages/project/%5BprojectId%5D/index.js"));
  assert(routes.render("/_next/data/test-build/project/local.json"));
  for (const rejected of [
    "/api/auth/session",
    "/_app",
    "/project/local/private",
    "/project/%2e%2e",
    "/project/local/prompts/%2fetc",
    "/_next/data/unknown/project/local.json",
    "/_next/static/%2e%2e/BUILD_ID",
    "/fonts/missing.woff2",
  ]) assert.equal(routes.render(rejected), false, rejected);
  assert.equal(appUrl("confuse-ui://other/project/local"), null);
  assert.equal(appUrl("confuse-ui://workspace@other/project/local"), null);
});

test("tRPC keeps query dates and refuses mutations over GET or a foreign origin", async () => {
  const calls = [];
  const handle = workspaceApi({
    async request(channel, value) {
      calls.push({ channel, value });
      return { data: { timestamp: "2026-11-25T01:00:00.000Z" }, dates: ["timestamp"] };
    },
  }, { project: [], organization: [] });

  function request(procedure, input, options) {
    const url = new URL("confuse-ui://workspace/api/trpc/" + procedure);
    if (input) url.searchParams.set("input", JSON.stringify({ json: input }));
    return handle(new Request(url, options), url);
  }

  const time = await request("traces.byId", { projectId: "local", traceId: "demo" });
  assert.equal(time.status, 200);
  assert.deepEqual((await time.json()).result.data.meta.values, { timestamp: ["Date"] });
  assert.deepEqual(calls, [
    { channel: "langfuse.request", value: { procedure: "traces.byId", input: { projectId: "local", traceId: "demo" } } },
  ]);
  assert.equal((await request("traces.deleteMany", { projectId: "local", traceIds: ["demo"] })).status, 405);
  assert.equal((await request("traces.byId", { traceId: "demo" })).status, 400);
  assert.equal((await request("traces.byId", { projectId: "local" }, { headers: { origin: "https://example.test" } })).status, 403);
  assert.equal((await request("unknown.method", { projectId: "local" })).status, 501);
  assert.equal(calls.length, 1);
});
