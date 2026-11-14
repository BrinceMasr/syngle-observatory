// Tests of running an evaluator once per dataset, each in a node process of its own. Run: `node --test evals/lib/each-process.test.mjs`.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { runEach } from "./each-process.mjs";

/** An evaluator stand-in: writes one result naming its dataset, its process and its arguments to `--out`, or exits with `--fail`'s code. */
function standIn(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "obs-each-test-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const script = path.join(dir, "evaluator.mjs");
  fs.writeFileSync(
    script,
    `import fs from "node:fs";
const argv = process.argv.slice(2);
const at = (name) => argv[argv.indexOf(name) + 1];
if (at("--datasets") === at("--fail")) process.exit(7);
fs.writeFileSync(at("--out"), JSON.stringify({ build: at("--build"), results: [{ dataset: at("--datasets"), pid: process.pid, argv }] }));
`,
  );
  return script;
}

test("each dataset is measured in a process of its own, one after the other, and the results come back in order", (t) => {
  const script = standIn(t);
  const out = runEach({ script, datasets: ["a.fgb", "b.fgb", "c.fgb"], args: ["--build", "B", "--steps", "30"] });
  assert.deepEqual(out.results.map((r) => r.dataset), ["a.fgb", "b.fgb", "c.fgb"]);
  const pids = new Set(out.results.map((r) => r.pid));
  assert.equal(pids.size, 3, "three processes");
  assert.ok(!pids.has(process.pid), "none of them is this one");
  assert.equal(out.build, "B");
});

test("a child is given one dataset and the caller's other arguments", (t) => {
  const script = standIn(t);
  const { results } = runEach({ script, datasets: ["a.fgb", "b.fgb"], args: ["--build", "B", "--steps", "30"] });
  assert.deepEqual(results[1].argv.slice(0, 6), ["--build", "B", "--steps", "30", "--datasets", "b.fgb"]);
});

test("a failing child stops the run with its exit status and names its dataset", (t) => {
  const script = standIn(t);
  assert.throws(() => runEach({ script, datasets: ["a.fgb", "b.fgb", "c.fgb"], args: ["--build", "B", "--fail", "b.fgb"] }), (error) => error.status === 7 && /b\.fgb/.test(error.message));
});
