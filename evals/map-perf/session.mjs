// One measured session of the canvas: the real sidecar serves the real canvas build, one headless Chrome (a chosen
// rendering path) loads `/canvas/` and is driven only by real input through CDP (`Input.dispatchMouseEvent`) and the
// bridge's own `layers` command. What is read back is the page's own record: animation-frame callbacks and the view's per-frame
// samples (`?spike=1`, `window.__spikeApp`). Nothing in the build is patched. Read-only: it writes only its result.
import fs from "node:fs";
import path from "node:path";
import { startEngine } from "../lib/engine.mjs";
import { assertPath, launchChrome, PROBE_EXPRESSION } from "../lib/chrome.mjs";
import { sleep } from "../lib/args.mjs";
import { appStats, frameIntervals, frameStats, latencies, ROW, windowRows } from "./frames.mjs";
import { decodePng, inkMask, pointRecall, readXy, RECALL_BAR } from "./fidelity.mjs";

// Installed in every frame before its scripts run. The view's iframe starts as an empty document and is then navigated, and
// Chrome keeps that window across the navigation but drops the old document's animation callbacks, so the frame ticker is
// started on demand (`start`) in the document that is there, and every time is `performance.timeOrigin` of that document plus
// its own timestamp (epoch milliseconds). An input is a wheel step or a pointer move that moves, with a button down (a drag
// move): a hover before a press, the pointer's return after a release and a move of no distance change no camera, and scored
// they read the press delay or the gap to the next drag.
export const INIT_SCRIPT = `(() => {
  if (window.__eval) return;
  const E = (window.__eval = { recording: false, loop: 0, raf: [], inputs: [], long: [], gpuSubmits: 0, glDraws: 0 });
  E.start = () => {
    E.recording = true;
    const id = ++E.loop;
    const tick = (t) => { if (!E.recording || id !== E.loop) return; E.raf.push(performance.timeOrigin + t); requestAnimationFrame(tick); };
    requestAnimationFrame(tick);
  };
  E.stop = () => { E.recording = false; E.loop++; };
  for (const type of ["pointermove", "wheel"]) addEventListener(type, (e) => { if (E.recording && (type === "wheel" || (e.buttons && (e.movementX || e.movementY)))) E.inputs.push(performance.timeOrigin + e.timeStamp); }, { capture: true, passive: true });
  try { new PerformanceObserver((l) => { for (const x of l.getEntries()) if (E.recording) E.long.push(x.duration); }).observe({ type: "longtask" }); } catch {}
  try { if (window.GPUQueue) { const s = GPUQueue.prototype.submit; GPUQueue.prototype.submit = function () { E.gpuSubmits++; return s.apply(this, arguments); }; } } catch {}
  try {
    const proto = window.WebGL2RenderingContext && WebGL2RenderingContext.prototype;
    if (proto) for (const n of ["drawArrays", "drawElements", "drawArraysInstanced", "drawElementsInstanced", "drawRangeElements"]) {
      const f = proto[n]; if (f) proto[n] = function () { E.glDraws++; return f.apply(this, arguments); };
    }
  } catch {}
})();`;

const VIEW = `document.getElementById("view").contentWindow`;

/**
 * The `Input.dispatchMouseEvent` parameters of one mouse event. A move with no button down says `button: "none"`, as a mouse
 * sends it: sent with `button: "left"` it reached the page as a pointermove with `buttons` 1, so the hover before a press and
 * the pointer's return after a release looked like drag moves.
 */
export const mouseParams = (type, x, y, extra = {}) => ({ type, x, y, button: type === "mouseMoved" && !extra.buttons ? "none" : "left", clickCount: 1, ...extra });

/** Everything a scenario can ask for, in the order a run takes them. */
export const SCENARIOS = ["first-draw", "pan", "zoom", "toggle"];

export async function runCanvas(options) {
  const {
    buildDir,
    pathName,
    canvas = "canvas",
    dataset = "points_30000.fgb",
    scenarios = SCENARIOS,
    width = 1280,
    height = 800,
    dpr = 1,
    fidelity = false,
    log = () => {},
  } = options;
  const dataDir = path.join(buildDir, "data");
  const engine = await startEngine({ buildDir, canvasDir: path.join(buildDir, canvas), dataDir });
  let chrome = null;
  try {
    const sample = path.join(dataDir, "countries.geojson");
    if (fs.existsSync(sample)) await engine.open({ url: `${engine.base}/data/countries.geojson`, name: "Countries", zoom: false });
    let opened = null;
    if (dataset) opened = await engine.open({ url: `${engine.base}/data/${dataset}`, name: "Points", zoom: true });
    const project = await engine.project();
    const layerNames = (project.maps?.[0]?.layers ?? []).map((l) => l.name);
    const pointsLayer = dataset ? (layerNames.find((n) => n === "Points") ?? layerNames[layerNames.length - 1]) : layerNames[0];
    log(`engine ${engine.base}: layers ${JSON.stringify(layerNames)}; open ${opened ? Math.round(opened.loadMs) : 0} ms`);

    chrome = await launchChrome({ pathName, width, height, dpr });
    const { send, on } = chrome;
    const ev = async (expression, awaitPromise = false) => {
      const { result, exceptionDetails } = await send("Runtime.evaluate", { expression, awaitPromise, returnByValue: true });
      if (exceptionDetails) throw new Error(`page: ${exceptionDetails.exception?.description ?? exceptionDetails.text}`);
      return result.value;
    };
    const consoleErrors = [];
    on("Runtime.exceptionThrown", (p) => consoleErrors.length < 8 && consoleErrors.push(String(p.exceptionDetails?.exception?.description ?? p.exceptionDetails?.text).slice(0, 300)));
    await send("Page.enable");
    await send("Runtime.enable");
    await send("Page.addScriptToEvaluateOnNewDocument", { source: INIT_SCRIPT });

    const query = ["project=1", "spike=1", ...(pathName.includes("webgl2") ? ["backend=webgl2"] : [])].join("&");
    const navigatedAt = Date.now();
    await send("Page.navigate", { url: `${engine.base}/canvas/?${query}` });

    const frames = () => ev(`(() => { const w = ${VIEW}; const a = w && w.__spikeApp; return a ? a.frames.length : -1; })()`).catch(() => -1);
    const rowsFrom = async (from) => {
      const data = await ev(`(() => { const w = ${VIEW}; return { origin: w.performance.timeOrigin, rows: w.__spikeApp.frames.slice(${from}) }; })()`);
      return data.rows.map((row) => Object.assign(row, { at: data.origin + row[ROW.now] }));
    };
    // The rows recorded since `from`, and the one recorded just before them: the first row is compared with it, so a
    // change carried by the first frame after the window began is seen (see `latencies`).
    const windowFrom = async (from) => {
      const all = await rowsFrom(Math.max(0, from - 1));
      return from > 0 ? { before: all[0], rows: all.slice(1) } : { before: null, rows: all };
    };
    const deadline = Date.now() + 120000;
    let firstRow = null;
    let firstAny = null;
    while (Date.now() < deadline) {
      const n = await frames();
      if (n > 0) {
        const rows = await rowsFrom(0);
        firstAny ??= rows[0].at;
        firstRow = rows.find((r) => r[ROW.features] > 0 && r[ROW.renders] >= 1);
        if (firstRow) break;
      }
      await sleep(100);
    }
    if (!firstRow) throw new Error("the view never drew a layer with features in 120 s");
    const result = {
      path: pathName,
      canvas,
      dataset,
      viewport: { width, height, dpr },
      chrome: chrome.version.product,
      firstDraw: { viewFirstFrameMs: Math.round(firstAny - navigatedAt), firstDataMs: Math.round(firstRow.at - navigatedAt) },
      scenarios: {},
    };
    await sleep(1500);
    const probe = await ev(PROBE_EXPRESSION, true);
    const used = await ev(`(() => { const e = ${VIEW}.__eval; return { gpuSubmits: e.gpuSubmits, glDraws: e.glDraws }; })()`);
    const backend = used.gpuSubmits > 0 ? "webgpu" : used.glDraws > 0 ? "webgl2" : null;
    result.gpu = { ...probe, backendUsed: backend };
    assertPath(pathName, probe, backend);

    const mouse = (type, x, y, extra = {}) => send("Input.dispatchMouseEvent", mouseParams(type, x, y, extra));
    const cx = Math.round(width / 2);
    const cy = Math.round(height / 2);
    const begin = async () => {
      const from = await frames();
      await ev(`(() => { const e = ${VIEW}.__eval; e.raf = []; e.inputs = []; e.long = []; e.start(); })()`);
      return from;
    };
    const finish = async (from, windows, changed) => {
      await sleep(400);
      const data = await ev(`(() => { const e = ${VIEW}.__eval; e.stop(); return { raf: e.raf, inputs: e.inputs, long: e.long }; })()`);
      const { before, rows } = await windowFrom(from);
      const inWindow = (t) => windows.some(([a, b]) => t >= a && t <= b);
      const inputs = data.inputs.filter(inWindow);
      const inside = windowRows(rows, before, inWindow);
      return {
        frame: frameStats(frameIntervals(data.raf, windows)),
        latency: latencies({ inputs, samples: rows, before, ticks: data.raf, changed, windows }),
        app: appStats(inside.rows, inside.before),
        longTasks: { count: data.long.length, max: data.long.length ? Math.round(Math.max(...data.long)) : 0 },
      };
    };
    const drag = async (dx, dy, ms) => {
      await mouse("mouseMoved", cx, cy, { buttons: 0 });
      await mouse("mousePressed", cx, cy, { buttons: 1 });
      const start = Date.now();
      for (;;) {
        const f = Math.min(1, (Date.now() - start) / ms);
        await mouse("mouseMoved", Math.round(cx + dx * f), Math.round(cy + dy * f), { buttons: 1 });
        if (f >= 1) break;
        await sleep(2);
      }
      await mouse("mouseReleased", Math.round(cx + dx), Math.round(cy + dy), { buttons: 0 });
      await mouse("mouseMoved", cx, cy, { buttons: 0 });
    };
    const settle = async (quietMs = 600, maxMs = 20000) => {
      let last = await frames();
      let since = Date.now();
      const stop = Date.now() + maxMs;
      while (Date.now() < stop) {
        await sleep(100);
        const rows = await rowsFrom(Math.max(0, last - 2));
        const renders = rows.length ? rows[rows.length - 1][ROW.renders] : 0;
        const key = `${renders}:${rows.length ? rows[rows.length - 1][ROW.fast] : 0}`;
        if (settle.key !== key) (settle.key = key), (since = Date.now());
        if (Date.now() - since >= quietMs) return;
        last = await frames();
      }
    };

    if (scenarios.includes("pan")) {
      log("scenario pan");
      const from = await begin();
      const windows = [];
      for (const [dx, dy] of [[420, 0], [-420, 0], [0, 260], [0, -260], [300, 200], [-300, -200]]) {
        const a = Date.now();
        await drag(dx, dy, 1000);
        windows.push([a, Date.now() + 100]);
        await sleep(300);
      }
      result.scenarios.pan = await finish(from, windows);
      await settle();
    }
    if (scenarios.includes("zoom")) {
      log("scenario zoom");
      const from = await begin();
      const windows = [];
      await mouse("mouseMoved", cx, cy, { buttons: 0 });
      for (const delta of [-120, 120]) {
        const a = Date.now();
        for (let i = 0; i < 30; i++) {
          await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: cx, y: cy, deltaX: 0, deltaY: delta, buttons: 0 });
          await sleep(40);
        }
        windows.push([a, Date.now() + 100]);
        await sleep(500);
      }
      result.scenarios.zoom = await finish(from, windows);
      await settle();
    }
    const bridge = (command) =>
      ev(`window.postMessage({ observatory: "bridge", protocolVersion: 1, id: ${Date.now() % 1e9}, command: ${JSON.stringify(command)} }, "*")`);
    if (scenarios.includes("toggle")) {
      log("scenario toggle");
      const from = await begin();
      const windows = [];
      const sent = [];
      for (let cycle = 0; cycle < 8; cycle++) {
        for (const visible of [false, true]) {
          const a = Date.now();
          sent.push(await ev(`(() => { window.postMessage({ observatory: "bridge", protocolVersion: 1, id: ${cycle * 2 + (visible ? 1 : 0) + 1}, command: ${JSON.stringify({ name: "layers", layers: [{ name: pointsLayer, visible }] })} }, "*"); return performance.timeOrigin + performance.now(); })()`));
          await sleep(500);
          windows.push([a, a + 500]);
        }
      }
      await sleep(300);
      const data = await ev(`(() => { const e = ${VIEW}.__eval; e.stop(); return { raf: e.raf, long: e.long }; })()`);
      const { before, rows } = await windowFrom(from);
      const changed = (row, previous) => row[ROW.visible] !== previous[ROW.visible];
      result.scenarios.toggle = {
        frame: frameStats(frameIntervals(data.raf, windows)),
        latency: latencies({ inputs: sent, samples: rows, before, ticks: data.raf, changed }),
        app: appStats(rows, before),
        longTasks: { count: data.long.length, max: data.long.length ? Math.round(Math.max(...data.long)) : 0 },
      };
      await bridge({ name: "layers", layers: [{ name: pointsLayer, visible: true }] });
      await settle();
    }

    if (fidelity && dataset) {
      const xyFile = path.join(dataDir, dataset.replace(/\.[a-z]+$/, ".xy"));
      if (!fs.existsSync(xyFile)) throw new Error(`no ${path.basename(xyFile)}: the fidelity gate needs the generator's point file`);
      const home = firstRow;
      await bridge({ name: "camera", camera: { center: [home[ROW.cx], home[ROW.cy]], res: home[ROW.res] } });
      await settle();
      const shot = async () => decodePng(Buffer.from((await send("Page.captureScreenshot", { format: "png" })).data, "base64"));
      const shown = await shot();
      const last = (await rowsFrom(Math.max(0, (await frames()) - 3))).at(-1);
      await bridge({ name: "layers", layers: [{ name: pointsLayer, visible: false }] });
      await settle();
      const hidden = await shot();
      const recall = pointRecall({ ink: inkMask(shown, hidden), xy: readXy(fs.readFileSync(xyFile)), camera: { cx: last[ROW.cx], cy: last[ROW.cy], res: last[ROW.res] }, dpr });
      result.fidelity = { ...recall, bar: RECALL_BAR, pass: recall.recall !== null && recall.recall >= RECALL_BAR, featuresHeld: last[ROW.features], pointsSubmitted: last[ROW.points] };
    }
    result.consoleErrors = consoleErrors;
    return result;
  } finally {
    chrome?.close();
    engine.stop();
  }
}
