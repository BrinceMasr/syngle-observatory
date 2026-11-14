// Test of the engine launcher's stop (no real sidecar: a stand-in that speaks the control channel's `ready` and `quit`).
// Run: `node --test evals/lib/engine.test.mjs`. The stand-in is a script, so the test runs on POSIX only.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { startEngine } from "./engine.mjs";

const posix = process.platform !== "win32";

/** A build folder whose engine answers `ready`, and on `quit` takes `exitMs` to exit, as a sidecar with a big dataset does. */
function standIn(exitMs) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "obs-engine-test-"));
  const file = path.join(dir, "syngle-observatory-engine");
  fs.writeFileSync(
    file,
    `#!/usr/bin/env node
process.stdout.write(JSON.stringify({ event: "ready", port: 1, pid: process.pid }) + "\\n");
process.stdin.on("data", (d) => { if (String(d).includes("quit")) setTimeout(() => process.exit(0), ${exitMs}); });
`,
    { mode: 0o755 },
  );
  return dir;
}

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

test("stop() answers when the process has exited, not when it was asked to", { skip: !posix }, async () => {
  const dir = standIn(300);
  try {
    const engine = await startEngine({ buildDir: dir });
    assert.ok(alive(engine.pid));
    await engine.stop();
    // The sidecar takes 300 ms to go after `quit`. Before the fix stop() answered at once and a measurement started on
    // top of a process that was still releasing its memory.
    assert.equal(alive(engine.pid), false);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
