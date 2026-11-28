const { randomBytes } = require("node:crypto");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const { appUrl, workspacePaths } = require("./paths.cjs");

class WorkspaceRenderer {
  constructor({ utilityProcess, web }) {
    this.credential = randomBytes(32).toString("base64url");
    this.catalog = JSON.parse(readFileSync(path.join(web, "confuse-upstream.json"), "utf8"));
    const buildId = readFileSync(path.join(web, ".next", "BUILD_ID"), "utf8").trim();
    this.paths = workspacePaths(this.catalog, buildId);
    this.running = false;
    this.child = utilityProcess.fork(path.join(__dirname, "render-server.cjs"), [], {
      cwd: web,
      env: {
        NODE_ENV: "production",
        NEXT_TELEMETRY_DISABLED: "1",
        TZ: process.env.TZ || Intl.DateTimeFormat().resolvedOptions().timeZone,
      },
      serviceName: "Confuse Workspace",
      stdio: "inherit",
    });
    this.exited = new Promise(resolve => this.child.once("exit", () => {
      this.running = false;
      resolve();
    }));
    this.ready = new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("Next.js workspace did not start")), 25_000);
      this.child.once("spawn", () => this.child.postMessage({ web, credential: this.credential }));
      this.child.once("message", message => {
        clearTimeout(timer);
        if (Number.isInteger(message?.port) && message.port > 0 && message.port <= 65_535) {
          this.running = true;
          resolve("http://127.0.0.1:" + message.port);
        } else {
          reject(new Error(message?.error || "Next.js workspace returned an invalid port"));
        }
      });
      this.child.once("exit", () => {
        clearTimeout(timer);
        reject(new Error("Next.js workspace stopped"));
      });
    });
  }

  async respond(request) {
    const parsed = appUrl(request.url);
    if (!parsed || !this.paths.render(parsed.pathname) || !["GET", "HEAD"].includes(request.method)) {
      return new Response("Not found", { status: 404 });
    }
    const privateOrigin = await this.ready;
    if (!this.running) return new Response("Workspace renderer stopped", { status: 503 });
    const headers = new Headers({ "x-confuse-render-token": this.credential });
    for (const name of ["accept", "if-none-match", "if-modified-since", "range", "x-nextjs-data"]) {
      if (request.headers.has(name)) headers.set(name, request.headers.get(name));
    }
    try {
      const response = await fetch(privateOrigin + parsed.pathname + parsed.search, {
        method: request.method, headers, signal: request.signal, redirect: "manual",
      });
      const forwarded = new Headers(response.headers);
      // Node fetch decompresses the body. Return its actual stream to Electron.
      forwarded.delete("content-encoding");
      forwarded.delete("content-length");
      forwarded.delete("transfer-encoding");
      const location = forwarded.get("location");
      if (location) {
        const redirected = new URL(location, request.url);
        if (!appUrl(redirected.href) || !this.paths.page(redirected.pathname)) {
          await response.body?.cancel();
          return new Response("Invalid workspace redirect", { status: 400 });
        }
        forwarded.set("location", redirected.href);
      }
      return new Response(response.body, { headers: forwarded, status: response.status });
    } catch (error) {
      if (request.signal.aborted) throw error;
      return new Response("Workspace renderer unavailable", { status: 503 });
    }
  }

  async stop() {
    if (!this.child.pid) return;
    if (this.running) this.child.postMessage("stop");
    else this.child.kill();
    const timer = setTimeout(() => this.child.kill(), 3_000);
    await this.exited;
    clearTimeout(timer);
  }
}

module.exports = { WorkspaceRenderer };
