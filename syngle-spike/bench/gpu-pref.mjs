// Spike A (#43): the per-app GPU preference, set and undone from the command line.
//
// Windows picks a GPU per executable under
// `HKCU\Software\Microsoft\DirectX\UserGpuPreferences` (GpuPreference=1 power saving, 2 high
// performance). This script writes that one value for Electron's executable only, and `restore`
// puts the previous value back — the spike leaves the user's configuration exactly as it found it.
//
//   node gpu-pref.mjs low        # Electron on the integrated GPU (the mid-range-laptop proxy)
//   node gpu-pref.mjs high       # Electron on the discrete GPU
//   node gpu-pref.mjs restore    # back to what it was
//   node gpu-pref.mjs show
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const electron = process.argv[3] || path.resolve(here, "..", "host", "node_modules", "electron", "dist", "electron.exe");
const key = "HKCU\\Software\\Microsoft\\DirectX\\UserGpuPreferences";
const stateFile = path.resolve(here, "results", "gpu-pref-state.json");
const mode = (process.argv[2] || "show").toLowerCase();

function current() {
  const out = spawnSync("reg.exe", ["query", key, "/v", electron], { encoding: "utf8" });
  if (out.status !== 0) return null;
  const m = /REG_SZ\s+(.*)/.exec(out.stdout);
  return m ? m[1].trim() : null;
}

function write(value) {
  if (value === null || value === undefined) {
    spawnSync("reg.exe", ["delete", key, "/v", electron, "/f"], { encoding: "utf8" });
    return;
  }
  spawnSync("reg.exe", ["add", key, "/v", electron, "/t", "REG_SZ", "/d", value, "/f"], { encoding: "utf8" });
}

if (mode === "show") {
  console.log(JSON.stringify({ exe: electron, value: current(), stateFile }, null, 2));
} else if (mode === "restore") {
  if (!fs.existsSync(stateFile)) {
    console.log("nothing to restore (no state file): " + stateFile);
  } else {
    const state = JSON.parse(fs.readFileSync(stateFile, "utf8"));
    write(state.previous);
    fs.unlinkSync(stateFile);
    console.log("restored " + electron + " to " + JSON.stringify(state.previous));
  }
} else if (mode === "low" || mode === "high") {
  fs.mkdirSync(path.dirname(stateFile), { recursive: true });
  const previous = current();
  if (!fs.existsSync(stateFile)) fs.writeFileSync(stateFile, JSON.stringify({ exe: electron, previous }, null, 2));
  write(mode === "low" ? "GpuPreference=1;" : "GpuPreference=2;");
  console.log(JSON.stringify({ exe: electron, was: previous, now: current() }, null, 2));
} else {
  console.error("usage: node gpu-pref.mjs low|high|restore|show [electron.exe]");
  process.exit(2);
}
