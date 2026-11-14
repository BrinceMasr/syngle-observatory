// Prints the baselines in evals/baselines as one Markdown table per group: the score, its median over the runs, the range and
// the spread (the noise a step must beat), the bar and whether the median meets it. Read-only.
//
//   node evals/climb/report.mjs [--dir evals/baselines] [--group canvas-webgpu-gpu]
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "../lib/args.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv.slice(2), { dir: path.join(here, "..", "baselines") });
const fmt = (v) => (v === null || v === undefined ? "n/a" : Number.isInteger(v) ? String(v) : String(Math.round(v * 10) / 10));
for (const file of fs.readdirSync(args.dir).filter((f) => f.endsWith(".json")).sort()) {
  const name = file.replace(/\.json$/, "");
  if (args.group && args.group !== name) continue;
  const b = JSON.parse(fs.readFileSync(path.join(args.dir, file), "utf8"));
  console.log(`\n### ${b.group} (${b.runs} runs, commit ${b.commit ?? "n/a"}, ${b.at?.slice(0, 10)})\n`);
  console.log("| score | median | range | spread | bar | meets bar |\n|---|---|---|---|---|---|");
  for (const [id, s] of Object.entries(b.scores)) {
    console.log(`| ${id} | ${fmt(s.median)} ${s.unit} | ${fmt(s.min)} to ${fmt(s.max)} | ${fmt(s.spread)} | ${s.direction === "lower" ? "at most" : "at least"} ${fmt(s.bar)} | ${s.meetsBar ? "yes" : "no"} |`);
  }
}
