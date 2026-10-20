// The canvas page's half of the Observatory bridge, against the shipped schema (Syngle-Ecosystem
// #67).
//
// The page is the bridge's view peer: `host/canvas.js` is the file the engine serves at
// `/canvas/canvas.js`, and this test runs that file — not a copy of it, not a stand-in — inside a
// sealed window pair: a host window that receives what the page posts, and a framed "view" that
// receives what the page pushes down the fork's own `rustgis` contract. Every command it drives
// comes out of the schema's own `view.commands` list, so a command the schema gains without the
// page implementing it fails here, and a command the page answers that the schema does not name
// fails here too.
//
// It runs on Node's built-in test runner (`node --test host/`), so the fork takes on no test
// dependency for it: this crate's own gates are cargo's, and this one is the JavaScript half of a
// bridge whose other half is Rust.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import test from "node:test";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
const SCHEMA = JSON.parse(readFileSync(join(here, "..", "bridge", "observatory-bridge.schema.json"), "utf8"));
const PAGE = readFileSync(join(here, "canvas.js"), "utf8");
const ORIGIN = "http://127.0.0.1:7331";
const TOKEN_PATH = "/t/contract-5f2a";

/** A tiny PNG: the signature and an IHDR of the size asked for. */
function png(width, height) {
  const bytes = new Uint8Array(33);
  bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52]);
  bytes.set([(width >> 24) & 0xff, (width >> 16) & 0xff, (width >> 8) & 0xff, width & 0xff], 16);
  bytes.set([(height >> 24) & 0xff, (height >> 16) & 0xff, (height >> 8) & 0xff, height & 0xff], 20);
  return bytes;
}

/** The project the view holds: two layers, a camera, one selected row. The fork's own shape. */
function project() {
  return {
    name: "Test",
    active_map: 0,
    maps: [
      {
        name: "Map",
        basemap: "ocean",
        projection: "flat",
        camera: { center: { x: 0, y: 0 }, res: 40000, bearing: 0, pitch: 0 },
        layers: [
          { id: 1, name: "Countries", visible: true, selection: [], dataset: { features: [], fields: [] }, renderer: {} },
          { id: 2, name: "Cities", visible: true, selection: [3], dataset: { features: [], fields: [] }, renderer: {} },
        ],
      },
    ],
  };
}

/**
 * The page in a sealed window pair: what the page posts to its host, what it pushes into the view,
 * and the addresses it fetches. Returns a driver for both directions.
 */
function mount(options = {}) {
  const toHost = [];
  // The target origin of each post to the host, in step with `toHost`: a reply must be addressed to
  // the host that spoke, never to `*` (Syngle-Ecosystem #67 review, non-blocking 2).
  const targets = [];
  const toView = [];
  const fetched = [];
  const listeners = [];
  const parent = {
    postMessage: (message, target) => {
      targets.push(target);
      toHost.push(JSON.parse(JSON.stringify(message)));
    },
  };
  const frameWindow = {
    postMessage: (message) => toView.push(JSON.parse(JSON.stringify(message))),
    document: { getElementById: () => null },
    __spikeApp: null,
    __spike: null,
  };
  const window = {
    parent,
    location: { href: `${ORIGIN}${TOKEN_PATH}/canvas/?project=1` },
    addEventListener: (type, fn) => {
      if (type === "message") listeners.push(fn);
    },
    setTimeout: () => 0,
    btoa: (text) => Buffer.from(text, "binary").toString("base64"),
  };
  const fetch = async (address) => {
    fetched.push(address);
    if (options.fetch) return options.fetch(address);
    const body = options.project ? JSON.stringify(options.project) : JSON.stringify(project());
    return { ok: true, status: 200, text: async () => body, arrayBuffer: async () => png(640, 400).buffer };
  };
  const sandbox = { window, document: { getElementById: () => ({ contentWindow: frameWindow, src: "/web/?embed" }) }, URL, fetch, JSON, Uint8Array, Array, Object, String, Boolean, Error, console };
  sandbox.window.setTimeout = () => 0;
  vm.runInNewContext(PAGE, sandbox, { filename: "canvas.js" });

  const host = (message, origin = ORIGIN) => {
    for (const fn of listeners) fn({ data: message, source: parent, origin });
  };
  const view = (message) => {
    for (const fn of listeners) fn({ data: message, source: frameWindow, origin: ORIGIN });
  };
  const drain = () => {
    const host = toHost.splice(0, toHost.length);
    const addresses = targets.splice(0, targets.length);
    const pushed = toView.splice(0, toView.length);
    return { host, targets: addresses, pushed };
  };
  return { toHost, toView, fetched, host, view, frameWindow, drain };
}

/**
 * A value a required member will accept where the schema's own type is not enough: the camera's
 * centre is a pair of numbers, a layer name has to exist in the project. Every key here has to be
 * a command the schema still names (asserted below), so a renamed command cannot hide in here.
 */
const VALUES = {
  camera: { camera: { center: [0, 0], res: 10 } },
  layers: { layers: [] },
  selection: { layer: "Countries", rows: [1] },
  style: { layer: "Countries", style: {} },
};

/** A valid instance of one schema command, from the schema's own required members. */
function instance(name, overrides = {}) {
  const spec = SCHEMA.view.commands[name];
  const value = { name };
  for (const key of spec.required ?? []) {
    value[key] = overrides[key] ?? VALUES[name]?.[key] ?? placeholder(spec, key);
  }
  for (const key of Object.keys(overrides)) value[key] = overrides[key];
  return value;
}

function placeholder(spec, key) {
  const prop = spec.properties[key] ?? {};
  if (prop.enum) return prop.enum[0];
  if (prop.type === "array") return [];
  if (prop.type === "object") {
    const value = {};
    for (const inner of prop.required ?? []) value[inner] = placeholder(prop, inner);
    return value;
  }
  return "";
}

/** The page's answer to one command: its state line (and anything else it said on the way). */
async function command(mounted, name, overrides = {}, id = 1) {
  mounted.drain();
  mounted.host({ observatory: "bridge", bridge: "command", protocolVersion: SCHEMA.protocolVersion, id, command: instance(name, overrides) });
  // A command that fetches settles on a later turn.
  await new Promise((resolve) => setTimeout(resolve, 0));
  return mounted.drain();
}

function seed(mounted) {
  mounted.view({ rustgis: "project", text: JSON.stringify(project()) });
  mounted.drain();
}

test("the page says hello with the schema's version and a state the schema requires", () => {
  const mounted = mount();
  const hello = mounted.toHost.find((m) => m.bridge === "hello");
  assert.ok(hello, "the page says hello on load");
  assert.equal(hello.observatory, "bridge");
  assert.equal(hello.protocolVersion, SCHEMA.protocolVersion, "the page and the schema are one version");
  for (const key of SCHEMA.view.state.required) assert.ok(key in hello.state, `the hello carries ${key}`);
});

test("a host on another version is refused, and served nothing after it", async () => {
  const mounted = mount();
  seed(mounted);
  mounted.host({ observatory: "bridge", bridge: "command", protocolVersion: SCHEMA.protocolVersion + 1, id: 1, command: instance("theme", { theme: "dark" }) });
  const refused = mounted.drain();
  const error = refused.host.find((m) => m.bridge === "error");
  assert.ok(error, "the refusal is an error line");
  assert.equal(error.payload.code, "version-mismatch");
  assert.match(error.payload.message, new RegExp(String(SCHEMA.protocolVersion)));
  assert.equal(refused.pushed.length, 0, "nothing reached the view");

  // The next command on the right version still gets nothing: the page stopped talking.
  mounted.host({ observatory: "bridge", bridge: "command", protocolVersion: SCHEMA.protocolVersion, id: 2, command: instance("theme", { theme: "light" }) });
  assert.equal(mounted.drain().pushed.length, 0, "the link stays shut after a mismatch");
});

test("every view command the schema names is implemented by the page", async () => {
  const mounted = mount();
  seed(mounted);
  for (const name of Object.keys(VALUES)) assert.ok(SCHEMA.view.commands[name], `VALUES names a schema command: ${name}`);
  const names = Object.keys(SCHEMA.view.commands);
  assert.deepEqual(names.sort(), ["camera", "layers", "render-to-png", "selection", "style", "theme", "tool"].sort(), "the spec's seven");
  for (const name of names) {
    const answer = await command(mounted, name);
    const refused = answer.host.find((m) => m.bridge === "error");
    assert.equal(refused, undefined, `${name} is implemented by the page: ${JSON.stringify(refused)}`);
    const state = answer.host.find((m) => m.bridge === "state" || (m.bridge === "hello"));
    assert.ok(state, `${name} answers with state`);
    for (const key of SCHEMA.view.state.required) assert.ok(key in state.state, `${name}'s state carries ${key}`);
  }
});

test("the view commands change the project the view is given, through the fork's own contract", async () => {
  const mounted = mount();
  seed(mounted);

  // camera: [x, y] metres and a resolution in, the fork's own {x, y} camera out.
  const camera = await command(mounted, "camera", { camera: { center: [1512093, -4000000], res: 1200 } });
  const pushed = camera.pushed.find((m) => m.rustgis === "project");
  assert.ok(pushed, "a new project went down to the view");
  const after = JSON.parse(pushed.text);
  assert.deepEqual(after.maps[0].camera.center, { x: 1512093, y: -4000000 });
  assert.equal(after.maps[0].camera.res, 1200);
  assert.equal(camera.host.find((m) => m.payload?.type === "camera-changed").payload.camera.res, 1200);

  // layers: order and visibility, and the names the state reports.
  const layers = await command(mounted, "layers", { layers: [{ name: "Cities", visible: false }, { name: "Countries" }], active: "Cities" });
  const ordered = JSON.parse(layers.pushed.find((m) => m.rustgis === "project").text).maps[0].layers;
  assert.deepEqual(ordered.map((l) => l.name), ["Cities", "Countries"], "the view draws what the host ordered");
  assert.equal(ordered[0].visible, false);
  assert.deepEqual(layers.host.find((m) => m.bridge === "state").state.layers, [
    { name: "Cities", visible: false },
    { name: "Countries", visible: true },
  ]);
  assert.equal(layers.host.find((m) => m.bridge === "state").state.active, "Cities");

  // style: the layer's renderer, whole.
  const style = await command(mounted, "style", { layer: "Countries", style: { Single: { fill: [200, 200, 200, 255] } } });
  const styled = JSON.parse(style.pushed.find((m) => m.rustgis === "project").text).maps[0].layers.find((l) => l.name === "Countries");
  assert.deepEqual(styled.renderer, { Single: { fill: [200, 200, 200, 255] } });

  // selection: the rows, which is the agent's scope.
  const selection = await command(mounted, "selection", { layer: "Countries", rows: [4, 9] });
  const selected = JSON.parse(selection.pushed.find((m) => m.rustgis === "project").text).maps[0].layers.find((l) => l.name === "Countries");
  assert.deepEqual(selected.selection, [4, 9]);
  assert.deepEqual(selection.host.find((m) => m.payload?.type === "selection-changed").payload, { type: "selection-changed", layer: "Countries", rows: [4, 9] });

  // theme: the fork's own `ui.theme`, and the state.
  const theme = await command(mounted, "theme", { theme: "dark" });
  const run = theme.pushed.find((m) => m.rustgis === "run");
  assert.equal(run.command, "ui.theme");
  assert.equal(run.params.dark, true);
  assert.equal(theme.host.find((m) => m.bridge === "state").state.theme, "dark");

  // tool: the bridge carries which of the eight is active.
  const tool = await command(mounted, "tool", { tool: "measure" });
  assert.equal(tool.host.find((m) => m.bridge === "state").state.tool, "measure");
});

test("render-to-png asks the engine's gateway and answers with the PNG's bytes", async () => {
  const mounted = mount({
    fetch: async (address) => {
      // The engine's `/render` is the launch-token's own address, drawn by the fork's renderer.
      assert.match(address, /\/t\/contract-5f2a\/render\?width=800&height=500$/);
      return { ok: true, status: 200, arrayBuffer: async () => png(800, 500).buffer };
    },
  });
  seed(mounted);
  const answer = await command(mounted, "render-to-png", { width: 800, height: 500 });
  const line = answer.host.find((m) => m.bridge === "state" && m.payload?.type === "render-to-png");
  assert.ok(line, "the answer is a state line with the image in it");
  assert.equal(line.payload.bytes, 33);
  const decoded = Buffer.from(line.payload.png, "base64");
  assert.equal(decoded.subarray(0, 8).toString("hex"), "89504e470d0a1a0a", "a real PNG signature");
  assert.equal(decoded.readUInt32BE(16), 800);
  assert.equal(decoded.readUInt32BE(20), 500);
});

test("a command or member the schema does not have is refused by name", async () => {
  const mounted = mount();
  seed(mounted);
  mounted.drain();
  mounted.host({ observatory: "bridge", bridge: "command", protocolVersion: SCHEMA.protocolVersion, id: 7, command: { name: "drop-layers" } });
  let answer = mounted.drain();
  assert.equal(answer.host.find((m) => m.bridge === "error").payload.code, "unknown-command");

  mounted.host({
    observatory: "bridge",
    bridge: "command",
    protocolVersion: SCHEMA.protocolVersion,
    id: 8,
    command: { name: "camera", camera: { center: [0, 0], res: 10 }, spin: true },
  });
  answer = mounted.drain();
  const refused = answer.host.find((m) => m.bridge === "error");
  assert.equal(refused.payload.code, "bad-params", JSON.stringify(refused));
  assert.equal(answer.pushed.length, 0, "a refused command changes nothing");

  // Every command refuses a member the schema does not give it: the page's own list of members and
  // the schema's properties are the same list, one command at a time.
  for (const name of Object.keys(SCHEMA.view.commands)) {
    mounted.drain();
    mounted.host({
      observatory: "bridge",
      bridge: "command",
      protocolVersion: SCHEMA.protocolVersion,
      id: 20,
      command: { ...instance(name), definitelyNotAMember: true },
    });
    const refused = mounted.drain().host.find((m) => m.bridge === "error");
    assert.equal(refused?.payload.code, "bad-params", `${name} refuses a member the schema does not have`);
  }

  // The fork's own refusal is carried, not swallowed: no such layer.
  mounted.host({ observatory: "bridge", bridge: "command", protocolVersion: SCHEMA.protocolVersion, id: 9, command: instance("style", { layer: "Nope", style: {} }) });
  answer = mounted.drain();
  assert.equal(answer.host.find((m) => m.bridge === "error").payload.code, "bad-params");
});

test("a pan the hand made comes back up as camera-changed, and a click as selection-changed", () => {
  const mounted = mount();
  seed(mounted);
  const moved = project();
  moved.maps[0].camera = { center: { x: 1000, y: 2000 }, res: 800, bearing: 0, pitch: 0 };
  moved.maps[0].layers[0].selection = [1, 2, 5];
  mounted.view({ rustgis: "project", text: JSON.stringify(moved) });
  const answer = mounted.drain();
  const events = answer.host.filter((m) => m.bridge === "event").map((m) => m.payload);
  assert.deepEqual(events.find((e) => e.type === "camera-changed").camera, { center: [1000, 2000], res: 800 });
  assert.deepEqual(events.find((e) => e.type === "selection-changed").rows, [1, 2, 5]);
  const state = answer.host.filter((m) => m.bridge === "state").pop();
  assert.deepEqual(state.state.camera, { center: [1000, 2000], res: 800 }, "the host learns the state the view has");
});

// The page's replies are addressed, not broadcast (Syngle-Ecosystem #67). Until a host has spoken the
// page has no origin to aim at, so the two messages that open the conversation go to `*`; from the
// host's first message on, everything the page posts is addressed to that origin, so a page framed
// elsewhere cannot read the map's state.
test("replies are addressed to the host's own origin once it has spoken", () => {
  const mounted = mount();
  const opening = mounted.drain();
  assert.deepEqual(opening.targets, ["*", "*"], `the opening messages: ${JSON.stringify(opening.targets)}`);
  assert.equal(opening.host[0].observatory, "ready");
  assert.equal(opening.host[1].bridge, "hello");

  mounted.host({ observatory: "hello" });
  const answer = mounted.drain();
  assert.equal(answer.host.length, 1, JSON.stringify(answer.host));
  assert.equal(answer.host[0].observatory, "ready");
  assert.deepEqual(answer.targets, [ORIGIN], "the reply goes to the host that spoke, not to '*'");

  // And a peer on another origin is still not answered at all, so nothing is addressed to it either.
  mounted.host({ observatory: "hello" }, "http://example.invalid");
  const ignored = mounted.drain();
  assert.deepEqual(ignored.host, [], "a message from a second origin is ignored");
  assert.deepEqual(ignored.targets, [], "and nothing is posted back to it");
});
