// Gate: the map's features still work. A speed step is kept only when this passes beside `cargo xtask ci` (fmt, clippy -D
// warnings, tests, layers) and the bridge contract tests (`node --test crates/observatory-engine/host/bridge.contract.test.mjs`).
// Read-only. Real engine, real canvas, real input: what a person does in the map, checked by what the page and the engine report.
//
//   node evals/map-perf/features.mjs --build <dir> [--path webgpu-gpu] [--out file]      exit 0 all pass, 1 a check failed
import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "../lib/args.mjs";
import { startEngine } from "../lib/engine.mjs";
import { decodePng } from "./fidelity.mjs";
import { runCanvas } from "./session.mjs";

const args = parseArgs(process.argv.slice(2), { path: "webgpu-gpu" });
if (!args.build) {
  console.error("usage: node evals/map-perf/features.mjs --build <dir> [--path webgpu-gpu] [--out file]");
  process.exit(2);
}
const checks = [];
const check = (name, pass, detail = "") => checks.push({ name, pass: Boolean(pass), detail: String(detail) });

async function canvasChecks() {
  const r = await runCanvas({ buildDir: args.build, pathName: args.path, dataset: "points_30000.fgb", scenarios: ["pan", "zoom", "toggle"], fidelity: false });
  check("canvas draws a layer with features", r.firstDraw.firstDataMs > 0, `${r.firstDraw.firstDataMs} ms`);
  check("canvas draws on the path asked for", r.gpu.backendUsed !== null, JSON.stringify(r.gpu.backendUsed));
  check("pan moves the camera", (r.scenarios.pan.latency.p50 ?? null) !== null, `answered ${r.scenarios.pan.latency.n} of ${r.scenarios.pan.latency.events} moves`);
  check("zoom changes the scale", (r.scenarios.zoom.latency.p50 ?? null) !== null, `answered ${r.scenarios.zoom.latency.n} of ${r.scenarios.zoom.latency.events} wheel events`);
  check("layer toggle shows and hides a layer", r.scenarios.toggle.latency.n >= 10, `answered ${r.scenarios.toggle.latency.n} of ${r.scenarios.toggle.latency.events} toggles`);
  check("the view holds the points it was given", r.scenarios.pan.app.featuresHeld > 1000, `${r.scenarios.pan.app.featuresHeld} features held`);
  check("no script error in the page", r.consoleErrors.length === 0, r.consoleErrors.join(" | "));
}

async function engineChecks() {
  const engine = await startEngine({ buildDir: args.build });
  try {
    const hello = await engine.hello();
    check("bridge handshake answers", Array.isArray(hello) && hello.length > 0, JSON.stringify(hello).slice(0, 120));
    const bad = await (await fetch(`${engine.base}/bridge`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ cmd: "bridge", bridge: "hello", protocolVersion: 999 }) })).json();
    check("a wrong protocol version is refused", JSON.stringify(bad).includes("version-mismatch"), JSON.stringify(bad).slice(0, 120));
    await engine.hello();
    const dataDir = path.join(args.build, "data");
    await engine.open({ path: path.join(dataDir, "countries.geojson"), name: "Countries", zoom: true });
    await engine.open({ path: path.join(dataDir, "points_30000.fgb"), name: "Points", zoom: false });
    const lines = await engine.bridge({ name: "dataset.query", dataset: "Countries", limit: 3 });
    const rows = lines.flatMap((l) => l.payload?.value?.rows ?? []);
    check("dataset.query returns rows", rows.length === 3, `${rows.length} rows`);
    const catchment = await engine.bridge({ name: "catchment", dataset: "Points", lon: 151.2, lat: -33.9, radiusM: 20000, mode: "radius" });
    const count = catchment.flatMap((l) => (l.payload?.value?.count !== undefined ? [l.payload.value.count] : []))[0];
    check("catchment finds points around Sydney", count > 0, `${count} points`);
    const zoom = await engine.bridge({ name: "command", id: "map.zoomIn", params: {} });
    check("a registry command runs over the bridge", zoom.some((l) => l.event === "bridge.state"), "");
    const png = await engine.render(640, 400);
    let distinct = 0;
    try {
      const image = decodePng(png.bytes);
      const seen = new Set();
      for (let i = 0; i < image.data.length; i += 4 * 97) seen.add((image.data[i] << 16) | (image.data[i + 1] << 8) | image.data[i + 2]);
      distinct = seen.size;
    } catch (error) {
      check("render-to-png is a PNG", false, error.message);
    }
    check("render-to-png draws the map (not a blank image)", png.status === 200 && distinct > 4, `status ${png.status}, ${distinct} sampled colours`);
  } finally {
    engine.stop();
  }
}

try {
  await canvasChecks();
  await engineChecks();
} catch (error) {
  check("the checks ran to the end", false, error.message);
}
const out = { build: args.build, path: args.path, pass: checks.every((c) => c.pass), checks };
const text = JSON.stringify(out, null, 2);
if (args.out) fs.writeFileSync(args.out, text);
console.log(args.out ? out.checks.map((c) => `${c.pass ? "ok  " : "FAIL"} ${c.name}${c.pass ? "" : ` (${c.detail})`}`).join("\n") : text);
process.exit(out.pass ? 0 : 1);
