// The hill-climb scorer: runs one group of scores several times, prints each score's median and measured noise against its
// bar, saves the runs, and (given a baseline) applies the keep rule. Read-only: it never edits the product.
//
//   node evals/climb/run.mjs --group canvas:webgpu-gpu --build <dir> [--runs 5] [--save file.json]
//        [--scores-out evals/baselines/<group>.json] [--baseline evals/baselines/<file>.json] [--gates-pass] [--label "step 2: ..."] [--log evals/climb/log.md]
//   node evals/climb/run.mjs --list
//
// The method (Syngle-Ecosystem #581 and #593): score by script, a bar, a measured baseline; one logged step at a time; a
// step is kept only when the median of the runs beats the noise AND every gate passes. `--baseline` compares each score of
// the group with its baseline entry by `decide()`; `--gates-pass` asserts the gates (cargo xtask ci, the bridge contract
// tests, evals/map-perf/features.mjs) passed on this same tree, since this script cannot see them. `--rescore <saved.json>` scores runs
// saved earlier (--save) instead of running. Exit 0 measured and
// (when asked) kept; 1 a run failed; 4 measured but no score kept; 5 a score is worse than its baseline by more than the noise;
// 2 usage; 3 path mismatch.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { parseArgs } from "../lib/args.mjs";
import { decide, noiseOf } from "../lib/stats.mjs";
import { groups } from "./scores.mjs";

const args = parseArgs(process.argv.slice(2), { runs: 5 });
const all = groups();
if (args.list) {
  for (const [name, g] of Object.entries(all)) console.log(`${name}\n  ${Object.keys(g.scores).join("\n  ")}`);
  process.exit(0);
}
const group = all[args.group];
if (!group || (!args.build && !args.rescore && !args.group?.startsWith("gov"))) {
  console.error("usage: node evals/climb/run.mjs --group <name> --build <dir> [--runs 5] [--save f] [--scores-out f] [--cli "<syngle-map>" --scale <dir> --kinds a,b (gov)] [--baseline f] [--gates-pass] [--label s] [--log f]\n       --list shows the groups");
  process.exit(2);
}

const perRun = [];
const raw = [];
const saved = args.rescore ? JSON.parse(fs.readFileSync(args.rescore, "utf8")) : null;
if (saved) args.runs = saved.raw.length;
for (let i = 1; i <= args.runs; i++) {
  let result;
  if (saved) {
    // Scoring runs that were measured earlier (a score was added or its reading changed): nothing is run.
    result = saved.raw[i - 1].result;
    raw.push(saved.raw[i - 1]);
  } else {
    const tmp = path.join(os.tmpdir(), `obs-eval-run-${process.pid}-${i}.json`);
    process.stderr.write(`[run ${i}/${args.runs}] ${args.group}\n`);
    const started = Date.now();
    const child = spawnSync(process.execPath, [group.script, ...group.args({ build: args.build, cli: args.cli, scale: args.scale, kinds: args.kinds }), "--out", tmp], { stdio: ["ignore", "inherit", "inherit"] });
    if (child.status !== 0 || !fs.existsSync(tmp)) {
      console.error(`run ${i} failed (exit ${child.status})`);
      process.exit(child.status === 3 ? 3 : 1);
    }
    result = JSON.parse(fs.readFileSync(tmp, "utf8"));
    fs.rmSync(tmp, { force: true });
    raw.push({ seconds: Math.round((Date.now() - started) / 1000), result });
  }
  const values = {};
  for (const [id, spec] of Object.entries(group.scores)) {
    try {
      values[id] = spec.read(result);
    } catch {
      values[id] = null;
    }
  }
  perRun.push(values);
}

const scores = {};
for (const [id, spec] of Object.entries(group.scores)) {
  const runs = perRun.map((r) => r[id]).filter((v) => v !== null && v !== undefined);
  scores[id] = { ...noiseOf(runs), values: runs, direction: spec.direction, unit: spec.unit, bar: spec.bar };
  const meets = scores[id].median === null ? false : spec.direction === "lower" ? scores[id].median <= spec.bar : scores[id].median >= spec.bar;
  scores[id].meetsBar = meets;
}

const git = spawnSync("git", ["rev-parse", "--short", "HEAD"], { cwd: path.dirname(group.script), encoding: "utf8" });
const record = { group: args.group, label: args.label ?? null, at: new Date().toISOString(), commit: (git.stdout ?? "").trim() || null, machine: { cpu: os.cpus()[0]?.model, cores: os.cpus().length, platform: process.platform }, runs: args.runs, scores };
if (args.save) fs.writeFileSync(args.save, JSON.stringify({ ...record, raw }, null, 2));
if (args["scores-out"]) fs.writeFileSync(args["scores-out"], JSON.stringify(record, null, 2));

const pad = (v, n) => String(v ?? "-").padEnd(n);
console.log(`\n${pad("score", 38)}${pad("median", 12)}${pad("min", 10)}${pad("max", 10)}${pad("spread", 10)}${pad("bar", 10)}ok`);
for (const [id, s] of Object.entries(scores)) console.log(`${pad(id, 38)}${pad(s.median, 12)}${pad(s.min, 10)}${pad(s.max, 10)}${pad(s.spread === null ? null : Math.round(s.spread * 10) / 10, 10)}${pad((s.direction === "lower" ? "<= " : ">= ") + s.bar, 10)}${s.meetsBar ? "yes" : "no"}`);

let exit = 0;
if (args.baseline) {
  const base = JSON.parse(fs.readFileSync(args.baseline, "utf8"));
  const decisions = {};
  for (const [id, s] of Object.entries(scores)) {
    if (!base.scores?.[id]) continue;
    const b = base.scores[id];
    const noise = Math.max(b.spread ?? 0, s.spread ?? 0);
    if (b.median === null || b.median === undefined) {
      // The baseline could not measure this score (a verb that did not exist): the first value has nothing to beat.
      decisions[id] = { keep: Boolean(args["gates-pass"]) && s.median !== null && s.runs >= 3, delta: null, noise, regressed: false, reason: "first measurement: the baseline had no value" };
      continue;
    }
    const d = decide({ baseline: b, candidate: s, direction: s.direction, gatesPass: Boolean(args["gates-pass"]) });
    // Worse than the baseline by more than the noise: a step that improves one score and does this to another is not kept.
    decisions[id] = { ...d, regressed: s.median !== null && d.delta < -noise };
  }
  console.log("\nkeep rule vs baseline (median must beat the noise, every gate must pass, no score may get worse than the noise):");
  for (const [id, d] of Object.entries(decisions)) console.log(`  ${pad(id, 38)}${d.regressed ? "WORSE " : d.keep ? "KEEP  " : "revert"} gain ${d.delta === null ? "n/a" : Math.round(d.delta * 100) / 100} noise ${Math.round(d.noise * 100) / 100}: ${d.regressed ? "worse than the baseline by more than the noise" : d.reason}`);
  const headline = Object.values(decisions);
  if (headline.some((d) => d.regressed)) exit = 5;
  else if (headline.length && !headline.some((d) => d.keep)) exit = 4;
  record.decisions = decisions;
}
if (args.log) {
  const rows = Object.entries(scores).map(([id, s]) => `| ${record.label ?? ""} | ${id} | ${s.median} | ${s.min} to ${s.max} | ${s.meetsBar ? "yes" : "no"} |`);
  if (!fs.existsSync(args.log)) fs.writeFileSync(args.log, "| step | score | median | range over runs | meets bar |\n|---|---|---|---|---|\n");
  fs.appendFileSync(args.log, `${rows.join("\n")}\n`);
}
process.exit(exit);
