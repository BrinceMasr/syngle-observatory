// Evaluator: do the surveyed data sources work in the map? Read-only. Needs the network (it fetches the real sources).
//
//   node evals/data-sources/check.mjs --build <dir> [--out file]
//
// For every source in catalogue.json: (1) `reachable`: its probe address answers a byte-range request (200 or 206, two tries);
// (2) `opened`: the map command named in `open` runs on the real sidecar without error (the layer or tile source is added).
// Scores: reachablePct (the catalogue is still true), openedPct (how much of it the map can bring in), wantOpened (of the
// sources the survey says the map should gain, how many now open). Network failures are reported per source, not hidden:
// a source that is unreachable counts against reachablePct and is skipped for opening.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "../lib/args.mjs";
import { startEngine } from "../lib/engine.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv.slice(2));
if (!args.build) {
  console.error("usage: node evals/data-sources/check.mjs --build <dir> [--out file]");
  process.exit(2);
}
const catalogue = JSON.parse(fs.readFileSync(path.join(here, "catalogue.json"), "utf8"));

async function probe(url) {
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const reply = await fetch(url, { headers: { range: "bytes=0-1023" }, signal: AbortSignal.timeout(30000) });
      await reply.arrayBuffer();
      if (reply.status === 200 || reply.status === 206) return { ok: true, status: reply.status, range: reply.status === 206 };
      if (attempt === 1) return { ok: false, status: reply.status };
    } catch (error) {
      if (attempt === 1) return { ok: false, error: String(error.message ?? error).slice(0, 120) };
    }
  }
  return { ok: false };
}

const engine = await startEngine({ buildDir: args.build });
const rows = [];
try {
  await engine.hello();
  for (const source of catalogue.sources) {
    const reach = await probe(source.probe);
    const row = { id: source.id, group: source.group, want: source.want, reachable: reach.ok, rangeReadable: Boolean(reach.range), opened: false, openMs: null, error: null };
    if (reach.ok && source.open) {
      const t = performance.now();
      try {
        const lines = await engine.bridge({ name: "command", id: source.open.command, params: source.open.params });
        const failure = lines.find((l) => l.event === "bridge.error");
        if (failure) row.error = JSON.stringify(failure.payload ?? failure).slice(0, 200);
        else row.opened = true;
      } catch (error) {
        row.error = String(error.message ?? error).slice(0, 200);
      }
      row.openMs = Math.round(performance.now() - t);
    } else if (!reach.ok) row.error = `unreachable: ${JSON.stringify(reach)}`;
    rows.push(row);
    process.stderr.write(`[data-sources] ${row.id}: reachable=${row.reachable} opened=${row.opened}${row.error ? ` (${row.error.slice(0, 90)})` : ""}\n`);
  }
} finally {
  engine.stop();
}
const pct = (n, d) => (d ? Math.round((n / d) * 1000) / 10 : 0);
const wanted = rows.filter((r) => r.want);
const out = {
  build: args.build,
  checkedOn: catalogue.checkedOn,
  reachablePct: pct(rows.filter((r) => r.reachable).length, rows.length),
  openedPct: pct(rows.filter((r) => r.opened).length, rows.length),
  opened: rows.filter((r) => r.opened).length,
  total: rows.length,
  wantOpened: wanted.filter((r) => r.opened).length,
  wantTotal: wanted.length,
  rows,
};
const text = JSON.stringify(out, null, 2);
if (args.out) fs.writeFileSync(args.out, text);
else console.log(text);
