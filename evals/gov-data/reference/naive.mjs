#!/usr/bin/env node
// NOT A PRODUCT. A naive, whole-file-in-memory implementation of the `syngle-map gov` contract
// (agents/docs/specs/map-hill-climb-and-gov-data.md, "The gov lookup contract"), so that:
//   1. the evaluator (evals/gov-data/lookup.mjs) is itself tested: this passes every golden case, so a failure of the real
//      build is the build's, not the evaluator's;
//   2. the contract is executable: where the spec's prose is unclear, this file says what the answer is.
// It scales to fixtures only. The real index is the Rust crate `observatory-gov` (tickets G1 to G3).
//
//   node naive.mjs gov [--data-version V] <verb> ...        env SYNGLE_GOV_DIR = where indexes live (required)
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { parseAbn, parseAbs, parseAsgs, parseAsicCompany, parseBusinessNames, parseGnaf } from "./parse.mjs";

const LICENCE = {
  abr: ["CC BY 3.0 AU", "Contains data from the Australian Business Register, licensed under CC BY 3.0 AU."],
  asic: ["CC BY 3.0 AU", "Contains ASIC company register data, licensed under CC BY 3.0 AU."],
  asicBn: ["CC BY 3.0 AU", "Contains ASIC business names register data, licensed under CC BY 3.0 AU."],
  gnaf: ["CC BY 4.0 (Open G-NAF EULA)", "G-NAF (c) Geoscape Australia licensed by the Commonwealth of Australia under the Open G-NAF End User Licence Agreement."],
  abs: ["CC BY 4.0", "Source: Australian Bureau of Statistics."],
  asgs: ["CC BY 4.0", "Source: Australian Bureau of Statistics (ASGS)."],
};
const USES = { abn: ["abr"], name: ["abr", "asic", "asicBn"], company: ["asic"], "business-names": ["asicBn"], address: ["gnaf"], stats: ["abs"], area: ["asgs"] };
const fail = (code, message, query = {}) => ({ ok: false, error: { code, message }, query });
const out = (obj, exit = 0) => (process.stdout.write(`${JSON.stringify(obj)}\n`), process.exit(exit));

const root = process.env.SYNGLE_GOV_DIR;
if (!root) out(fail("no-data-dir", "set SYNGLE_GOV_DIR"), 2);
const snapDir = (id) => path.join(root, "snapshots", id);
const currentId = () => (fs.existsSync(path.join(root, "current")) ? fs.readFileSync(path.join(root, "current"), "utf8").trim() : null);

// ---- refresh ----------------------------------------------------------------------------------------------------
function refresh(argv) {
  const from = argv[argv.indexOf("--from") + 1];
  if (argv.indexOf("--from") < 0 || !from || !fs.existsSync(path.join(from, "manifest.json"))) return fail("bad-source", "refresh needs --from <dir with manifest.json>");
  try {
    const manifest = JSON.parse(fs.readFileSync(path.join(from, "manifest.json"), "utf8"));
    const sources = {};
    const index = {};
    const records = {};
    for (const [name, meta] of Object.entries(manifest.sources)) {
      const hash = crypto.createHash("sha256");
      const walk = (p) => (fs.statSync(p).isDirectory() ? fs.readdirSync(p).sort().flatMap((f) => walk(path.join(p, f))) : [p]);
      for (const f of meta.files.flatMap((f) => walk(path.join(from, f)))) hash.update(path.relative(from, f).replaceAll("\\", "/")).update(fs.readFileSync(f));
      sources[name] = { version: `${name}-${meta.asOf}-${hash.digest("hex").slice(0, 8)}`, asOf: meta.asOf, licence: LICENCE[name][0], attribution: LICENCE[name][1], publisher: meta.publisher };
      const file = (i) => path.join(from, meta.files[i]);
      if (name === "abr") (index.abn = parseAbn(file(0))), (records[name] = index.abn.length);
      if (name === "asic") (index.company = parseAsicCompany(file(0))), (records[name] = index.company.length);
      if (name === "asicBn") (index.bn = parseBusinessNames(file(0))), (records[name] = index.bn.length);
      if (name === "gnaf") (index.addresses = parseGnaf(file(0))), (records[name] = index.addresses.length);
      if (name === "abs") (index.abs = parseAbs(path.dirname(file(0)))), (records[name] = index.abs.flows.length);
      if (name === "asgs") (index.areas = parseAsgs(path.dirname(file(0)))), (records[name] = index.areas.length);
    }
    const id = `gov-${crypto.createHash("sha256").update(Object.values(sources).map((s) => s.version).sort().join("\n")).digest("hex").slice(0, 8)}`;
    const unchanged = fs.existsSync(snapDir(id));
    if (!unchanged) {
      const tmp = `${snapDir(id)}.tmp`;
      fs.rmSync(tmp, { recursive: true, force: true });
      fs.mkdirSync(tmp, { recursive: true });
      fs.writeFileSync(path.join(tmp, "index.json"), JSON.stringify({ id, sources, index }));
      fs.renameSync(tmp, snapDir(id));
    }
    fs.writeFileSync(path.join(root, "current"), id);
    return { ok: true, dataVersion: id, built: !unchanged, sources: Object.entries(sources).map(([source, s]) => ({ source, dataVersion: s.version, asOf: s.asOf, records: records[source] })) };
  } catch (e) {
    return fail("refresh-failed", String(e.message).slice(0, 200));
  }
}

// ---- queries ----------------------------------------------------------------------------------------------------
const norm = (s) => s.toUpperCase().replaceAll("&", " AND ").replace(/[^A-Z0-9 ]+/g, " ").replace(/\s+/g, " ").trim();
const ABBR = { ST: "STREET", RD: "ROAD", AVE: "AVENUE", AV: "AVENUE", DR: "DRIVE", CT: "COURT", PDE: "PARADE", HWY: "HIGHWAY", LN: "LANE", PL: "PLACE", CL: "CLOSE", CRES: "CRESCENT", TCE: "TERRACE", BLVD: "BOULEVARD", SQ: "SQUARE" };
const DROP = new Set(["UNIT", "FLAT", "APT", "APARTMENT", "SUITE", "SHOP", "U"]);

function flagValue(args, name) {
  const i = args.indexOf(name);
  return i < 0 ? undefined : args[i + 1];
}
function limitOf(args, query) {
  const raw = flagValue(args, "--limit");
  if (raw === undefined) return 5;
  const n = Number(raw);
  if (!Number.isFinite(n) || n < 1) return fail("invalid-limit", "--limit must be a positive number", query);
  return Math.min(50, Math.floor(n));
}
const positional = (args) => args.filter((a, i) => !a.startsWith("--") && !(i > 0 && args[i - 1].startsWith("--") && !["--active"].includes(args[i - 1])));

function run(snap, verb, args) {
  const ix = snap.index;
  const query = { verb };
  if (verb === "abn") {
    const digits = (args[0] ?? "").replace(/\s+/g, "");
    query.abn = digits;
    const abnOk = (d) => d.length === 11 && ([...d].map(Number).map((n, i) => (i === 0 ? n - 1 : n) * [10, 1, 3, 5, 7, 9, 11, 13, 15, 17, 19][i]).reduce((a, b) => a + b, 0) % 89 === 0);
    if (!/^\d+$/.test(digits) || !(digits.length === 9 || abnOk(digits))) return fail("invalid-abn", "not an ABN with a valid check digit, or a 9 digit ACN", query);
    const hits = ix.abn.filter((r) => (digits.length === 11 ? r.abn === digits : r.acn === digits));
    return { ok: true, query, count: hits.length, results: hits };
  }
  if (verb === "name") {
    const text = positional(args)[0] ?? "";
    query.name = text;
    const q = norm(text);
    if (!q) return fail("empty-query", "give a name to search for", query);
    const limit = limitOf(args, query);
    if (typeof limit === "object") return limit;
    const state = flagValue(args, "--state");
    const active = args.includes("--active");
    const qTokens = q.split(" ");
    const candidates = new Map();
    const consider = (key, entity, matched, matchedType) => {
      const n = norm(matched);
      const cls = n === q ? 0 : n.startsWith(`${q} `) || n.startsWith(q) ? 1 : qTokens.every((t) => n.split(" ").includes(t)) ? 2 : 3;
      if (cls === 3) return;
      const best = candidates.get(key);
      if (!best || [cls, n.length].join() < [best.cls, best.len].join()) candidates.set(key, { cls, len: n.length, matched, matchedType, entity });
    };
    const abnByNumber = new Map(ix.abn.map((r) => [r.abn, r]));
    for (const r of ix.abn) {
      consider(r.abn, r, r.name, "MN");
      for (const n of r.names) consider(r.abn, r, n.text, n.type);
    }
    for (const c of ix.company) if (c.abn) for (const n of [c.name, ...c.formerNames]) consider(c.abn, abnByNumber.get(c.abn) ?? { abn: c.abn, name: c.name, status: c.status === "Registered" ? "Active" : "Cancelled", state: "", postcode: "", acn: c.acn }, n, "ASIC");
    for (const b of ix.bn) consider(b.abn ?? `bn:${b.name}`, b.abn && abnByNumber.get(b.abn) ? abnByNumber.get(b.abn) : { abn: b.abn, name: b.name, status: b.status === "Registered" ? "Active" : "Cancelled", state: "", postcode: "", acn: null }, b.name, "BN");
    let rows = [...candidates.values()].filter((c) => (!state || c.entity.state === state) && (!active || c.entity.status === "Active"));
    rows.sort((a, b) => a.cls - b.cls || (a.entity.status === "Active" ? 0 : 1) - (b.entity.status === "Active" ? 0 : 1) || a.len - b.len || (a.matched < b.matched ? -1 : a.matched > b.matched ? 1 : 0) || String(a.entity.abn) .localeCompare(String(b.entity.abn)));
    rows = rows.slice(0, limit).map((c) => ({ kind: c.entity.abn ? "abn" : "business-name", abn: c.entity.abn, name: c.entity.name, matched: c.matched, matchedType: c.matchedType, status: c.entity.status, state: c.entity.state, postcode: c.entity.postcode, acn: c.entity.acn ?? null }));
    return { ok: true, query, count: rows.length, results: rows };
  }
  if (verb === "company") {
    const acn = (args[0] ?? "").replace(/\s+/g, "");
    query.acn = acn;
    if (!/^\d{9}$/.test(acn)) return fail("invalid-acn", "an ACN is 9 digits", query);
    const hits = ix.company.filter((c) => c.acn === acn);
    return { ok: true, query, count: hits.length, results: hits };
  }
  if (verb === "business-names") {
    const abn = (args[0] ?? "").replace(/\s+/g, "");
    query.abn = abn;
    if (!/^\d{11}$/.test(abn)) return fail("invalid-abn", "an ABN is 11 digits", query);
    const hits = ix.bn.filter((b) => b.abn === abn).sort((a, b) => (a.name < b.name ? -1 : 1));
    return { ok: true, query, count: hits.length, results: hits };
  }
  if (verb === "address") {
    const limit = limitOf(args, query);
    if (typeof limit === "object") return limit;
    const id = flagValue(args, "--id");
    const view = (a) => ({ addressId: a.addressId, address: a.address, lat: a.lat, lon: a.lon, locality: a.locality, state: a.state, postcode: a.postcode });
    if (id !== undefined) {
      query.id = id;
      const hit = ix.addresses.filter((a) => a.addressId === id);
      return { ok: true, query, count: hit.length, results: hit.map(view) };
    }
    const text = positional(args)[0] ?? "";
    query.address = text;
    const tokens = norm(text.replaceAll("/", " ")).split(" ").filter((t) => t && !DROP.has(t));
    if (!tokens.length) return fail("empty-query", "give an address to search for", query);
    const scored = [];
    for (const a of ix.addresses) {
      const addrTokens = norm(`${a.flatNumber ?? ""} ${a.number} ${a.street} ${a.streetType} ${a.locality} ${a.state} ${a.postcode}`).split(" ");
      if (tokens.every((t) => addrTokens.includes(t) || addrTokens.includes(ABBR[t]))) scored.push({ a, extra: addrTokens.length - tokens.length });
    }
    scored.sort((x, y) => x.extra - y.extra || (x.a.addressId < y.a.addressId ? -1 : 1));
    const rows = scored.slice(0, limit).map((s) => view(s.a));
    return { ok: true, query, count: rows.length, results: rows };
  }
  if (verb === "stats") {
    const sub = args[0];
    if (sub === "find") {
      query.text = args.slice(1).join(" ");
      const tokens = norm(query.text).split(" ").filter(Boolean);
      if (!tokens.length) return fail("empty-query", "give words to search the dataflows for", query);
      const hits = ix.abs.flows.filter((f) => tokens.every((t) => norm(`${f.id} ${f.name}`).split(" ").some((w) => w.startsWith(t))));
      return { ok: true, query, count: hits.length, results: hits.slice(0, 50) };
    }
    if (sub === "get") {
      const flow = args[1] ?? "";
      query.dataflow = flow;
      const rows = ix.abs.data[flow];
      if (!rows) return fail("unknown-dataflow", `no data for ${flow} in this index`, query);
      const filters = [];
      for (let i = 0; i < args.length; i++) if (args[i] === "--filter") filters.push(args[i + 1] ?? "");
      const pairs = [];
      for (const f of filters) {
        const [k, v] = f.split("=");
        if (!k || v === undefined) return fail("invalid-filter", "--filter takes DIMENSION=VALUE", query);
        pairs.push([k, v]);
      }
      const from = flagValue(args, "--from");
      const to = flagValue(args, "--to");
      const last = flagValue(args, "--last");
      if (last !== undefined && !(Number(last) >= 1)) return fail("invalid-limit", "--last must be a positive number", query);
      let sel = rows.filter((r) => pairs.every(([k, v]) => r[k] === v) && (!from || r.TIME_PERIOD >= from) && (!to || r.TIME_PERIOD <= to));
      sel.sort((a, b) => (a.TIME_PERIOD < b.TIME_PERIOD ? -1 : a.TIME_PERIOD > b.TIME_PERIOD ? 1 : 0));
      if (last !== undefined) sel = sel.slice(-Math.floor(Number(last)));
      const skip = new Set(["DATAFLOW", "TIME_PERIOD", "OBS_VALUE", "UNIT_MEASURE", "OBS_STATUS", "DECIMALS"]);
      const res = sel.map((r) => ({ period: r.TIME_PERIOD, value: Number(r.OBS_VALUE), unit: r.UNIT_MEASURE, dims: Object.fromEntries(Object.entries(r).filter(([k]) => !skip.has(k))) }));
      return { ok: true, query, count: res.length, results: res };
    }
    return fail("unknown-verb", "stats find <words> | stats get <dataflow> [--filter D=V] [--from P] [--to P] [--last N]", query);
  }
  if (verb === "area") {
    const lat = Number(flagValue(args, "--lat"));
    const lon = Number(flagValue(args, "--lon"));
    query.lat = flagValue(args, "--lat");
    query.lon = flagValue(args, "--lon");
    if (!Number.isFinite(lat) || !Number.isFinite(lon) || Math.abs(lat) > 90 || Math.abs(lon) > 180) return fail("invalid-coordinates", "latitude within 90 and longitude within 180", query);
    const inRing = (ring) => {
      let inside = false;
      for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
        const [xi, yi] = ring[i];
        const [xj, yj] = ring[j];
        if (yi > lat !== yj > lat && lon < ((xj - xi) * (lat - yi)) / (yj - yi) + xi) inside = !inside;
      }
      return inside;
    };
    const hits = ix.areas.filter((a) => a.rings.some((poly) => inRing(poly[0]) && !poly.slice(1).some(inRing))).sort((a, b) => (a.code < b.code ? -1 : 1));
    return { ok: true, query, count: hits.length, results: hits.map((a) => ({ layer: a.layer, code: a.code, name: a.name, state: a.state })) };
  }
  return fail("unknown-verb", `no verb ${verb}`, query);
}

// ---- main -------------------------------------------------------------------------------------------------------
const argv = process.argv.slice(2);
if (argv[0] === "gov") argv.shift();
let pin = null;
const pi = argv.indexOf("--data-version");
if (pi >= 0) (pin = argv[pi + 1]), argv.splice(pi, 2);
const verb = argv[0];
if (!verb) out(fail("missing-verb", "gov refresh | status | abn | name | company | business-names | address | stats | area"), 2);
fs.mkdirSync(root, { recursive: true });
if (verb === "refresh") {
  const answer = refresh(argv.slice(1));
  out(answer, answer.ok ? 0 : 2);
}
const id = pin ?? currentId();
if (!id) out(fail("no-data", "nothing is indexed yet: run gov refresh"), 2);
if (!fs.existsSync(snapDir(id))) out(fail("unknown-data-version", `no snapshot ${id}`), 2);
const snap = JSON.parse(fs.readFileSync(path.join(snapDir(id), "index.json"), "utf8"));
if (verb === "status") out({ ok: true, dataVersion: snap.id, sources: Object.fromEntries(Object.entries(snap.sources).map(([k, s]) => [k, { dataVersion: s.version, asOf: s.asOf, licence: s.licence, publisher: s.publisher }])) });
if (!USES[verb]) out(fail("unknown-verb", `no verb ${verb}`), 2);
const answer = run(snap, verb, argv.slice(1));
if (!answer.ok) out(answer, 2);
const used = Object.fromEntries(USES[verb].filter((s) => snap.sources[s]).map((s) => [s, { dataVersion: snap.sources[s].version, asOf: snap.sources[s].asOf, licence: snap.sources[s].licence, attribution: snap.sources[s].attribution }]));
out({ ok: true, dataVersion: snap.id, sources: used, query: answer.query, count: answer.count, results: answer.results });
