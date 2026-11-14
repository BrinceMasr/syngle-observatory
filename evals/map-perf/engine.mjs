// Evaluator: the native engine sidecar's speed, through the real process and the transports the app and the agent use
// (control channel, `POST /bridge`, `GET /render`). Read-only.
//
//   node evals/map-perf/engine.mjs --build <dir> [--datasets points_30000.fgb,points_1000000.fgb] [--size 1280x800] [--steps 30] [--out file]
//
// Per dataset, in a fresh engine process: the start, the dataset opened by path (the agent's route: the engine reads the
// file) and by address (the canvas's route: the engine reads what its own gateway serves), then the engine's frame, which is
// a `/render` of the project (the fork's tiny-skia renderer, PNG out): a still view, 30 pan steps, 20 zoom steps and 20 layer
// toggles, each step being the bridge command plus the render, wall clock. Also the engine's working set after the work.
//
// Each dataset is measured in a node process of its own (`runEach`, lib/each-process.mjs), with a fresh sidecar, and the process
// ends only when its sidecars have exited. In one process the second dataset's first seconds were three to four times slower
// than the same dataset alone: the 1M "still" read 260 to 640 ms for two or three frames of an otherwise 60 to 130 ms series
// (a p95 of 300 ms), in four runs of five. A pause before it did not remove it in the engine climber's runs. The result has the
// same shape either way, and a baseline and a candidate are measured the same way.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "../lib/args.mjs";
import { runEach } from "../lib/each-process.mjs";
import { startEngine } from "../lib/engine.mjs";
import { summarize } from "../lib/stats.mjs";

const args = parseArgs(process.argv.slice(2), { datasets: "points_30000.fgb,points_1000000.fgb", size: "1280x800", steps: 30 });
if (!args.build) {
  console.error("usage: node evals/map-perf/engine.mjs --build <dir> [--datasets a.fgb,b.fgb] [--size WxH] [--out file]");
  process.exit(2);
}
const [width, height] = String(args.size).split("x").map(Number);
const now = () => performance.now();

function workingSetMb(pid) {
  if (process.platform === "win32") {
    const out = spawnSync("tasklist", ["/FI", `PID eq ${pid}`, "/FO", "CSV", "/NH"], { encoding: "utf8" }).stdout ?? "";
    const kb = Number((/"([\d,]+) K"\s*$/m.exec(out.trim()) ?? [])[1]?.replace(/,/g, ""));
    return Number.isFinite(kb) ? Math.round(kb / 1024) : null;
  }
  const status = fs.existsSync(`/proc/${pid}/status`) ? fs.readFileSync(`/proc/${pid}/status`, "utf8") : "";
  const kb = Number((/VmRSS:\s+(\d+)/.exec(status) ?? [])[1]);
  return Number.isFinite(kb) ? Math.round(kb / 1024) : null;
}

async function timed(fn, count) {
  const out = [];
  for (let i = 0; i < count; i++) {
    const t = now();
    await fn(i);
    out.push(now() - t);
  }
  return out;
}

async function measure(dataset) {
  const dataDir = path.join(args.build, "data");
  const t0 = now();
  const engine = await startEngine({ buildDir: args.build });
  const startupMs = Math.round(now() - t0);
  try {
    const hello = await engine.hello();
    const file = path.join(dataDir, dataset);
    const t1 = now();
    const byPath = await engine.open({ path: file, name: "Points", zoom: true });
    const openPathMs = Math.round(now() - t1);
    const stats = await engine.stats();
    const command = async (id, params) => {
      const lines = await engine.bridge({ name: "command", id, params });
      const failed = lines.find((l) => l.event === "bridge.error");
      if (failed) throw new Error(`${id}: ${JSON.stringify(failed)}`);
    };
    const render = async () => {
      const r = await engine.render(width, height);
      if (r.status !== 200 || r.bytes.length < 1000) throw new Error(`/render answered ${r.status} with ${r.bytes.length} bytes`);
      return r.bytes.length;
    };
    let pngBytes = 0;
    await render();
    const still = await timed(async () => (pngBytes = await render()), 20);
    const center = { lon: 134, lat: -25 };
    const pan = await timed(async (i) => {
      await command("map.goToXY", { lon: center.lon + ((i % 15) - 7) * 0.8, lat: center.lat + (Math.floor(i / 15) - 0.5) * 3 });
      await render();
    }, args.steps);
    const zoom = await timed(async (i) => {
      await command(i < 10 ? "map.zoomIn" : "map.zoomOut", {});
      await render();
    }, 20);
    const toggle = await timed(async (i) => {
      await command("layer.visibility", { layer: "Points", visible: i % 2 === 1 });
      await render();
    }, 20);
    await command("layer.visibility", { layer: "Points", visible: true });
    const byUrl = await (async () => {
      const second = await startEngine({ buildDir: args.build });
      try {
        const t2 = now();
        await second.open({ url: `${second.base}/data/${dataset}`, name: "Points", zoom: true });
        return Math.round(now() - t2);
      } finally {
        await second.stop();
      }
    })();
    return {
      dataset,
      startupMs,
      bridgeVersion: hello?.[0]?.state?.protocolVersion ?? hello?.[0]?.protocolVersion ?? null,
      openByPathMs: openPathMs,
      openByUrlMs: byUrl,
      featuresHeld: stats.features,
      layers: stats.layers,
      render: { still: summarize(still), pan: summarize(pan), zoom: summarize(zoom), toggle: summarize(toggle), pngBytes },
      workingSetMb: workingSetMb(engine.pid),
      size: { width, height },
      openedResult: { ok: byPath.ok },
    };
  } finally {
    await engine.stop();
  }
}

const datasets = String(args.datasets).split(",");
let record;
try {
  if (datasets.length > 1) {
    record = runEach({ script: fileURLToPath(import.meta.url), datasets, args: ["--build", args.build, "--size", args.size, "--steps", String(args.steps)] });
  } else {
    process.stderr.write(`[engine] ${datasets[0]}\n`);
    record = { build: args.build, results: [await measure(datasets[0])] };
  }
} catch (error) {
  console.error(`[engine] ${error.message}`);
  process.exit(1);
}
const text = JSON.stringify(record, null, 2);
if (args.out) fs.writeFileSync(args.out, text);
else console.log(text);
