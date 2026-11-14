// The catalogue of scores: which evaluator makes each, how to read its answer, which way is better, and the bar.
// A score is a number a script computes (the hill-climb method of Syngle-Ecosystem #581 and #593). Omar named the targets
// (smooth pan, zoom, toggle and first draw; CPU-only and GPU; most points at 60 and 30 fps; a quick, deterministic Australian
// lookup) but gave no numbers: the bars here are map-lead's proposals, stated in agents/docs/specs/map-hill-climb-and-gov-data.md,
// and a bar that proves out of reach is reported with its measured floor, not dropped. `evals/baselines/*.json` holds the start.
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const maps = path.join(here, "..", "map-perf");
const gov = path.join(here, "..", "gov-data");

export const CANVAS_PATHS = ["webgpu-gpu", "webgl2-gpu", "webgl2-cpu", "webgpu-cpu"];
const isGpu = (p) => p.endsWith("-gpu");

/** Bars for the canvas's frame and latency scores, per kind of path: a GPU path holds 60 Hz, a software path holds 30 Hz. */
const bar = (p, gpuBar, cpuBar) => (isGpu(p) ? gpuBar : cpuBar);

function canvasScores(p) {
  const id = (name) => `canvas.${p}.${name}`;
  return {
    [id("pan.frameP95Ms")]: { direction: "lower", unit: "ms", bar: bar(p, 20, 36.7), read: (r) => r.scenarios.pan.frame.p95 },
    // The share of frames that missed the path's own budget: 60 fps (counted above 20 ms) on a GPU path, 30 fps (above 36.7 ms) on a software path.
    ...(isGpu(p)
      ? { [id("pan.over60Pct")]: { direction: "lower", unit: "%", bar: 2, read: (r) => r.scenarios.pan.frame.over60 } }
      : { [id("pan.over30Pct")]: { direction: "lower", unit: "%", bar: 5, read: (r) => r.scenarios.pan.frame.over30 } }),
    [id("pan.latencyP95Ms")]: { direction: "lower", unit: "ms", bar: bar(p, 50, 100), read: (r) => r.scenarios.pan.latency.p95 },
    [id("zoom.frameP95Ms")]: { direction: "lower", unit: "ms", bar: bar(p, 20, 36.7), read: (r) => r.scenarios.zoom.frame.p95 },
    [id("zoom.latencyP95Ms")]: { direction: "lower", unit: "ms", bar: bar(p, 50, 100), read: (r) => r.scenarios.zoom.latency.p95 },
    [id("toggle.latencyP95Ms")]: { direction: "lower", unit: "ms", bar: bar(p, 100, 200), read: (r) => r.scenarios.toggle.latency.p95 },
    [id("toggle.frameMaxMs")]: { direction: "lower", unit: "ms", bar: bar(p, 100, 250), read: (r) => r.scenarios.toggle.frame.max },
    [id("firstDataMs")]: { direction: "lower", unit: "ms", bar: bar(p, 1000, 3000), read: (r) => r.firstDraw.firstDataMs },
  };
}

function maxPointsScores(p) {
  return {
    [`maxpoints60.${p}`]: { direction: "higher", unit: "points", bar: isGpu(p) ? 1000000 : 50000, read: (r) => r.maxPoints60 },
    [`maxpoints30.${p}`]: { direction: "higher", unit: "points", bar: isGpu(p) ? 1000000 : 100000, read: (r) => r.maxPoints30 },
  };
}

function engineScores() {
  const at = (n, pick) => (r) => pick(r.results.find((x) => x.dataset === `points_${n}.fgb`) ?? {});
  const frame = (n, step, bar) => ({ direction: "lower", unit: "ms", bar, read: at(n, (x) => x.render?.[step]?.p95) });
  return {
    "engine.30k.still.p95Ms": frame(30000, "still", 50),
    "engine.30k.pan.p95Ms": frame(30000, "pan", 60),
    "engine.30k.zoom.p95Ms": frame(30000, "zoom", 60),
    "engine.30k.toggle.p95Ms": frame(30000, "toggle", 60),
    "engine.1m.still.p95Ms": frame(1000000, "still", 250),
    "engine.1m.pan.p95Ms": frame(1000000, "pan", 250),
    "engine.1m.zoom.p95Ms": frame(1000000, "zoom", 250),
    "engine.1m.workingSetMb": { direction: "lower", unit: "MB", bar: 1000, read: at(1000000, (x) => x.workingSetMb) },
    "engine.startupMs": { direction: "lower", unit: "ms", bar: 50, read: at(30000, (x) => x.startupMs) },
  };
}

function govScores() {
  const pct = (key) => ({ direction: "higher", unit: "%", bar: 100, read: (r) => r[key].passPct });
  const lat = (key, bar) => ({ direction: "lower", unit: "ms", bar, read: (r) => r.latency?.[key]?.p95 ?? null });
  return {
    "gov.golden.passPct": pct("golden"),
    ...Object.fromEntries(["abn", "name", "company", "address", "stats", "area"].map((k) => [`gov.golden.${k}.passPct`, { direction: "higher", unit: "%", bar: 100, read: (r) => r.golden.byKind?.[k]?.passPct ?? null }])),
    "gov.determinism.passPct": pct("determinism"),
    "gov.versioned.passPct": pct("versioned"),
    "gov.lifecycle.passPct": pct("lifecycle"),
    "gov.hostile.passPct": pct("hostile"),
    "gov.answerBytes.p95": { direction: "lower", unit: "bytes", bar: 1200, read: (r) => r.answerBytes.p95 },
    // One process per call, process start included, over a 500,000 ABN set (evals/gov-data/gen-scale.mjs).
    "gov.abn.p95Ms": lat("abn", 100),
    "gov.name.p95Ms": lat("name", 150),
    "gov.address.p95Ms": lat("address", 150),
    "gov.stats.p95Ms": lat("stats", 100),
    "gov.area.p95Ms": lat("area", 100),
    "gov.coldCallMs": { direction: "lower", unit: "ms", bar: 400, read: (r) => r.latency?.coldCallMs ?? null },
    "gov.build.secPerMillionRecords": { direction: "lower", unit: "s", bar: 120, read: (r) => r.latency?.scale?.buildSecondsPerMillionRecords ?? null },
  };
}

/** Groups: one evaluator command run per measured run, and every score its answer holds. */
export function groups() {
  const out = {};
  for (const p of CANVAS_PATHS) {
    out[`canvas:${p}`] = {
      script: path.join(maps, "canvas.mjs"),
      args: (ctx) => ["--build", ctx.build, "--path", p, "--dataset", "points_30000.fgb", "--scenarios", "first-draw,pan,zoom,toggle"],
      scores: canvasScores(p),
    };
    out[`maxpoints:${p}`] = {
      script: path.join(maps, "max-points.mjs"),
      args: (ctx) => ["--build", ctx.build, "--path", p],
      scores: maxPointsScores(p),
    };
  }
  out.engine = {
    script: path.join(maps, "engine.mjs"),
    args: (ctx) => ["--build", ctx.build, "--datasets", "points_30000.fgb,points_1000000.fgb"],
    scores: engineScores(),
  };
  out.datasources = {
    script: path.join(here, "..", "data-sources", "check.mjs"),
    args: (ctx) => ["--build", ctx.build],
    scores: {
      "datasources.reachablePct": { direction: "higher", unit: "%", bar: 100, read: (r) => r.reachablePct },
      "datasources.openedPct": { direction: "higher", unit: "%", bar: 100, read: (r) => r.openedPct },
      "datasources.wantOpened": { direction: "higher", unit: "sources", bar: 1, read: (r) => r.wantOpened },
    },
  };
  out.gov = {
    script: path.join(gov, "lookup.mjs"),
    args: (ctx) => ["--cli", ctx.cli ?? "", "--fixtures", path.join(gov, "fixtures"), ...(ctx.scale ? ["--scale", ctx.scale] : []), ...(ctx.kinds ? ["--kinds", ctx.kinds] : [])],
    scores: govScores(),
  };
  return out;
}
