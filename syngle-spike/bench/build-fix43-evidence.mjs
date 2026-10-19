// Spike A (#43) fix round: rebuild the run evidence from the raw per-run JSON files.
// Run from the bench directory with Windows node (results/ paths are Windows paths).
import fs from "node:fs";
const R = "results";
const round1 = (x) => (typeof x === "number" && Number.isFinite(x) ? +x.toFixed(1) : null);

const coldTags = [
  "fix-rtx-webgpu-parquet",
  "fix-uhd-webgpu-parquet",
  "fix-rtx-webgpu-fgb",
  "fix-uhd-webgpu-fgb",
  "fix-rtx-webgl2-fgb",
  "fix-uhd-webgl2-fgb",
];
const cold = [];
for (const t of coldTags) {
  const s = JSON.parse(fs.readFileSync(`${R}/fix43/${t}-summary.json`, "utf8"));
  const runs = fs
    .readdirSync(`${R}/fix43`)
    .filter((f) => f.startsWith(t + "-run") && f.endsWith(".json"))
    .sort()
    .map((f) => JSON.parse(fs.readFileSync(`${R}/fix43/${f}`, "utf8")));
  cold.push({
    tag: t,
    runs: s.runs,
    fromSpawnMs: s.cold.fromSpawnMs,
    medianMs: s.cold.medianFromSpawnMs,
    firstDataFeatures: runs.map((r) => r.firstData && r.firstData.features),
    firstDataColourHits: runs.map((r) => r.firstData && r.firstData.count && r.firstData.count.hits),
    backend: runs[0].backend,
    appCanvasContext: runs[0].gpu && runs[0].gpu.appCanvasContext,
  });
}

const pan = [];
for (const d of ["fix43", "fix43gpu", "fix43uhd"]) {
  if (!fs.existsSync(`${R}/${d}`)) continue;
  for (const f of fs.readdirSync(`${R}/${d}`).filter((f) => /-run\d\.json$/.test(f)).sort()) {
    const r = JSON.parse(fs.readFileSync(`${R}/${d}/${f}`, "utf8"));
    if (!r.pan) continue;
    const p = r.pan;
    pan.push({
      run: f.replace(/\.json$/, ""),
      dir: d,
      ok: r.ok,
      error: r.error,
      renderer: p.renderer,
      uncapped: Boolean(p.uncapped),
      samples: p.samples,
      wallMs: round1(p.wallMs),
      frameMs: p.frameMs,
      renderMs: p.renderMs,
      fpsFromP50: round1(p.fpsFromP50),
      renders: p.renders,
      fastRepaints: p.fastRepaints,
      droppedOver1p5: p.droppedOver1p5,
      featuresHeld: p.featuresHeld,
      gpuPointInstances: p.gpuPointInstances,
      fullDrawMs: (r.fullDraw && r.fullDraw.ms) || [],
      painted: p.painted && p.painted.hits,
      cameraMoved: p.cameraMoved && p.cameraMoved.ratio,
      memory: r.memory && {
        rendererMb: Math.round(r.memory.rendererKb / 1024),
        gpuProcessMb: Math.round(r.memory.gpuKb / 1024),
        engineMb: Math.round(r.memory.engineRssKb / 1024),
      },
    });
  }
}

const failedBatch = ["stock-uhd-fgb-cold-run1.json", "stock-uhd-fgb-cold-run2.json", "stock-uhd-fgb-cold-run3.json"].map((f) => {
  const r = JSON.parse(fs.readFileSync(`${R}/${f}`, "utf8"));
  return { run: f, ok: r.ok, error: r.error, note: r.notes && r.notes.find((x) => x.includes("web-stock")) };
});

const out = {
  measuredAt: new Date().toISOString(),
  cold,
  pan,
  failedBatch,
  note:
    "Cold open: shipped build (dist/web-stock, the 30k remote view window), gate = the view's first real render after the project arrived, with features in the layer. Pan: the view's own per-frame samples (frameMs = the view's frame cadence; renderMs = the app's own render work on frames that rebuilt the map; featuresHeld and gpuPointInstances from the same samples).",
};
fs.writeFileSync("runs-fix43.json", JSON.stringify(out, null, 1));
console.log("cold", cold.length, "pan", pan.length, "failed", failedBatch.length);
