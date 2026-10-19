// Spike A (#43): the installer-size line — what the Observatory adds, raw and compressed.
//
//   node sizes.mjs                       # default paths, writes sizes.json next to it
//   node sizes.mjs --engine <exe> --web <dist/web> --extra <exe> --out sizes.json
//
// Compression is the installer question, so it is measured twice: DEFLATE (`zip -9`, what a simple
// updater or a self-extracting zip ships) and LZMA2 (`7z -mx=9`, what NSIS/Inno/7-Zip-based
// installers ship). Electron's own runtime is *not* included: the target is what the Observatory
// adds to an app that already has it.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..", "..");
function parse(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    if (!argv[i].startsWith("--")) continue;
    out[argv[i].slice(2)] = argv[i + 1];
    i++;
  }
  return out;
}
const args = parse(process.argv.slice(2));
const engine = args.engine || path.join(root, "target", "release", "syngle-observatory-engine.exe");
const web = args.web || path.join(root, "dist", "web");
const extras = args.extra ? String(args.extra).split(",") : [];
const out = args.out || path.join(here, "sizes.json");
const sevenZip = args.sevenzip || "C:\\Program Files\\7-Zip\\7z.exe";

function walk(dir, base = dir, acc = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, base, acc);
    else acc.push({ path: path.relative(base, full), bytes: fs.statSync(full).size });
  }
  return acc;
}

const sets = [];
const engineFiles = [engine].concat(extras).map((p) => ({ path: path.basename(p), bytes: fs.statSync(p).size, full: p }));
sets.push({ name: "engine", files: engineFiles });
const webFiles = walk(web);
sets.push({ name: "view (wasm + glue + assets)", files: webFiles });

function compress(name, files, dir) {
  const size = (p) => (fs.existsSync(p) ? fs.statSync(p).size : null);
  // One stage per format: archiving the stage directory that already holds the other format's
  // archive would count it twice (that is what made the first LZMA number larger than DEFLATE).
  const run = (ext, args) => {
    const stage = fs.mkdtempSync(path.join(os.tmpdir(), "spike-size-"));
    for (const f of files) {
      fs.copyFileSync(f.full || f.source || path.join(dir, f.path), path.join(stage, f.path.replace(/[\\/]/g, "_")));
    }
    const out = path.join(stage, `${name}.${ext}`);
    const r = spawnSync(sevenZip, ["a", ...args, "-bso0", "-bsp0", out, path.join(stage, "*")], { encoding: "utf8" });
    const bytes = size(out);
    fs.rmSync(stage, { recursive: true, force: true });
    return { bytes, status: r.status };
  };
  const zip = run("zip", ["-tzip", "-mx=9"]);
  const seven = run("7z", ["-t7z", "-m0=lzma2", "-mx=9"]);
  return { zip9Bytes: zip.bytes, lzma2Bytes: seven.bytes, zipStatus: zip.status, sevenStatus: seven.status };
}

const report = { engine, web, extras, measuredAt: new Date().toISOString(), sets: [] };
for (const set of sets) {
  const raw = set.files.reduce((a, f) => a + f.bytes, 0);
  const biggest = set.files.slice().sort((a, b) => b.bytes - a.bytes).slice(0, 8);
  const packed = compress(set.name.replace(/[^a-z0-9]+/gi, "-"), set.files, web);
  report.sets.push({ name: set.name, count: set.files.length, rawBytes: raw, ...packed, biggest });
}
const totalRaw = report.sets.reduce((a, s) => a + s.rawBytes, 0);
const totalZip = report.sets.reduce((a, s) => a + (s.zip9Bytes || 0), 0);
const totalLzma = report.sets.reduce((a, s) => a + (s.lzma2Bytes || 0), 0);
// The two sets compress to one archive in a real installer: measure that too.
report.total = { rawBytes: totalRaw, zip9Bytes: totalZip, lzma2Bytes: totalLzma };
const combined = compress("observatory", engineFiles.concat(webFiles.map((f) => ({ path: f.path, source: path.join(web, f.path) }))), web);
report.total.combined = combined;
fs.writeFileSync(out, JSON.stringify(report, null, 2));
const mb = (n) => (n === null ? "n/a" : (n / 1e6).toFixed(1) + " MB");
for (const s of report.sets) console.log(`${s.name}: ${s.count} files, raw ${mb(s.rawBytes)}, zip -9 ${mb(s.zip9Bytes)}, 7z lzma2 ${mb(s.lzma2Bytes)}`);
console.log(`total: raw ${mb(totalRaw)}, zip -9 ${mb(totalZip)}, 7z lzma2 ${mb(totalLzma)}, one archive ${mb(combined.zip9Bytes)} / ${mb(combined.lzma2Bytes)}`);
console.log("written " + out);
