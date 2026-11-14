// Evaluator: the Australian government data lookup (`syngle-map gov ...`) against its contract. Read-only; it writes only
// into a temporary data folder it creates and removes (SYNGLE_GOV_DIR), never the installed Syngle's.
//
//   node evals/gov-data/lookup.mjs [--fixtures <dir>] [--scale <dir>] [--out file] -- <the command that runs syngle-map>
//   e.g. -- C:\...\syngle-map.exe      or, to test this evaluator: -- node evals/gov-data/reference/naive.mjs
//   (`--cli "<command>"` also works where the shell keeps the quotes.)
//
// Scores (percent unless noted): golden (the answers in fixtures/golden.json), determinism (byte-identical answers from a
// second index built from the same files, from another working folder, under another time zone and locale), versioned (every
// answer states its sources, as-of dates, licence and attribution, and the data version matches `gov status`), lifecycle
// (refresh, unchanged refresh, a newer refresh, a pinned old version, a corrupt refresh that must change nothing), hostile
// (odd input is answered with JSON and exit 0 or 2, never a crash). `--kinds` scores a stage. With `--scale` it also times each verb as a person's
// agent calls it, one process per call (process start included), and the index build.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "../lib/args.mjs";
import { percentile, summarize } from "../lib/stats.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const dash = argv.indexOf("--");
const args = parseArgs(dash < 0 ? argv : argv.slice(0, dash), { fixtures: path.join(here, "fixtures") });
if (dash >= 0) args.cli = argv.slice(dash + 1).map((t) => (/\s/.test(t) ? `"${t}"` : t)).join(" ");
if (!args.cli) {
  console.error('usage: node evals/gov-data/lookup.mjs [--fixtures dir] [--scale dir] [--kinds a,b] [--out file] -- <syngle-map command...>');
  process.exit(2);
}
const cli = String(args.cli).trim().match(/"[^"]+"|\S+/g).map((t) => t.replace(/^"|"$/g, "")).map((t) => (/\.mjs$/.test(t) ? path.resolve(t) : t));
const golden = JSON.parse(fs.readFileSync(path.join(args.fixtures, "golden.json"), "utf8"));
// `--kinds abn,name,company` scores a stage: only those golden kinds run, and only the sources they need must be indexed.
// The kinds: abn, name, company (also business names), address, stats, area. Without it, everything runs.
const NEEDS = { abn: ["abr"], name: ["abr", "asic", "asicBn"], company: ["asic", "asicBn"], address: ["gnaf"], stats: ["abs"], area: ["asgs"] };
const kinds = typeof args.kinds === "string" && args.kinds !== "all" ? args.kinds.split(",") : Object.keys(NEEDS);
if (kinds.some((k) => !NEEDS[k])) {
  console.error(`unknown kind in --kinds; one of ${Object.keys(NEEDS).join(", ")}`);
  process.exit(2);
}
golden.cases = golden.cases.filter((c) => kinds.includes(c.kind));
const expectedSources = [...new Set(kinds.flatMap((k) => NEEDS[k]))];
const work = fs.mkdtempSync(path.join(os.tmpdir(), "obs-gov-eval-"));
process.on("exit", () => fs.rmSync(work, { recursive: true, force: true }));

function call(dir, gov, { cwd = work, env = {}, timeoutMs = 120000 } = {}) {
  const t = performance.now();
  const r = spawnSync(cli[0], [...cli.slice(1), "gov", ...gov], { cwd, env: { ...process.env, ...env, SYNGLE_GOV_DIR: dir }, encoding: "utf8", timeout: timeoutMs, maxBuffer: 64 * 1024 * 1024 });
  let json = null;
  try {
    json = JSON.parse(r.stdout);
  } catch {
    // Not JSON: the checks below say so.
  }
  return { status: r.status, signal: r.signal, stdout: r.stdout ?? "", stderr: (r.stderr ?? "") + (r.error ? ` ${r.error.message}` : ""), json, ms: performance.now() - t, bytes: Buffer.byteLength(r.stdout ?? "") };
}

const get = (obj, dotted) => dotted.split(".").reduce((o, k) => (o === null || o === undefined ? undefined : o[k]), obj);
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const fixture = (v) => path.join(args.fixtures, v);
const rate = (passed, total) => (total ? Math.round((passed / total) * 1000) / 10 : 0);
const answered = (text) => {
  try {
    const j = JSON.parse(text);
    return j !== null && typeof j === "object" && j.ok === true;
  } catch {
    return false;
  }
};

const dirA = path.join(work, "A");
const dirB = path.join(work, "B");
const result = { cli: args.cli, golden: {}, determinism: {}, versioned: {}, lifecycle: {}, hostile: {}, answerBytes: {}, latency: null };

// --- 1. refresh v1 into two separate folders ---------------------------------------------------------------------
const tBuild = performance.now();
const r1a = call(dirA, ["refresh", "--from", fixture("v1")]);
const buildMs = performance.now() - tBuild;
const r1b = call(dirB, ["refresh", "--from", fixture("v1")]);
const lifecycle = [];
const check = (name, pass, detail = "") => lifecycle.push({ name, pass: Boolean(pass), detail: String(detail).slice(0, 200) });
check("refresh from official-format files succeeds", r1a.json?.ok === true && r1a.status === 0, r1a.stdout.slice(0, 160) || r1a.stderr.slice(0, 200));
const v1 = r1a.json?.dataVersion;
check("the data version is the same when the same files are indexed again elsewhere", v1 && v1 === r1b.json?.dataVersion, `${v1} vs ${r1b.json?.dataVersion}`);
const has = (list, name) => Array.isArray(list) && list.some((s) => s.source === name);
check("every source in the stage reports a version, an as-of date and a record count", expectedSources.every((n) => has(r1a.json?.sources, n)) && r1a.json.sources.filter((s) => expectedSources.includes(s.source)).every((s) => s.dataVersion && /^\d{4}-\d\d-\d\d$/.test(s.asOf) && s.records > 0), JSON.stringify(r1a.json?.sources)?.slice(0, 160));
const again = call(dirA, ["refresh", "--from", fixture("v1")]);
check("refreshing unchanged files changes nothing", again.json?.ok === true && again.json?.built === false && again.json?.dataVersion === v1, again.stdout.slice(0, 160));

// --- 2. golden at v1, determinism against B and under other conditions, versioned ---------------------------------
const status = call(dirA, ["status"]);
check("status names the version and every source in the stage", status.json?.dataVersion === v1 && expectedSources.every((n) => status.json?.sources?.[n]?.dataVersion), status.stdout.slice(0, 160));
const failed = [];
let goldenPass = 0;
let goldenTotal = 0;
const byKind = {};
let detPass = 0;
let detTotal = 0;
let verPass = 0;
let verTotal = 0;
const sizes = [];
const answersV1 = new Map();
const runCases = (version, dir) => {
  for (const c of golden.cases.filter((x) => x.v === version)) {
    const a = call(dir, c.args);
    goldenTotal++;
    byKind[c.kind] ??= { passed: 0, total: 0 };
    byKind[c.kind].total++;
    const wrong = Object.entries(c.expect).filter(([k, want]) => !same(get(a.json, k), want));
    if (!wrong.length && a.json && (a.json.ok ? a.status === 0 : a.status === 2)) (goldenPass++, byKind[c.kind].passed++);
    else failed.push({ id: c.id, got: a.stdout.slice(0, 200), wrong: wrong.map(([k, w]) => `${k}: want ${JSON.stringify(w)} got ${JSON.stringify(get(a.json, k))}`).slice(0, 3) });
    if (a.json?.ok) {
      verTotal++;
      const s = a.json.sources ?? {};
      const entries = Object.values(s);
      if (typeof a.json.dataVersion === "string" && a.json.dataVersion.length >= 8 && entries.length > 0 && entries.every((x) => x.dataVersion && /^\d{4}-\d\d-\d\d$/.test(x.asOf) && x.licence && x.attribution) && (version !== "v1" || a.json.dataVersion === v1)) verPass++;
      if (c.args[0] !== "stats" || c.args[1] === "get") sizes.push(a.bytes);
    }
    if (version === "v1") answersV1.set(c.id, a.stdout);
  }
};
runCases("v1", dirA);
for (const c of golden.cases.filter((x) => x.v === "v1")) {
  const b = call(dirB, c.args);
  const elsewhere = call(dirB, c.args, { cwd: os.tmpdir(), env: { TZ: "Pacific/Auckland", LANG: "de_DE.UTF-8", LC_ALL: "de_DE.UTF-8" } });
  detTotal++;
  const base = answersV1.get(c.id);
  // An answer that is not JSON is not a deterministic answer: two empty outputs do not count as equal.
  if (typeof base === "string" && base.length > 0 && (answered(base) || c.expect.ok === false) && b.stdout === base && elsewhere.stdout === base) detPass++;
  else if (detTotal - detPass <= 3) failed.push({ id: `determinism: ${c.id}`, got: `${b.stdout.slice(0, 100)} | ${elsewhere.stdout.slice(0, 100)}`, wrong: [] });
}

// --- 3. a newer refresh, a pinned old version, a corrupt refresh ---------------------------------------------------
const r2 = call(dirA, ["refresh", "--from", fixture("v2")]);
const v2 = r2.json?.dataVersion;
check("a newer refresh succeeds and gives a new data version", r2.json?.ok === true && v2 && v2 !== v1, `${v1} then ${v2}`);
check("status shows the newer as-of dates", call(dirA, ["status"]).json?.sources?.abr?.asOf === golden.versions.v2.abr, "");
runCases("v2", dirA);
const probe = golden.cases.find((c) => c.id === "abn exact, digits");
const pinned = call(dirA, [...probe.args, "--data-version", v1]);
check("an older version answers the same, byte for byte, when pinned", answered(pinned.stdout) && pinned.stdout === answersV1.get(probe.id), pinned.stdout.slice(0, 160));
check("an unknown pinned version is refused", (() => { const r = call(dirA, [...probe.args, "--data-version", "gov-00000000"]); return r.status === 2 && r.json?.ok === false; })(), "");
const bad = path.join(work, "corrupt");
fs.cpSync(fixture("v2"), bad, { recursive: true });
const abnFile = path.join(bad, "abn", "public_split_1_10.xml");
fs.writeFileSync(abnFile, fs.readFileSync(abnFile, "utf8").slice(0, 700));
const rb = call(dirA, ["refresh", "--from", bad]);
check("a corrupt refresh is refused", rb.json?.ok === false && rb.status === 2, rb.stdout.slice(0, 160));
const after = call(dirA, ["status"]);
const stillGood = call(dirA, ["abn", probe.args[1]]);
check("a corrupt refresh leaves the current version and its answers untouched", after.json?.dataVersion === v2 && stillGood.json?.ok === true && stillGood.json?.dataVersion === v2, after.stdout.slice(0, 160));
check("a query before any refresh says what to do", (() => { const r = call(path.join(work, "empty"), ["abn", probe.args[1]]); return r.status === 2 && r.json?.ok === false; })(), "");

// --- 4. hostile input ---------------------------------------------------------------------------------------------
let hostilePass = 0;
const hostileFailed = [];
for (const h of golden.hostile) {
  const a = call(dirA, h, { timeoutMs: 20000 });
  const ok = a.signal === null && (a.status === 0 || a.status === 2) && a.json && typeof a.json === "object" && typeof a.json.ok === "boolean";
  if (ok) hostilePass++;
  else hostileFailed.push({ args: JSON.stringify(h).slice(0, 80), status: a.status, signal: a.signal, out: a.stdout.slice(0, 80) });
}

Object.assign(result, {
  stage: { kinds, sources: expectedSources },
  golden: { passPct: rate(goldenPass, goldenTotal), passed: goldenPass, total: goldenTotal, byKind: Object.fromEntries(Object.entries(byKind).map(([k, v]) => [k, { ...v, passPct: rate(v.passed, v.total) }])), failed },
  determinism: { passPct: rate(detPass, detTotal), passed: detPass, total: detTotal },
  versioned: { passPct: rate(verPass, verTotal), passed: verPass, total: verTotal },
  lifecycle: { passPct: rate(lifecycle.filter((c) => c.pass).length, lifecycle.length), checks: lifecycle },
  hostile: { passPct: rate(hostilePass, golden.hostile.length), failed: hostileFailed },
  answerBytes: { p50: percentile(sizes, 50), p95: percentile(sizes, 95), max: Math.max(...sizes, 0) },
  fixtureBuildMs: Math.round(buildMs),
});

// --- 5. speed at scale (only with --scale <dir> from gen-scale.mjs) ------------------------------------------------
if (args.scale) {
  const queries = JSON.parse(fs.readFileSync(path.join(args.scale, "queries.json"), "utf8"));
  const dirC = path.join(work, "C");
  const t0 = performance.now();
  const built = call(dirC, ["refresh", "--from", args.scale], { timeoutMs: 3 * 3600 * 1000 });
  const buildSeconds = (performance.now() - t0) / 1000;
  if (!built.json?.ok) {
    result.latency = { error: built.stdout.slice(0, 300) };
  } else {
    const records = built.json.sources.reduce((n, s) => n + s.records, 0);
    const cold = call(dirC, ["abn", queries.abn[0]]).ms;
    const time = (list, make) => summarize(list.map((q) => call(dirC, make(q)).ms));
    const floor = summarize(Array.from({ length: 30 }, () => call(dirC, ["status"]).ms));
    result.latency = {
      scale: { records, buildSeconds: Math.round(buildSeconds), buildSecondsPerMillionRecords: Math.round((buildSeconds / records) * 1e6) },
      coldCallMs: Math.round(cold),
      floorStatus: floor,
      abn: time(queries.abn, (q) => ["abn", q]),
      name: time(queries.name, (q) => ["name", q]),
      address: time(queries.address, (q) => ["address", q]),
      stats: time(queries.stats, (q) => ["stats", "get", ...q]),
      area: time(queries.area, (q) => ["area", "--lat", String(q[0]), "--lon", String(q[1])]),
    };
  }
}

const text = JSON.stringify(result, null, 2);
if (args.out) fs.writeFileSync(args.out, text);
else console.log(text);
process.exit(0);
