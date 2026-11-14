// Evaluator: the canvas's frame times, input-to-frame latency and first draw, on one rendering path. Read-only.
//
//   node evals/map-perf/canvas.mjs --build <dir> --path webgpu-gpu|webgl2-gpu|webgl2-cpu|webgpu-cpu
//        [--canvas canvas|canvas-spike] [--dataset points_30000.fgb] [--scenarios first-draw,pan,zoom,toggle]
//        [--fidelity] [--width 1280 --height 800 --dpr 1] [--out result.json]
//
// `<dir>` is a build from evals/build/boat.sh (or the release's `observatory\` folder): the Windows engine, `canvas/`
// and `data/`. Exit: 0 measured; 1 failed; 2 usage; 3 the path asked for is not the path Chrome gave (refused, not reported).
import fs from "node:fs";
import { parseArgs } from "../lib/args.mjs";
import { SCENARIOS, runCanvas } from "./session.mjs";

const args = parseArgs(process.argv.slice(2), { path: "webgpu-gpu", canvas: "canvas", dataset: "points_30000.fgb" });
if (!args.build) {
  console.error("usage: node evals/map-perf/canvas.mjs --build <dir> --path <path> [--dataset <file in data/>] [--scenarios a,b] [--fidelity] [--out file]");
  process.exit(2);
}
const scenarios = typeof args.scenarios === "string" ? args.scenarios.split(",") : SCENARIOS;
try {
  const result = await runCanvas({
    buildDir: args.build,
    pathName: args.path,
    canvas: args.canvas,
    dataset: args.dataset === "none" ? null : args.dataset,
    scenarios,
    width: args.width,
    height: args.height,
    dpr: args.dpr,
    fidelity: Boolean(args.fidelity),
    log: (line) => process.stderr.write(`[canvas] ${line}\n`),
  });
  const text = JSON.stringify(result, null, 2);
  if (args.out) fs.writeFileSync(args.out, text);
  else console.log(text);
} catch (error) {
  console.error(`[canvas] ${error.message}`);
  process.exit(error.code === "PATH_MISMATCH" ? 3 : 1);
}
