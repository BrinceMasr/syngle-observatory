// Starts the real sidecar the way the app's supervisor does and talks to it the way the app does: its control channel
// (JSON lines on stdin and stdout) and its loopback gateway (`/canvas/`, `/project.json`, `/data/*`, `POST /bridge`, `/render`).
// An evaluator never mocks it. The gateway runs without a token here (the engine's own "open" mode, which spike A's bench
// used): the token only guards the address, the work is the same.
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

/** The engine binary inside a build folder: `syngle-observatory-engine.exe` on Windows, without the suffix elsewhere. */
export function engineBinary(buildDir) {
  const name = process.platform === "win32" ? "syngle-observatory-engine.exe" : "syngle-observatory-engine";
  const file = path.join(buildDir, name);
  if (!fs.existsSync(file)) throw new Error(`no ${name} in ${buildDir}: build it with evals/build/boat.sh`);
  return file;
}

export async function startEngine({ buildDir, canvasDir = path.join(buildDir, "canvas"), dataDir = path.join(buildDir, "data") }) {
  const exe = engineBinary(buildDir);
  const child = spawn(exe, ["--root", buildDir, "--web", canvasDir, "--data", dataDir, "--port", "0"], {
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  const events = [];
  const waiters = [];
  let buffer = "";
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    let at;
    while ((at = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, at).trim();
      buffer = buffer.slice(at + 1);
      if (!line) continue;
      let event;
      try {
        event = JSON.parse(line);
      } catch {
        continue;
      }
      events.push(event);
      for (const waiter of [...waiters]) if (waiter.match(event)) (waiters.splice(waiters.indexOf(waiter), 1), waiter.resolve(event));
    }
  });
  child.stderr.on("data", () => {});
  let stopped = false;
  const stop = () => {
    if (stopped) return;
    stopped = true;
    process.removeListener("exit", stop);
    try {
      child.stdin.write('{"cmd":"quit"}\n');
    } catch {
      // The pipe is already closed.
    }
    setTimeout(() => {
      try {
        if (process.platform === "win32") spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
        else child.kill("SIGKILL");
      } catch {
        // Gone.
      }
    }, 1500).unref();
  };
  process.once("exit", stop);

  const next = (match, timeoutMs = 120000) => {
    const seen = events.find(match);
    if (seen) return Promise.resolve(seen);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("the engine did not answer in time")), timeoutMs);
      waiters.push({ match, resolve: (event) => (clearTimeout(timer), resolve(event)) });
    });
  };
  const ready = await next((e) => e.event === "ready" || e.event === "error", 30000);
  if (ready.event !== "ready") {
    stop();
    throw new Error(`the engine did not start: ${JSON.stringify(ready)}`);
  }
  const base = `http://127.0.0.1:${ready.port}`;
  let ids = 1000;
  return {
    port: ready.port,
    pid: ready.pid,
    base,
    /** Opens a dataset by address (what the view reads in place) or by path; answers the engine's own `opened` event. */
    async open({ url, path: file, name, zoom = true }) {
      const id = ++ids;
      child.stdin.write(`${JSON.stringify({ cmd: "open", id, ...(url ? { url } : { path: file }), ...(name ? { name } : {}), zoom })}\n`);
      const opened = await next((e) => e.event === "opened" && e.id === id);
      if (!opened.result?.ok) throw new Error(`the engine could not open ${url ?? file}: ${JSON.stringify(opened.result?.error ?? opened.result)}`);
      return opened.result;
    },
    async stats() {
      const id = ++ids;
      child.stdin.write(`${JSON.stringify({ cmd: "stats", id })}\n`);
      return next((e) => e.event === "stats" && e.id === id);
    },
    /** `hello` then one bridge command over `POST /bridge`, the transport the agent's map tools use; answers the lines. */
    async bridge(command) {
      const id = ++ids;
      const body = JSON.stringify({ cmd: "bridge", bridge: "command", protocolVersion: 1, id, command });
      const reply = await fetch(`${base}/bridge`, { method: "POST", headers: { "content-type": "application/json" }, body });
      return reply.json();
    },
    async hello() {
      const reply = await fetch(`${base}/bridge`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ cmd: "bridge", bridge: "hello", protocolVersion: 1 }),
      });
      return reply.json();
    },
    async render(width, height) {
      const reply = await fetch(`${base}/render?width=${width}&height=${height}`);
      return { status: reply.status, bytes: new Uint8Array(await reply.arrayBuffer()) };
    },
    async project() {
      return (await fetch(`${base}/project.json`)).json();
    },
    stop,
  };
}
