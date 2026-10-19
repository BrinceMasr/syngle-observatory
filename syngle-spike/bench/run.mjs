// Spike A (#43): the repeatable runner. One Electron process per measured run, then the medians.
//
//   node run.mjs --runs 5 --mode cold --tag rtx-webgpu-cold --backend webgpu
//   node run.mjs --runs 3 --mode pan  --tag uhd-webgl2-pan  --backend webgl2 --gpu low
//
// It knows nothing the host does not report: it spawns `electron .` with the engine path, reads the
// run's JSON, and aggregates. A `--gpu low|high` run also needs `node gpu-pref.mjs low|high` around
// it (the runner does it itself and always restores the previous value).
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const hostDir = path.resolve(here, "..", "host");
const root = path.resolve(here, "..", "..");

function parse(argv) {
  const out = { runs: 1, mode: "both", backend: "webgpu", gpu: "default", poll: 20, panMs: 10000, outdir: path.resolve(here, "results") };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) continue;
    const key = a.slice(2);
    const v = argv[i + 1];
    if (v === undefined || v.startsWith("--")) out[key] = true;
    else {
      out[key] = /^-?\d+(\.\d+)?$/.test(v) ? Number(v) : v === "false" || v === "no" ? false : v;
      i++;
    }
  }
  return out;
}

const args = parse(process.argv.slice(2));
const engineExe = args.engine || path.join(root, "target", "release", "syngle-spike-engine.exe");
const tag = args.tag || `${args.mode}-${args.backend}-${args.gpu}`;
const electron = args.electron || path.join(hostDir, "node_modules", "electron", "dist", "electron.exe");
const outdir = path.resolve(args.outdir);

function log(line) {
  process.stdout.write(line + "\n");
}

const median = (xs) => {
  const v = xs.filter((x) => typeof x === "number" && Number.isFinite(x)).sort((a, b) => a - b);
  return v.length ? v[Math.floor(v.length / 2)] : null;
};
const mean = (xs) => {
  const v = xs.filter((x) => typeof x === "number" && Number.isFinite(x));
  return v.length ? v.reduce((a, b) => a + b, 0) / v.length : null;
};

function setGpuPreference(kind) {
  // HKCU only, and only for this executable: the documented per-app GPU preference.
  const value = kind === "low" ? "GpuPreference=1;" : kind === "high" ? "GpuPreference=2;" : null;
  if (!value) return () => {};
  const key = "HKCU\\Software\\Microsoft\\DirectX\\UserGpuPreferences";
  const query = spawnSync("reg.exe", ["query", key, "/v", electron], { encoding: "utf8" });
  const previous = query.status === 0 && /REG_SZ\s+(.*)/.exec(query.stdout) ? /REG_SZ\s+(.*)/.exec(query.stdout)[1].trim() : null;
  spawnSync("reg.exe", ["add", key, "/v", electron, "/t", "REG_SZ", "/d", value, "/f"], { encoding: "utf8" });
  return () => {
    if (previous) spawnSync("reg.exe", ["add", key, "/v", electron, "/t", "REG_SZ", "/d", previous, "/f"], { encoding: "utf8" });
    else spawnSync("reg.exe", ["delete", key, "/v", electron, "/f"], { encoding: "utf8" });
  };
}

async function runOne(index, warmup) {
  const name = warmup ? `${tag}-warmup` : `${tag}-run${index}`;
  const out = path.join(outdir, `${name}.json`);
  const logFile = path.join(outdir, `${name}.log`);
  fs.mkdirSync(outdir, { recursive: true });
  const argv = [
    ".",
    "--engine",
    engineExe,
    "--root",
    root,
    "--mode",
    args.mode === true ? "both" : String(args.mode),
    "--backend",
    String(args.backend),
    "--poll",
    String(args.poll),
    "--pan-ms",
    String(args.panMs),
    "--out",
    out,
    "--tag",
    name,
  ];
  if (args["engine-open"]) argv.push("--engine-open", args["engine-open"]);
  else argv.push("--engine-open", "AUTO");
  if (args["engine-data"]) argv.push("--engine-data", String(args["engine-data"]));
  // The engine spawns with the host's cwd, so a relative --engine-web would resolve there:
  // give it the absolute path (this is what broke the first UHD FlatGeobuf stock batch).
  if (args["engine-web"]) argv.push("--engine-web", path.resolve(root, String(args["engine-web"])));
  if (args["engine-cache"]) argv.push("--engine-cache");
  if (args["project-mode"]) argv.push("--project-mode");
  if (args.uncapped) argv.push("--uncapped");
  if (args["map-gpu"]) argv.push("--map-gpu");
  // Always: every measurement needs the view's own frame samples and the feature-count gate.
  argv.push("--spike");
  if (args.draws) argv.push("--draws", String(args.draws));
  if (args["settle-after"]) argv.push("--settle-after", String(args["settle-after"]));
  if (args["low-power"]) argv.push("--low-power");
  if (args["high-perf"]) argv.push("--high-perf");
  if (args.screenshot) argv.push("--screenshot", path.join(outdir, `${name}.png`));
  const spawnWall = Date.now();
  log(`\n=== ${name}: electron ${argv.join(" ")}`);
  const child = spawn(electron, argv, { cwd: hostDir, env: { ...process.env } });
  // One stuck UI thread must not wedge the whole batch: kill the run after --run-timeout s.
  const runTimeoutMs = Number(args["run-timeout"] || 240) * 1000;
  const watchdog = setTimeout(() => {
    log(`!! ${name}: timed out after ${runTimeoutMs / 1000} s; killing pid ${child.pid}`);
    try {
      spawnSync("taskkill", ["/F", "/PID", String(child.pid), "/T"], { encoding: "utf8" });
    } catch (e) {
      log(`!! ${name}: taskkill failed: ${e.message}`);
    }
  }, runTimeoutMs);
  const logStream = fs.createWriteStream(logFile);
  let bytes = "";
  child.stdout.on("data", (c) => {
    const text = c.toString();
    logStream.write(text);
    bytes += text;
    if (process.env.SPIKE_VERBOSE) process.stdout.write(text);
  });
  child.stderr.on("data", (c) => logStream.write(c));
  const code = await new Promise((resolve) => child.on("exit", resolve));
  clearTimeout(watchdog);
  logStream.end();
  let result = null;
  try {
    result = JSON.parse(fs.readFileSync(out, "utf8"));
  } catch (e) {
    log(`!! ${name}: no result file (${e.message}); log ${logFile}`);
    return { name, ok: false, spawnWall, code };
  }
  result.runnerSpawnWall = spawnWall;
  result.warmup = Boolean(warmup);
  fs.writeFileSync(out, JSON.stringify(result, null, 2));
  const fromSpawn = result.started && result.started.firstData ? result.started.firstData - spawnWall : null;
  log(
    `${name}: ok=${result.ok} firstData=${fromSpawn} ms (from spawn) engineLoad=${result.engineOpen && result.engineOpen.loadMs && result.engineOpen.loadMs.toFixed(0)} ms pan=${result.pan ? result.pan.fpsFromMedianMs + " fps median " + result.pan.medianFrameMs + " ms" : "-"} gpu=${describeGpu(result)}`
  );
  return { name, ok: Boolean(result.ok), warmup: Boolean(warmup), spawnWall, code, result, firstDataFromSpawn: fromSpawn };
}

function describeGpu(result) {
  const g = result.gpu || {};
  const webgpu = g.webgpu && typeof g.webgpu === "object" ? `${g.webgpu.vendor || ""} ${g.webgpu.architecture || ""} ${g.webgpu.device || ""}`.trim() : String(g.webgpu);
  const instrument = g.instrument ? `backend=${g.instrument.backend || "?"}` : "";
  return `webgpu[${webgpu}] webgl2[${g.webgl2 || "?"}] ${instrument}`;
}

const restore = args.gpu && args.gpu !== "default" ? setGpuPreference(args.gpu) : () => {};
const runs = [];
try {
  if (args.warmup !== false) {
    log("warm-up run (the OS file cache is cold, this one is not counted)");
    runs.push(await runOne(0, true));
  }
  for (let i = 1; i <= args.runs; i++) runs.push(await runOne(i, false));
} finally {
  restore();
}

const measured = runs.filter((r) => r.ok && !r.warmup);
const summary = {
  tag,
  mode: args.mode,
  backend: args.backend,
  gpu: args.gpu,
  runs: measured.length,
  warmup: runs.filter((r) => r.warmup).map((r) => ({ firstDataFromSpawn: r.firstDataFromSpawn, ok: r.ok })),
  cold: {
    fromSpawnMs: measured.map((r) => r.firstDataFromSpawn),
    medianFromSpawnMs: median(measured.map((r) => r.firstDataFromSpawn)),
    engineReadyMs: median(measured.map((r) => r.result.coldOpen && r.result.coldOpen.engineReadyMs)),
    engineLoadMs: median(measured.map((r) => r.result.engineOpen && r.result.engineOpen.loadMs)),
    viewReadyMs: median(measured.map((r) => r.result.coldOpen && r.result.coldOpen.viewReadyMs)),
    projectSentMs: median(measured.map((r) => r.result.coldOpen && r.result.coldOpen.projectSentMs)),
    firstDataMs: median(measured.map((r) => r.result.coldOpen && r.result.coldOpen.firstDataMs)),
  },
  pan: {
    renderer: measured.length ? measured[0].result.pan && measured[0].result.pan.renderer : null,
    uncapped: measured.length ? Boolean(measured[0].result.pan && measured[0].result.pan.uncapped) : null,
    fpsFromP50: median(measured.map((r) => r.result.pan && r.result.pan.fpsFromP50)),
    frameP50Ms: median(measured.map((r) => r.result.pan && r.result.pan.frameMs && r.result.pan.frameMs.p50)),
    frameP95Ms: median(measured.map((r) => r.result.pan && r.result.pan.frameMs && r.result.pan.frameMs.p95)),
    frameP99Ms: median(measured.map((r) => r.result.pan && r.result.pan.frameMs && r.result.pan.frameMs.p99)),
    renderP50Ms: median(measured.map((r) => r.result.pan && r.result.pan.renderMs && r.result.pan.renderMs.p50)),
    renderP95Ms: median(measured.map((r) => r.result.pan && r.result.pan.renderMs && r.result.pan.renderMs.p95)),
    renderP99Ms: median(measured.map((r) => r.result.pan && r.result.pan.renderMs && r.result.pan.renderMs.p99)),
    renderFpsFromP50: median(measured.map((r) => r.result.pan && r.result.pan.renderFpsFromP50)),
    renders: measured.map((r) => r.result.pan && r.result.pan.renders),
    fastRepaints: measured.map((r) => r.result.pan && r.result.pan.fastRepaints),
    featuresHeld: measured.map((r) => r.result.pan && r.result.pan.featuresHeld),
    gpuPointInstances: measured.map((r) => r.result.pan && r.result.pan.gpuPointInstances),
    droppedOver1p5: measured.map((r) => r.result.pan && r.result.pan.droppedOver1p5),
    samples: measured.map((r) => r.result.pan && r.result.pan.samples),
    rAfIntervals: measured.map((r) => r.result.pan && r.result.pan.rAfIntervals),
    perRunFps: measured.map((r) => r.result.pan && r.result.pan.fpsFromP50),
    perRunFrameP50: measured.map((r) => r.result.pan && r.result.pan.frameMs && r.result.pan.frameMs.p50),
    perRunRenderP50: measured.map((r) => r.result.pan && r.result.pan.renderMs && r.result.pan.renderMs.p50),
    fullDrawMs: measured.map((r) => r.result.fullDraw && r.result.fullDraw.ms),
  },
  memoryKb: {
    perRun: measured.map((r) => r.result.memory),
    rendererMedianKb: median(measured.flatMap((r) => (r.result.memory ? r.result.memory.appMetrics.filter((m) => m.type === "Tab" || m.type === "Renderer").map((m) => m.workingSetKb) : []))),
    gpuMedianKb: median(measured.flatMap((r) => (r.result.memory ? r.result.memory.appMetrics.filter((m) => m.type === "GPU").map((m) => m.workingSetKb) : []))),
    engineMedianKb: median(measured.map((r) => r.result.memory && r.result.memory.engineRssKb)),
    totalMeanKb: mean(measured.map((r) => (r.result.memory ? r.result.memory.appMetrics.reduce((a, m) => a + (m.workingSetKb || 0), 0) + (r.result.memory.engineRssKb || 0) : null))),
  },
  gpu: measured.length ? describeGpu(measured[0].result) : null,
  gpuPerRun: measured.map((r) => describeGpu(r.result)),
  files: runs.map((r) => r.name + ".json"),
  ok: measured.length > 0 && measured.every((r) => r.ok),
};
const summaryPath = path.join(outdir, `${tag}-summary.json`);
fs.writeFileSync(summaryPath, JSON.stringify(summary, null, 2));
log("\n=== summary " + summaryPath);
log(JSON.stringify(summary, null, 2));
