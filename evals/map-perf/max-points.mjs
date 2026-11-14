// Evaluator: how many points can be on screen at once at 60 fps and at 30 fps, on one rendering path. Read-only.
//
//   node evals/map-perf/max-points.mjs --build <dir> --path webgpu-gpu [--ladder 10000,20000,50000,...] [--canvas canvas] [--out file]
//
// The ladder is the files `data/points_<n>.fgb` the build holds (deterministic, 80 % clustered over 40 Australian cities,
// 20 % uniform: syngle-spike/gen). A rung "holds 60 fps" when the p95 animation-frame interval of the pan and of the zoom are
// both at most MISSED_FRAME_MS (20 ms: no missed 60 Hz frame at p95), and "holds 30 fps" at MISSED_30_MS (36.7 ms); and
// only if the fidelity gate passes (fidelity.mjs: at least 98 % of the file's points in the view have ink near them), so a
// build cannot score by drawing fewer points. Scores: the largest rung that holds, stopping at the first rung that fails
// both (no gaps counted). A path that holds the top rung scores the top rung: extend the ladder then.
import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "../lib/args.mjs";
import { MISSED_30_MS, MISSED_FRAME_MS } from "./frames.mjs";
import { runCanvas } from "./session.mjs";

const args = parseArgs(process.argv.slice(2), { path: "webgpu-gpu", canvas: "canvas" });
if (!args.build) {
  console.error("usage: node evals/map-perf/max-points.mjs --build <dir> --path <path> [--ladder 10000,...] [--out file]");
  process.exit(2);
}
const available = fs
  .readdirSync(path.join(args.build, "data"))
  .map((f) => /^points_(\d+)\.fgb$/.exec(f))
  .filter(Boolean)
  .map((m) => Number(m[1]))
  .sort((a, b) => a - b);
const ladder = typeof args.ladder === "string" ? args.ladder.split(",").map(Number) : available;
const missing = ladder.filter((n) => !available.includes(n));
if (!ladder.length || missing.length) {
  console.error(`[max-points] no data for rungs ${missing.join(", ") || "(none)"}; the build has ${available.join(", ")}`);
  process.exit(2);
}

const rungs = [];
let max60 = 0;
let max30 = 0;
let alive60 = true;
let alive30 = true;
try {
  for (const n of ladder) {
    process.stderr.write(`[max-points] ${args.path} rung ${n}\n`);
    const r = await runCanvas({ buildDir: args.build, pathName: args.path, canvas: args.canvas, dataset: `points_${n}.fgb`, scenarios: ["pan", "zoom"], fidelity: true });
    const p95 = Math.max(r.scenarios.pan.frame.p95 ?? Infinity, r.scenarios.zoom.frame.p95 ?? Infinity);
    const faithful = Boolean(r.fidelity?.pass);
    const holds60 = faithful && p95 <= MISSED_FRAME_MS;
    const holds30 = faithful && p95 <= MISSED_30_MS;
    rungs.push({
      points: n,
      p95FrameMs: p95,
      panP50: r.scenarios.pan.frame.p50,
      zoomP50: r.scenarios.zoom.frame.p50,
      recall: r.fidelity?.recall ?? null,
      featuresHeld: r.fidelity?.featuresHeld ?? null,
      pointsSubmitted: r.fidelity?.pointsSubmitted ?? null,
      holds60,
      holds30,
      gpu: r.gpu,
    });
    if (alive60 && holds60) max60 = n;
    else alive60 = false;
    if (alive30 && holds30) max30 = n;
    else alive30 = false;
    if (!alive60 && !alive30) break;
  }
} catch (error) {
  console.error(`[max-points] ${error.message}`);
  process.exit(error.code === "PATH_MISMATCH" ? 3 : 1);
}
const out = { path: args.path, canvas: args.canvas, ladder, maxPoints60: max60, maxPoints30: max30, topRung: ladder[ladder.length - 1], rungs };
const text = JSON.stringify(out, null, 2);
if (args.out) fs.writeFileSync(args.out, text);
else console.log(text);
