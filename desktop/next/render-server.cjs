const http = require("node:http");
const { randomBytes, timingSafeEqual } = require("node:crypto");
const { createRequire } = require("node:module");
const path = require("node:path");
const { readFile } = require("node:fs/promises");
const { workspacePaths } = require("./paths.cjs");

const parent = process.parentPort;
if (!parent) throw new Error("The workspace renderer requires Electron UtilityProcess");

parent.once("message", async ({ data }) => {
  const { web, credential } = data;
  try {
    const next = createRequire(path.join(web, "package.json"))("next");
    const catalog = JSON.parse(await readFile(path.join(web, "confuse-upstream.json"), "utf8"));
    const buildID = (await readFile(path.join(web, ".next", "BUILD_ID"), "utf8")).trim();
    const allowed = workspacePaths(catalog, buildID);
    const app = next({ dev: false, dir: web, quiet: true });
    await app.prepare();
    const handle = app.getRequestHandler();
    const secret = Buffer.from(credential, "utf8");
    const server = http.createServer(async (request, response) => {
      const supplied = Buffer.from(request.headers["x-confuse-render-token"] ?? "", "utf8");
      if (supplied.length !== secret.length || !timingSafeEqual(supplied, secret)) {
        response.writeHead(403).end();
        return;
      }
      delete request.headers["x-confuse-render-token"];
      const target = request.url ?? "";
      const pathname = target.split("?", 1)[0];
      if (target.length > 32_768 || !allowed.render(pathname) || !["GET", "HEAD"].includes(request.method)) {
        response.writeHead(404).end();
        return;
      }
      const nonce = randomBytes(18).toString("base64");
      const policy = [
        "default-src 'self'",
        "script-src 'self' 'nonce-" + nonce + "' 'wasm-unsafe-eval'",
        "style-src 'self' 'unsafe-inline'",
        "img-src 'self' data: blob:",
        "font-src 'self'",
        "connect-src 'self'",
        "worker-src 'self' blob:",
        "media-src 'self' blob: data:",
        "frame-ancestors 'none'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'self'",
      ].join("; ");
      request.headers["x-confuse-nonce"] = nonce;
      request.headers["content-security-policy"] = policy;
      response.setHeader("Content-Security-Policy", policy);
      response.setHeader("X-Content-Type-Options", "nosniff");
      response.setHeader("Referrer-Policy", "no-referrer");
      if (pathname === "/") {
        response.writeHead(307, { Location: "/project/local" }).end();
        return;
      }
      try {
        await handle(request, response);
      } catch {
        if (!response.headersSent) response.writeHead(500);
        response.end();
      }
    });
    server.requestTimeout = 30_000;
    server.headersTimeout = 5_000;
    server.on("error", error => parent.postMessage({ error: error.message }));
    parent.on("message", ({ data: command }) => {
      if (command === "stop") {
        server.close(() => process.exit(0));
        server.closeAllConnections();
      }
    });
    server.listen(0, "127.0.0.1", () => {
      parent.postMessage({ port: server.address().port });
    });
  } catch (error) {
    parent.postMessage({ error: error.message });
    process.exitCode = 1;
  }
});
