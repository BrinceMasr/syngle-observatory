// Starts one headless Chrome for an evaluator and closes it, with its profile folder, when the evaluator ends (the workspace
// rule: one Chrome at a time, closed after). Never touches the installed Syngle or Chrome's own profile: the profile is a
// fresh temp folder.
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { Cdp } from "./cdp.mjs";

/**
 * The rendering paths a score is taken on. `query` is added to the canvas page's address (`?backend=webgl2` is the
 * page's own switch for the view's WebGL2 backend). `software` paths must show a software renderer: a run that lands on
 * a real GPU by mistake is refused by `assertPath` rather than reported under the wrong name.
 */
export const PATHS = {
  "webgpu-gpu": { gpu: true, query: "", flags: ["--enable-unsafe-webgpu", "--enable-gpu", "--ignore-gpu-blocklist"] },
  "webgl2-gpu": { gpu: true, query: "backend=webgl2", flags: ["--enable-gpu", "--ignore-gpu-blocklist"] },
  "webgl2-cpu": {
    gpu: false,
    query: "backend=webgl2",
    flags: ["--disable-gpu", "--enable-unsafe-swiftshader", "--use-gl=angle", "--use-angle=swiftshader"],
  },
  "webgpu-cpu": {
    gpu: false,
    query: "",
    flags: ["--enable-unsafe-webgpu", "--enable-unsafe-swiftshader", "--use-webgpu-adapter=swiftshader"],
  },
};

const SOFTWARE = /swiftshader|llvmpipe|software|basic render|softpipe/i;

export function findChrome() {
  if (process.env.CHROME && fs.existsSync(process.env.CHROME)) return process.env.CHROME;
  const home = os.homedir();
  const candidates =
    process.platform === "win32"
      ? [
          "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
          "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
          "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
        ]
      : [
          ...globSync(path.join(home, ".cache", "ms-playwright"), /^chromium_headless_shell-/, ["chrome-linux", "headless_shell"]),
          "/usr/bin/google-chrome",
          "/usr/bin/chromium",
          "/usr/bin/chromium-browser",
        ];
  const found = candidates.find((candidate) => fs.existsSync(candidate));
  if (!found) throw new Error("no Chrome found: set CHROME to a chrome executable");
  return found;
}

function globSync(dir, folderPattern, rest) {
  try {
    return fs
      .readdirSync(dir)
      .filter((name) => folderPattern.test(name))
      .sort()
      .reverse()
      .map((name) => path.join(dir, name, ...rest));
  } catch {
    return [];
  }
}

function killTree(pid) {
  if (!pid) return;
  try {
    if (process.platform === "win32") spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
    else process.kill(-pid, "SIGKILL");
  } catch {
    // Already gone.
  }
}

/**
 * Launches Chrome for `pathName` (a key of PATHS) and attaches to one blank page. Resolves to
 * `{ cdp, send, on, version, close }`: `send` and `on` talk to the page; `close` is idempotent and also runs when the
 * process exits or is interrupted, so a crashed evaluator leaves no Chrome behind.
 */
export async function launchChrome({ pathName, width = 1280, height = 800, dpr = 1, extraFlags = [] }) {
  const spec = PATHS[pathName];
  if (!spec) throw new Error(`unknown path ${pathName}; one of ${Object.keys(PATHS).join(", ")}`);
  const exe = findChrome();
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), "obs-eval-chrome-"));
  const args = [
    "--headless=new",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    `--window-size=${width},${height}`,
    `--force-device-scale-factor=${dpr}`,
    "--hide-scrollbars",
    "--mute-audio",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    "--disable-background-timer-throttling",
    "--disable-renderer-backgrounding",
    "--disable-backgrounding-occluded-windows",
    "--disable-features=CalculateNativeWinOcclusion",
    ...spec.flags,
    ...extraFlags,
    "about:blank",
  ];
  const child = spawn(exe, args, { stdio: "ignore", detached: process.platform !== "win32", windowsHide: true });
  let closed = false;
  let cdp = null;
  const close = () => {
    if (closed) return;
    closed = true;
    try {
      cdp?.close();
    } catch {
      // Closing anyway.
    }
    killTree(child.pid);
    for (let attempt = 0; attempt < 5; attempt++) {
      try {
        fs.rmSync(profile, { recursive: true, force: true });
        break;
      } catch {
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 300);
      }
    }
  };
  process.once("exit", close);
  for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => (close(), process.exit(130)));

  try {
    const portFile = path.join(profile, "DevToolsActivePort");
    const deadline = Date.now() + 30000;
    while (!fs.existsSync(portFile)) {
      if (Date.now() > deadline) throw new Error("Chrome did not open its DevTools port in 30 s");
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    const [port, browserPath] = fs.readFileSync(portFile, "utf8").trim().split(/\r?\n/);
    cdp = await Cdp.connect(`ws://127.0.0.1:${port}${browserPath}`);
    const version = await cdp.send("Browser.getVersion");
    const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
    return {
      cdp,
      sessionId,
      version: { product: version.product, userAgent: version.userAgent },
      send: (method, params, timeoutMs) => cdp.send(method, params, sessionId, timeoutMs),
      on: (method, handler) => cdp.on(method, (params, session) => (!session || session === sessionId) && handler(params)),
      close,
    };
  } catch (error) {
    close();
    throw error;
  }
}

/**
 * What the page really got: the WebGPU adapter (null when there is none) and the WebGL2 renderer string. Evaluated in
 * the page after it has loaded.
 */
export const PROBE_EXPRESSION = `(async () => {
  const out = { webgpu: null, webgl2: null };
  try {
    const adapter = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
    if (adapter) {
      const info = adapter.info || (adapter.requestAdapterInfo ? await adapter.requestAdapterInfo() : {});
      out.webgpu = { vendor: info.vendor || "", architecture: info.architecture || "", device: info.device || "", description: info.description || "", fallback: Boolean(adapter.isFallbackAdapter) };
    }
  } catch (e) { out.webgpuError = String(e); }
  try {
    const gl = document.createElement("canvas").getContext("webgl2");
    if (gl) {
      const ext = gl.getExtension("WEBGL_debug_renderer_info");
      out.webgl2 = ext ? String(gl.getParameter(ext.UNMASKED_RENDERER_WEBGL)) : String(gl.getParameter(gl.RENDERER));
    }
  } catch (e) { out.webgl2Error = String(e); }
  return out;
})()`;

/** Throws when the probe does not match the path asked for: a "cpu" path on a real GPU, or a "gpu" path on software. */
export function assertPath(pathName, probe, backendUsed) {
  const spec = PATHS[pathName];
  const wantWebgpu = pathName.startsWith("webgpu");
  // A WebGPU path is judged on the WebGPU adapter, a WebGL2 path on the WebGL2 renderer.
  const text = wantWebgpu
    ? probe.webgpu
      ? `${probe.webgpu.vendor} ${probe.webgpu.architecture} ${probe.webgpu.device} ${probe.webgpu.description}`
      : "no WebGPU adapter"
    : String(probe.webgl2 ?? "no WebGL2");
  const software = SOFTWARE.test(text) || Boolean(wantWebgpu && probe.webgpu?.fallback);
  const problems = [];
  if (wantWebgpu && !probe.webgpu) problems.push(`path ${pathName} has no WebGPU adapter`);
  if (spec.gpu && software) problems.push(`path ${pathName} landed on a software renderer: ${text.trim()}`);
  if (!spec.gpu && !software) problems.push(`path ${pathName} landed on a real GPU: ${text.trim()}`);
  if (backendUsed && wantWebgpu && backendUsed !== "webgpu") problems.push(`path ${pathName} drew with ${backendUsed}, not webgpu`);
  if (backendUsed && !wantWebgpu && backendUsed === "webgpu") problems.push(`path ${pathName} drew with webgpu`);
  if (problems.length) throw Object.assign(new Error(problems.join("; ")), { code: "PATH_MISMATCH" });
}
