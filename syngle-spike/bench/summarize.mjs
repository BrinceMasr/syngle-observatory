// Spike A (#43): recompute the per-tag summaries from the raw run files.
//
// `run.mjs` writes one JSON per run; this reads them all, leaves the warm-up runs out of the
// medians (they are the OS-file-cache ones), and prints the table the report quotes. Keeping it
// separate means a summary can always be rederived, never trusted.
//
//   node summarize.mjs [--dir results] [--json summaries.json]
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
function parse(argv) {
  const out = { dir: path.join(here, "results"), json: null };
  for (let i = 0; i < argv.length; i++) {
    if (!argv[i].startsWith("--")) continue;
    out[argv[i].slice(2)] = argv[i + 1];
    i++;
  }
  return out;
}
const args = parse(process.argv.slice(2));
const dir = path.resolve(args.dir);

const median = (xs) => {
  const v = xs.filter((x) => typeof x === "number" && Number.isFinite(x)).sort((a, b) => a - b);
  return v.length ? (v.length % 2 ? v[(v.length - 1) / 2] : (v[v.length / 2 - 1] + v[v.length / 2]) / 2) : null;
};
const round = (x, n = 1) => (typeof x === "number" && Number.isFinite(x) ? Number(x.toFixed(n)) : null);

const tags = new Map();
for (const file of fs.readdirSync(dir)) {
  if (!file.endsWith(".json") || file.endsWith("-summary.json") || file === "summaries.json") continue;
  let run;
  try {
    run = JSON.parse(fs.readFileSync(path.join(dir, file), "utf8"));
  } catch (e) {
    continue;
  }
  const tag = String(run.tag || "").replace(/-(warmup|run\d+)$/, "");
  if (!tag) continue;
  if (!tags.has(tag)) tags.set(tag, []);
  tags.get(tag).push({ file, run });
}

const out = [];
for (const [tag, entries] of [...tags.entries()].sort()) {
  const warm = entries.filter((e) => e.run.warmup);
  const measured = entries.filter((e) => !e.run.warmup && e.run.ok);
  const firstData = measured.map((e) => (e.run.started && e.run.started.firstData ? e.run.started.firstData - e.run.runnerSpawnWall : null));
  const pans = measured.filter((e) => e.run.pan);
  out.push({
    tag,
    mode: measured[0] && measured[0].run.mode,
    backend: measured[0] && measured[0].run.backend,
    gpuRequest: measured[0] && measured[0].run.gpu,
    runs: measured.length,
    warmups: warm.length,
    failed: entries.filter((e) => !e.run.ok).length,
    gpu: measured[0] && measured[0].run.gpu ? describe(measured[0].run.gpu) : null,
    coldOpenFromSpawnMs: { values: firstData, median: round(median(firstData)), min: round(Math.min(...firstData)), max: round(Math.max(...firstData)) },
    engineReadyMs: round(median(measured.map((e) => e.run.coldOpen && e.run.coldOpen.engineReadyMs))),
    engineLoadMs: round(median(measured.map((e) => e.run.engineOpen && e.run.engineOpen.loadMs)), 2),
    viewReadyMs: round(median(measured.map((e) => e.run.coldOpen && e.run.coldOpen.viewReadyMs))),
    projectSentMs: round(median(measured.map((e) => e.run.coldOpen && e.run.coldOpen.projectSentMs))),
    firstDataInternalMs: round(median(measured.map((e) => e.run.coldOpen && e.run.coldOpen.firstDataMs))),
    pan: pans.length
      ? {
          fpsMedian: round(median(pans.map((e) => e.run.pan.fpsFromMedianMs)), 2),
          fpsPerRun: pans.map((e) => round(e.run.pan.fpsFromMedianMs, 2)),
          medianFrameMs: round(median(pans.map((e) => e.run.pan.medianFrameMs)), 2),
          p95FrameMs: round(median(pans.map((e) => e.run.pan.p95FrameMs)), 2),
          p99FrameMs: round(median(pans.map((e) => e.run.pan.p99FrameMs)), 2),
          maxFrameMs: round(median(pans.map((e) => e.run.pan.maxFrameMs)), 1),
          dropped: pans.map((e) => e.run.pan.droppedOver1p5Period),
          intervals: round(median(pans.map((e) => e.run.pan.intervals))),
        }
      : null,
    memory: measured.length
      ? {
          rendererMb: round(median(measured.map((e) => e.run.memory && e.run.memory.rendererKb / 1024)), 0),
          gpuProcessMb: round(median(measured.map((e) => e.run.memory && e.run.memory.gpuKb / 1024)), 0),
          engineMb: round(median(measured.map((e) => e.run.memory && e.run.memory.engineRssKb / 1024)), 0),
          engineHttp: (measured[measured.length - 1].run.memory || {}).engineHttp || null,
        }
      : null,
  });
}

function describe(g) {
  if (!g) return null;
  const webgpu = g.webgpu && typeof g.webgpu === "object" ? `${g.webgpu.vendor}/${g.webgpu.architecture}` : String(g.webgpu);
  const backend = g.instrument ? g.instrument.backend : null;
  return `webgpu=${webgpu}; instrument=${backend}; webgl2=${g.webgl2 || "?"}`;
}

const jsonPath = args.json || path.join(dir, "summaries.json");
fs.writeFileSync(jsonPath, JSON.stringify(out, null, 2));
for (const s of out) {
  console.log(
    `${s.tag.padEnd(22)} runs=${s.runs}(+${s.warmups}w,${s.failed}f) cold=${s.coldOpenFromSpawnMs.median} ms [${s.coldOpenFromSpawnMs.values.map((v) => round(v, 0)).join(",")}] pan=${s.pan ? s.pan.fpsMedian + " fps" : "-"} p95=${s.pan ? s.pan.p95FrameMs : "-"} mem: r${s.memory ? s.memory.rendererMb : "-"}/g${s.memory ? s.memory.gpuProcessMb : "-"}/e${s.memory ? s.memory.engineMb : "-"} MB`
  );
}
console.log("\nwritten " + jsonPath);
