// Runs an evaluator once per dataset, each in a node process of its own, one after the other. A dataset measured in the
// process that measured another read three or four times slower in its first seconds (the engine evaluator's 1M "still":
// frames of 60 to 130 ms alone, two or three of 430 to 580 ms after the 30k dataset), so each dataset gets a fresh scorer
// and a fresh sidecar, the same for a baseline and for a candidate.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

/**
 * `script` is run as `node script <args> --datasets <one> --out <file>` for each dataset in turn; every run must write
 * `{ build, results: [...] }` to its `--out`. The answer is the first run's record with every run's `results` joined in
 * dataset order. A run that fails stops the rest: the error carries its exit status and names the dataset.
 */
export function runEach({ script, datasets, args = [] }) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "obs-each-"));
  try {
    let merged = null;
    for (const dataset of datasets) {
      const out = path.join(dir, "result.json");
      const child = spawnSync(process.execPath, [script, ...args, "--datasets", dataset, "--out", out], { stdio: ["ignore", "inherit", "inherit"] });
      if (child.status !== 0 || !fs.existsSync(out)) {
        throw Object.assign(new Error(`${dataset}: the evaluator exited with ${child.status ?? child.signal}`), { status: child.status ?? 1 });
      }
      const record = JSON.parse(fs.readFileSync(out, "utf8"));
      fs.rmSync(out, { force: true });
      merged = merged ? { ...merged, results: [...merged.results, ...record.results] } : record;
    }
    return merged ?? { results: [] };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}
