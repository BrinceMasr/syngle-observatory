// Spike A (#43): the Electron host. One process = one measured run.
//
// What it is: the shell a real Observatory window would be. It starts the native engine as a
// sidecar, waits for its localhost gateway, loads the embedding page (which frames the fork's WASM
// view in `?embed` mode), hands the view the project the engine built, and measures:
//
//   cold open   Electron main start → the first composited frame that shows the dataset
//   pan         frame pacing while a real pointer drags the map for 10 s
//   memory      renderer + GPU + engine working sets with 1M points loaded
//
// Usage (from syngle-spike/host):
//   npx electron . --engine ..\target\release\syngle-spike-engine.exe --root ..\.. \
//     --mode both --engine-open http://127.0.0.1:0/data/points_1m.fgb --out run.json
"use strict";

const { app, BrowserWindow } = require("electron");
const { spawn } = require("child_process");
const fs = require("fs");
const path = require("path");

function parseArgs(argv) {
  const out = { mode: "both", poll: 20, panMs: 10000, width: 1600, height: 1000, backend: "webgpu", gpu: "default", step: 16 };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) continue;
    const key = a.slice(2);
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("--")) {
      out[key] = true;
    } else {
      out[key] = /^-?\d+(\.\d+)?$/.test(value) ? Number(value) : value;
      i++;
    }
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
// From the process's own start, so a cold-open number can be compared with the runner's spawn time.
const processStart = Date.now() - Math.round(process.uptime() * 1000);
const t0 = processStart;
const started = { processStart, engineReady: 0, windowCreated: 0, pageLoaded: 0, viewReady: 0, projectSent: 0, firstData: 0, panStart: 0, panEnd: 0 };
const notes = [];
const result = { tag: args.tag || "run", mode: args.mode, poll: args.poll, backend: args.backend, gpu: args.gpu, started, notes, argv: process.argv.slice(2), electron: process.versions.electron };

function note(line) {
  const stamp = new Date().toISOString().slice(11, 23);
  console.log(`[${stamp}] ${line}`);
  notes.push(`${stamp} ${line}`);
}

const rel = (at) => (at ? at - t0 : null);

if (args["low-power"]) app.commandLine.appendSwitch("force_low_power_gpu");
if (args["high-perf"]) app.commandLine.appendSwitch("force_high_performance_gpu");
// Vsync off, so the view's frame cadence is its own cost rather than the display's 60 Hz.
if (args.uncapped) {
  app.commandLine.appendSwitch("disable-gpu-vsync");
  app.commandLine.appendSwitch("disable-frame-rate-limit");
}
app.commandLine.appendSwitch("disable-background-timer-throttling");
app.commandLine.appendSwitch("disable-renderer-backgrounding");
app.commandLine.appendSwitch("disable-backgrounding-occluded-windows");
app.commandLine.appendSwitch("disable-features", "CalculateNativeWinOcclusion");

function writeResult(extra) {
  const out = path.resolve(args.out || "results/run.json");
  fs.mkdirSync(path.dirname(out), { recursive: true });
  Object.assign(result, extra || {});
  result.finished = Date.now();
  fs.writeFileSync(out, JSON.stringify(result, null, 2));
  console.log("SPIKE_RESULT " + out);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// --- the engine sidecar: JSON lines over stdio, one pending call per id ----------------------

function startEngine() {
  return new Promise((resolve, reject) => {
    const exe = args.engine;
    if (!exe) return reject(new Error("--engine <path to syngle-spike-engine> is required"));
    const engineArgs = ["--root", args.root || path.resolve(__dirname, "..", ".."), "--data", args.data || path.resolve(__dirname, "..", "data")];
    if (args.port) engineArgs.push("--port", String(args.port));
    if (args["engine-web"]) engineArgs.push("--web", String(args["engine-web"]));
    if (args["engine-cache"]) engineArgs.push("--cache");
    if (args.preload) engineArgs.push("--preload", args.preload);
    const child = spawn(exe, engineArgs, { stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
    result.engine = { exe, args: engineArgs, pid: child.pid };
    const events = [];
    const pending = new Map();
    let nextId = 1;
    let buffer = "";
    let ready = false;
    const call = (msg) =>
      new Promise((res, rej) => {
        const id = nextId++;
        pending.set(id, res);
        child.stdin.write(JSON.stringify(Object.assign({ id }, msg)) + "\n");
        setTimeout(() => {
          if (pending.delete(id)) rej(new Error("engine call timed out: " + JSON.stringify(msg)));
        }, 300000);
      });
    const onLine = (line) => {
      const text = line.trim();
      if (!text) return;
      if (!text.startsWith("{")) {
        console.log("engine: " + text);
        return;
      }
      let msg;
      try {
        msg = JSON.parse(text);
      } catch (e) {
        console.log("engine (unparsed): " + text.slice(0, 200));
        return;
      }
      events.push({ at: Date.now(), event: msg.event });
      if (msg.event === "ready") {
        started.engineReady = Date.now();
        result.engine.port = msg.port;
        result.engine.base = msg.base;
        result.engine.bootMs = rel(started.engineReady);
        note(`engine ready on ${msg.base} after ${result.engine.bootMs} ms`);
        ready = true;
        resolve({ child, base: msg.base, events, call });
        return;
      }
      if (msg.id !== undefined && pending.has(msg.id) && msg.event !== "opening") {
        const res = pending.get(msg.id);
        pending.delete(msg.id);
        res(msg);
        return;
      }
      if (msg.event === "error") console.error("engine error: " + JSON.stringify(msg).slice(0, 300));
    };
    child.stdout.on("data", (chunk) => {
      buffer += chunk.toString();
      const lines = buffer.split("\n");
      buffer = lines.pop();
      lines.forEach(onLine);
    });
    child.stderr.on("data", (chunk) => {
      const text = chunk.toString().trimEnd();
      if (text) console.log("engine: " + text);
    });
    child.on("exit", (code) => note(`engine exited with ${code}`));
    setTimeout(() => {
      if (!ready) reject(new Error("the engine never reported ready"));
    }, 60000);
  });
}

// --- measurement ------------------------------------------------------------------------------

/// Frame-to-frame difference, used to prove the camera moved during a pan.
function bitmapDiff(before, after) {
  const a = before.toBitmap();
  const b = after.toBitmap();
  const n = Math.min(a.length, b.length);
  let changed = 0;
  let total = 0;
  for (let i = 0; i + 3 < n; i += 4) {
    const d = Math.abs(a[i] - b[i]) + Math.abs(a[i + 1] - b[i + 1]) + Math.abs(a[i + 2] - b[i + 2]);
    total++;
    if (d > 90) changed++;
  }
  return { changed, total, ratio: total ? Number((changed / total).toFixed(4)) : 0 };
}

/// The layer's own fill colour, read out of the project the engine built (the fork's default point
/// symbology is `[224, 123, 57]`). Counting *that* colour is how "data is on screen" is decided:
/// a difference against a blank page only says the app painted something.
function layerColour(projectText) {
  try {
    const project = JSON.parse(projectText);
    const layer = project.maps && project.maps[0] && project.maps[0].layers && project.maps[0].layers[0];
    const renderer = layer && layer.renderer;
    const single = renderer && (renderer.Single || renderer.single);
    const fill = single && single.fill;
    if (Array.isArray(fill) && fill.length >= 3) return [fill[0], fill[1], fill[2]];
  } catch (e) {
    /* fall through to the fork's default */
  }
  return [224, 123, 57];
}

/// Pixels matching `rgb` (the bitmap is BGRA on Windows), and how many the frame has in total.
function colourPixels(image, rgb, tolerance) {
  const b = image.toBitmap();
  const [r, g, bl] = rgb;
  let hits = 0;
  let total = 0;
  for (let i = 0; i + 3 < b.length; i += 4) {
    total++;
    if (Math.abs(b[i] - bl) <= tolerance && Math.abs(b[i + 1] - g) <= tolerance && Math.abs(b[i + 2] - r) <= tolerance) hits++;
  }
  return { hits, total, ratio: total ? Number((hits / total).toFixed(5)) : 0 };
}

/// Sends the project and waits for the first frame the *view* reports as drawn: a real map
/// render after the project arrived, with features in the layer. The loading splash cannot fire
/// it (it draws no layer), and neither can an empty map. The colour count is still taken, but as
/// evidence of what that frame shows, not as the gate.
async function loadProject(win, projectText) {
  const [w, h] = win.getContentSize();
  const rect = { x: 20, y: 20, width: Math.max(100, w - 40), height: Math.max(100, h - 40) };
  const colour = layerColour(projectText);
  result.captureRect = rect;
  result.layerColour = colour;
  const before = await win.webContents.executeJavaScript("spike.appState()");
  const rendersBefore = before ? before.renders : 0;
  const sent = await win.webContents.executeJavaScript(`spike.sendProject(${JSON.stringify(projectText)})`);
  started.projectSent = sent.date;
  let detection = null;
  if (args.poll > 0) {
    let attempts = 0;
    for (;;) {
      attempts++;
      const st = await win.webContents.executeJavaScript("spike.appState()");
      if (st && st.renders > rendersBefore && st.features > 0) {
        started.firstData = Date.now();
        const shot = await win.webContents.capturePage(rect);
        const count = colourPixels(shot, colour, 30);
        detection = {
          attempts,
          colour,
          count,
          captureMs: started.firstData - sent.date,
          features: st.features,
          windowedLayers: st.windowed,
          gpuPointInstances: st.points,
          renders: st.renders - rendersBefore,
        };
        const base = path.resolve(args.out || "results/run.json").replace(/\.json$/, "");
        fs.writeFileSync(base + "-firstdata.png", shot.toPNG());
        break;
      }
      if (Date.now() - sent.date > 60000) {
        detection = { attempts, colour, error: "the view never reported a drawn frame with features" };
        break;
      }
      await sleep(args.poll);
    }
  }
  const after = await win.webContents.executeJavaScript("spike.snapshot()");
  result.firstData = detection;
  result.coldOpen = {
    engineReadyMs: rel(started.engineReady),
    windowCreatedMs: rel(started.windowCreated),
    pageLoadedMs: rel(started.pageLoaded),
    viewReadyMs: rel(started.viewReady),
    projectSentMs: rel(started.projectSent),
    firstDataMs: rel(started.firstData),
    samplesBeforeProject: before ? before.count : 0,
    frameDrawsAtFirstData: after.draws,
    featuresAtFirstData: detection && detection.features,
    instrumentErrors: after.errors,
  };
  note(`loaded: first data at ${result.coldOpen.firstDataMs} ms after ${detection && detection.attempts} polls (features ${detection && detection.features}, colour hits ${detection && detection.count && detection.count.hits})`);
}

/// Percentiles over the samples that are actual numbers (null when there are none).
function percentiles(xs) {
  const v = xs.filter((x) => typeof x === "number" && Number.isFinite(x)).sort((a, b) => a - b);
  const at = (p) => (v.length ? v[Math.min(v.length - 1, Math.floor(p * v.length))] : null);
  return { n: v.length, p50: at(0.5), p95: at(0.95), p99: at(0.99), max: v.length ? v[v.length - 1] : null };
}

/// What the view's own samples say about a drag: frame cadence (the view's frame loop, so on an
/// uncapped run this is the real cost per frame), and the render work of every frame that rebuilt
/// the map (the app's own fractional-ms measure). Held features and GPU point instances come from
/// the same samples, so "1M drawn" is the view's number, not a guess.
function panMetrics(rows) {
  const out = { samples: rows ? rows.length : 0 };
  if (!rows || rows.length < 3) return out;
  const dt = [];
  const renderMs = [];
  let renders = 0;
  let fast = 0;
  let features = 0;
  let visible = 0;
  let windowed = 0;
  let points = 0;
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    if (i > 0) {
      const d = r[0] - rows[i - 1][0];
      if (d > 0) dt.push(d);
      if (r[1] > rows[i - 1][1]) {
        renders += r[1] - rows[i - 1][1];
        renderMs.push(r[3]);
      }
      if (r[2] > rows[i - 1][2]) fast += r[2] - rows[i - 1][2];
    }
    features = Math.max(features, r[4]);
    visible = Math.max(visible, r[5]);
    windowed = Math.max(windowed, r[6]);
    points = Math.max(points, r[7]);
  }
  const frame = percentiles(dt);
  const render = percentiles(renderMs);
  out.wallMs = rows[rows.length - 1][0] - rows[0][0];
  out.frameMs = frame;
  out.fpsFromP50 = frame.p50 ? Number((1000 / frame.p50).toFixed(2)) : null;
  out.fpsFromP95 = frame.p95 ? Number((1000 / frame.p95).toFixed(2)) : null;
  out.renderMs = render;
  out.renderFpsFromP50 = render.p50 ? Number((1000 / render.p50).toFixed(2)) : null;
  out.renders = renders;
  out.fastRepaints = fast;
  out.featuresHeld = features;
  out.visibleLayers = visible;
  out.windowedLayers = windowed;
  out.gpuPointInstances = points;
  const period = frame.p50 || 16.7;
  out.droppedOver1p5 = dt.filter((d) => d > period * 1.5).length;
  return out;
}

/// One full re-render per wheel step, after the drag: what a *complete* draw of the layer costs
/// with the renderer in use (for the CPU renderer this is the number the cached-texture drag
/// hides; for the GPU renderer it is the scene rebuild, not the per-frame raster).
async function fullDrawCost(win, count) {
  if (!count) return null;
  const [w, h] = win.getContentSize();
  const ms = [];
  let prev = await win.webContents.executeJavaScript("spike.appState()");
  let rendersPrev = prev ? prev.renders : 0;
  for (let i = 0; i < count; i++) {
    win.webContents.sendInputEvent({ type: "mouseWheel", x: Math.round(w / 2), y: Math.round(h / 2), deltaX: 0, deltaY: -120, wheelTicksX: 0, wheelTicksY: 1, canScroll: false });
    const deadline = Date.now() + 20000;
    let got = null;
    while (Date.now() < deadline) {
      const st = await win.webContents.executeJavaScript("spike.appState()");
      if (st && st.renders > rendersPrev) {
        got = st;
        break;
      }
      await sleep(10);
    }
    if (!got) {
      note(`full draw ${i + 1}: no render within 20 s`);
      break;
    }
    rendersPrev = got.renders;
    ms.push(got.ms);
    await sleep(120);
  }
  result.fullDraw = { count: ms.length, ms, ...percentiles(ms), fpsFromP50: null };
  const p = percentiles(ms);
  result.fullDraw.fpsFromP50 = p.p50 ? Number((1000 / p.p50).toFixed(2)) : null;
  note(`full draw: ${ms.length} renders, ${JSON.stringify(ms)} ms`);
  return result.fullDraw;
}

async function pan(win) {
  const [w, h] = win.getContentSize();
  const y = Math.round(h / 2);
  const x0 = Math.round(w * 0.25);
  const x1 = Math.round(w * 0.75);
  if (args["map-gpu"]) {
    // The fork's GPU point renderer (View › Toggle GPU Rendering) is a `ui.*` command, so the
    // embedding page may run it over the documented contract.
    try {
      result.mapGpuToggle = await win.webContents.executeJavaScript("spike.runUiCommand('ui.map.gpu')");
      note("toggled the GPU map renderer: " + JSON.stringify(result.mapGpuToggle));
      // The toggle invalidates the frame, and the first GPU scene for a large layer tessellates
      // and uploads every marker on the UI thread: wait until the view reports the scene built,
      // or the drag would measure the build instead of the pan.
      const gpuDeadline = Date.now() + Number(args["map-gpu-wait"] || 90000);
      let gpuState = null;
      for (;;) {
        gpuState = await win.webContents.executeJavaScript("spike.appState()");
        if (gpuState && gpuState.points > 0) {
          note(`GPU scene ready: ${gpuState.points} point instances, ${gpuState.features} features`);
          break;
        }
        if (Date.now() > gpuDeadline) {
          note("the GPU scene never built: the drag below is not measuring the GPU path");
          break;
        }
        await sleep(250);
      }
      result.gpuScene = gpuState && { points: gpuState.points, features: gpuState.features, ms: gpuState.ms, renders: gpuState.renders };
    } catch (e) {
      note("could not toggle the GPU map renderer: " + e.message);
    }
  }
  // A big "read the window" lands just after the first frame: give it a moment before timing the
  // drag, or the measurement is of the read, not of the pan.
  await sleep(Number(args.settle || 1500));
  await win.webContents.executeJavaScript("spike.reset()");
  await win.webContents.executeJavaScript("spike.resetApp()");
  await sleep(200);
  const shotBefore = await win.webContents.capturePage();
  started.panStart = Date.now();
  win.webContents.sendInputEvent({ type: "mouseDown", x: x0, y, button: "middle", clickCount: 1 });
  const periodMs = 2000;
  let t = 0;
  while (t < args.panMs) {
    const phase = (t % periodMs) / periodMs;
    const x = Math.round(x0 + (x1 - x0) * phase);
    const yy = Math.round(y + Math.sin(phase * Math.PI) * 60);
    win.webContents.sendInputEvent({ type: "mouseMove", x, y: yy, button: "middle", movementX: 4, movementY: 1 });
    t += args.step;
    await sleep(args.step);
  }
  win.webContents.sendInputEvent({ type: "mouseUp", x: x1, y, button: "middle", clickCount: 1 });
  started.panEnd = Date.now();
  // The drag's own samples, taken the moment it stops (before any settle re-render can add to
  // the window). Then a settled frame for the pixel evidence.
  const app = await win.webContents.executeJavaScript("spike.appFrames()");
  const snap = await win.webContents.executeJavaScript("spike.snapshot()");
  const metrics = panMetrics(app);
  await sleep(Number(args["settle-after"] || 1500));
  const shotAfter = await win.webContents.capturePage();
  const painted = colourPixels(shotAfter, result.layerColour || [224, 123, 57], 30);
  started.panEndCaptured = Date.now();
  // The view posts its project 600 ms after a change settles: that post is how the report learns
  // what the layer holds (its features are streamed, so the project carries none of them).
  let settled = null;
  try {
    settled = await win.webContents.executeJavaScript("spike.waitForPost(undefined, 5000)");
  } catch (e) {
    note("no settle post after the pan: " + e.message);
  }
  const posts = await win.webContents.executeJavaScript("spike.state.posts.slice(-3)");
  result.panDriver = { frameBacklog: snap.frames ? snap.frames.length : 0, settled };
  const frames = snap.frames || [];
  const rAf = [];
  for (let i = 1; i < frames.length; i++) {
    const d = frames[i] - frames[i - 1];
    if (d > 0 && d < 2000) rAf.push(d);
  }
  result.pan = {
    panMs: args.panMs,
    stepMs: args.step,
    renderer: args["map-gpu"] ? "gpu-map" : "default-cpu",
    uncapped: Boolean(args.uncapped),
    backend: snap.backend,
    draws: snap.draws,
    rAfIntervals: rAf.length,
    // Frame cadence from the view's own loop, and the render work of every rebuilt frame.
    ...metrics,
    instrumentErrors: snap.errors,
    cameraMoved: bitmapDiff(shotBefore, shotAfter),
    painted,
    projectPosts: posts,
    screenshot: Boolean(args.screenshot),
  };
  note(
    `pan (${result.pan.renderer}${args.uncapped ? ", uncapped" : ""}): ${metrics.samples} view frames, ` +
      `frame p50 ${metrics.frameMs && metrics.frameMs.p50 && metrics.frameMs.p50.toFixed(2)} ms ` +
      `(${metrics.fpsFromP50} fps), p95 ${metrics.frameMs && metrics.frameMs.p95 && metrics.frameMs.p95.toFixed(2)}, ` +
      `${metrics.renders} full renders, render p50 ${metrics.renderMs && metrics.renderMs.p50 && metrics.renderMs.p50.toFixed(1)} ms, ` +
      `features ${metrics.featuresHeld}, gpu points ${metrics.gpuPointInstances}`
  );
  await fullDrawCost(win, Number(args.draws || 0));
}

async function memory(win, engine) {
  const metrics = app.getAppMetrics();
  const stats = await engine.call({ cmd: "stats" });
  const engineStats = stats.result || stats;
  // The engine's own working set: the host reads it, because `K32GetProcessMemoryInfo` needs
  // `unsafe` and the fork forbids it.
  let engineRssKb = null;
  try {
    const out = require("child_process").execFileSync("powershell.exe", ["-NoProfile", "-Command", `(Get-Process -Id ${engine.child.pid}).WorkingSet64`], { encoding: "utf8" });
    engineRssKb = Math.round(Number(String(out).trim()) / 1024);
  } catch (e) {
    note("could not read the engine's working set: " + e.message);
  }
  result.memory = {
    appMetrics: metrics.map((m) => ({ type: m.type, name: m.name, pid: m.pid, workingSetKb: m.memory && m.memory.workingSetSize, peakWorkingSetKb: m.memory && m.memory.peakWorkingSetSize })),
    engineRssKb,
    engineLayers: engineStats && engineStats.layers,
    engineFeatures: engineStats && engineStats.features,
    engineHttp: engineStats && engineStats.requests !== undefined ? { requests: engineStats.requests, bytes: engineStats.bytes, ranges: engineStats.ranges } : null,
    rendererKb: metrics.filter((m) => m.type === "Tab" || m.type === "Renderer").reduce((a, m) => a + ((m.memory && m.memory.workingSetSize) || 0), 0),
    gpuKb: metrics.filter((m) => m.type === "GPU").reduce((a, m) => a + ((m.memory && m.memory.workingSetSize) || 0), 0),
  };
  const rendererMb = (result.memory.rendererKb / 1024).toFixed(0);
  const gpuMb = (result.memory.gpuKb / 1024).toFixed(0);
  note(`memory: renderer ${rendererMb} MB, GPU process ${gpuMb} MB, engine ${engineRssKb && (engineRssKb / 1024).toFixed(0)} MB, engine HTTP ${JSON.stringify(result.memory.engineHttp)}`);
}

// --- run ------------------------------------------------------------------------------------

app.whenReady().then(async () => {
  let engine = null;
  try {
    engine = await startEngine();
    // The engine builds the project (it is the data authority); AUTO means the 1M-point FlatGeobuf
    // over the gateway, which is the default load the report measures.
    const dataFile = args["engine-data"] ? String(args["engine-data"]) : "points_1m.fgb";
    const engineOpen = !args["engine-open"] || args["engine-open"] === "AUTO" ? `${engine.base}/data/${dataFile}` : String(args["engine-open"]);
    {
      const which = engineOpen.startsWith("http") ? "url" : "path";
      const reply = await engine.call({ cmd: "open", [which]: engineOpen, name: args["engine-name"] || "points_1m", zoom: true });
      const opened = reply.result || {};
      result.engineOpen = { ok: opened.ok, target: engineOpen, loadMs: opened.loadMs, serializeMs: opened.serializeMs, projectBytes: opened.projectBytes, result: opened.result, error: opened.error };
      if (!opened.project) throw new Error("the engine did not return a project: " + JSON.stringify(opened).slice(0, 400));
      args.projectText = opened.project;
      note(`engine loaded the dataset in ${opened.loadMs && opened.loadMs.toFixed(0)} ms; project ${opened.projectBytes} bytes`);
    }

    const win = new BrowserWindow({
      width: args.width,
      height: args.height,
      show: true,
      backgroundColor: "#101318",
      autoHideMenuBar: true,
      webPreferences: { contextIsolation: true, nodeIntegration: false, backgroundThrottling: false },
    });
    started.windowCreated = Date.now();
    const queryParts = [];
    if (args.backend && args.backend !== "webgpu") queryParts.push(`backend=${args.backend}`);
    // The view's own per-frame samples (frame work, held features, drawn points). Off unless the
    // runner asks for it, so the stock measurement path is unchanged.
    if (args.spike) queryParts.push("spike");
    if (args["project-mode"]) queryParts.push("project=1");
    const query = queryParts.length ? `?${queryParts.join("&")}` : "";
    await win.loadURL(engine.base + "/host/" + query);
    started.pageLoaded = Date.now();
    const ready = await win.webContents.executeJavaScript("spike.waitReady(60000)");
    started.viewReady = ready.date;
    note(`view ready at ${rel(started.viewReady)} ms`);
    // Every mode loads the project first: a pan over an empty map is not the measurement.
    await loadProject(win, args.projectText);
    if (args.mode === "pan" || args.mode === "both") await pan(win);
    result.gpu = await win.webContents.executeJavaScript("spike.info()");
    await memory(win, engine);
    if (args.screenshot) {
      const shot = await win.webContents.capturePage();
      fs.writeFileSync(path.resolve(args.screenshot), shot.toPNG());
    }
    writeResult({ ok: true });
    win.destroy();
  } catch (e) {
    console.error("spike host failed: " + (e && e.stack ? e.stack : e));
    writeResult({ ok: false, error: String(e && e.message ? e.message : e) });
  } finally {
    if (engine && engine.child) {
      try {
        engine.child.stdin.write(JSON.stringify({ cmd: "quit" }) + "\n");
      } catch (e) {
        /* the engine is gone */
      }
      setTimeout(() => process.exit(result.ok ? 0 : 1), 800);
    } else {
      process.exit(result.ok ? 0 : 1);
    }
  }
});
