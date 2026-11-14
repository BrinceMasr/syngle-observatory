// Measures the baseline (5 runs, the noise of each score) of the groups in turn, one heavy process at a time, and writes
// `evals/baselines/<group>.json` (scores only) plus the raw runs under `--raw <dir>` (outside the repo). A failing group is
// reported and the next one still runs. Read-only for the product. Run it through `heavy` on this PC.
//
//   node evals/climb/baseline.mjs --build <dir> --raw <dir> [--runs 5] [--only canvas:webgpu-gpu,engine] [--skip maxpoints]
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "../lib/args.mjs";
import { groups } from "./scores.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv.slice(2), { runs: 5 });
if (!args.build || !args.raw) {
  console.error("usage: node evals/climb/baseline.mjs --build <dir> --raw <dir> [--runs 5] [--only a,b] [--skip prefix,prefix]");
  process.exit(2);
}
const only = typeof args.only === "string" ? args.only.split(",") : null;
const skip = typeof args.skip === "string" ? args.skip.split(",") : [];
fs.mkdirSync(args.raw, { recursive: true });
fs.mkdirSync(path.join(here, "..", "baselines"), { recursive: true });
const failed = [];
for (const name of Object.keys(groups())) {
  if (only && !only.includes(name)) continue;
  if (skip.some((prefix) => name.startsWith(prefix))) continue;
  if (name === "gov") continue; // measured against the CLI when it exists (evals/gov-data)
  const file = name.replace(/[:]/g, "-");
  const out = path.join(here, "..", "baselines", `${file}.json`);
  console.error(`[baseline] ${name}`);
  const child = spawnSync(
    process.execPath,
    [path.join(here, "run.mjs"), "--group", name, "--build", args.build, "--runs", String(args.runs), "--save", path.join(args.raw, `${file}.json`), "--scores-out", out, "--label", "baseline"],
    { stdio: "inherit" },
  );
  if (child.status !== 0) failed.push(`${name} (exit ${child.status})`);
}
if (failed.length) {
  console.error(`[baseline] failed: ${failed.join(", ")}`);
  process.exit(1);
}
