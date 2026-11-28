const { spawn } = require("node:child_process");
const readline = require("node:readline");

class Backend {
  constructor(executable, directory) {
    this.nextId = 0;
    this.pending = new Map();
    this.streams = new Map();
    this.child = spawn(executable, ["desktop", directory], {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true
    });
    this.ready = new Promise((resolve, reject) => {
      this.resolveReady = resolve;
      this.rejectReady = reject;
      this.readyTimer = setTimeout(() => reject(new Error("Rust backend did not start")), 10_000);
    });
    this.ready.catch(() => {});
    this.child.stdin.on("error", () => {});
    this.child.stderr.on("data", () => {});
    const lines = readline.createInterface({ input: this.child.stdout, crlfDelay: Infinity });
    lines.on("line", line => this.receive(line));
    this.child.once("error", error => this.closed(error));
    this.child.once("close", () => this.closed(new Error("Rust backend stopped")));
  }

  receive(line) {
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      return;
    }
    if (message.ready === true) {
      clearTimeout(this.readyTimer);
      this.resolveReady();
      return;
    }
    if (message.stream) {
      const stream = this.streams.get(message.id);
      if (stream) stream.receive(message);
      return;
    }
    const pending = this.pending.get(message.id);
    if (!pending) return;
    clearTimeout(pending.timer);
    this.pending.delete(message.id);
    if (message.error) pending.reject(new Error(message.error));
    else pending.resolve(message.result);
  }

  closed(error) {
    clearTimeout(this.readyTimer);
    this.rejectReady(error);
    for (const { timer, reject } of this.pending.values()) {
      clearTimeout(timer);
      reject(error);
    }
    this.pending.clear();
    for (const stream of this.streams.values()) stream.fail(error);
  }

  async request(channel, payload) {
    await this.ready;
    const id = ++this.nextId;
    if (this.child.exitCode !== null || !this.child.stdin.writable) throw new Error("Rust backend stopped");
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error("Backend request timed out"));
      }, 35_000);
      this.pending.set(id, { resolve, reject, timer });
      this.child.stdin.write(JSON.stringify({ id, channel, payload }) + "\n", error => {
        if (!error) return;
        clearTimeout(timer);
        this.pending.delete(id);
        reject(error);
      });
    });
  }

  async playground(payload, signal) {
    await this.ready;
    if (signal?.aborted) throw signal.reason;
    if (this.child.exitCode !== null || !this.child.stdin.writable) throw new Error("Rust backend stopped");
    const id = ++this.nextId;
    const encoder = new TextEncoder();
    return new Promise((resolve, reject) => {
      let controller;
      let started = false;
      const cleanup = () => {
        this.streams.delete(id);
        signal?.removeEventListener("abort", abort);
        clearTimeout(deadline);
      };
      const cancel = () => {
        cleanup();
        if (this.child.stdin.writable) {
          this.child.stdin.write(JSON.stringify({ id: ++this.nextId, channel: "langfuse.cancel-playground", payload: { id } }) + "\n", () => {});
        }
      };
      const fail = error => {
        if (!this.streams.has(id)) return;
        if (started) controller.error(error);
        else reject(error);
        cancel();
      };
      const abort = () => fail(signal.reason || new Error("Playground request canceled"));
      const body = new ReadableStream({
        start(stream) { controller = stream; },
        cancel,
      });
      const deadline = setTimeout(() => fail(new Error("Playground request timed out")), 125_000);
      this.streams.set(id, {
        fail,
        receive(message) {
          if (message.stream === "start" && !started) {
            started = true;
            resolve(new Response(body, {
              status: message.status,
              headers: { "content-type": message.contentType, "cache-control": "no-store" },
            }));
          } else if (message.stream === "chunk" && started) {
            controller.enqueue(encoder.encode(message.chunk));
          } else if (message.stream === "end" && started) {
            controller.close();
            cleanup();
          } else if (message.stream === "error") {
            fail(new Error(message.message));
          }
        },
      });
      signal?.addEventListener("abort", abort, { once: true });
      this.child.stdin.write(JSON.stringify({ id, channel: "langfuse.playground", payload }) + "\n", error => {
        if (error) fail(error);
      });
    });
  }

  async stop() {
    if (this.child.exitCode !== null || !this.child.pid) return;
    let timeout;
    const closed = new Promise(resolve => this.child.once("close", resolve));
    const deadline = new Promise(resolve => {
      timeout = setTimeout(() => {
        this.child.kill("SIGKILL");
        resolve();
      }, 4_000);
    });
    const shutdown = this.request("desktop.quit", {}).catch(() => {});
    await Promise.race([shutdown, closed, deadline]);
    this.child.stdin.end();
    await Promise.race([closed, deadline]);
    clearTimeout(timeout);
  }
}
module.exports = { Backend };
