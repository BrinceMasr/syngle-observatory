// Writes a deterministic, larger set of SYNTHETIC records in the official file shapes (see fixtures/make.mjs), for the speed
// scores of lookup.mjs. Real sizes are about 18 M ABNs, 3.5 M companies, 3 M business names, 16 M addresses; the default
// here is a thirtieth of that so a build and 1,400 timed calls fit in minutes. `--scale 1` is the real size.
//
//   node evals/gov-data/gen-scale.mjs --out <dir> [--abn 500000] [--abs-rows 100000] [--seed 7]
//   Writes <dir>/{abn,asic,gnaf,abs,asgs}/..., <dir>/manifest.json (same layout as fixtures/v1) and <dir>/queries.json
//   (the calls lookup.mjs times: ABNs that exist, names, addresses, stats keys, points, all drawn from the data).
import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "../lib/args.mjs";

const args = parseArgs(process.argv.slice(2), { abn: 500000, seed: 7, "abs-rows": 100000 });
if (!args.out) {
  console.error("usage: node evals/gov-data/gen-scale.mjs --out <dir> [--abn 500000] [--seed 7]");
  process.exit(2);
}
const N = Number(args.abn);
let state = Number(args.seed) >>> 0;
const rnd = () => {
  state = (state + 0x6d2b79f5) >>> 0;
  let t = state;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const pick = (list) => list[Math.floor(rnd() * list.length)];
const W = [10, 1, 3, 5, 7, 9, 11, 13, 15, 17, 19];
const abnOf = (nine) => {
  for (let p = 10; p <= 99; p++) {
    const d = [...`${p}${nine}`].map(Number);
    d[0] -= 1;
    if (d.reduce((s, n, i) => s + n * W[i], 0) % 89 === 0) return `${p}${nine}`;
  }
  throw new Error(nine);
};
const acnOf = (eight) => `${eight}${(10 - ([...eight].map(Number).reduce((s, n, i) => s + n * (8 - i), 0) % 10)) % 10}`;
const ADJ = "BLUE RED GREEN GOLDEN SILVER NORTHERN SOUTHERN EASTERN WESTERN CENTRAL COASTAL ALPINE URBAN RURAL BRIGHT CLEAR SWIFT STEADY PRIME ROYAL HARBOUR RIVER BUSH OUTBACK DESERT TROPICAL PACIFIC CAPITAL FEDERAL UNITED GLOBAL LOCAL SMART FRESH SOLID MODERN CLASSIC ANCIENT NEW".split(" ");
const NOUN = "GUM KANGAROO CROSS WATTLE BANKSIA EAGLE FALCON PELICAN LYREBIRD KOALA ROOFING TIMBER STEEL CONCRETE DIGITAL LOGISTICS CONSULTING BAKERY PLUMBING ELECTRICAL FREIGHT MINING FARMS ORCHARDS VINEYARDS FISHERIES TRANSPORT BUILDERS CLEANING SECURITY DESIGN ENERGY WATER MEDICAL DENTAL LEGAL ACCOUNTING PROPERTY TRAVEL".split(" ");
const SUFFIX = ["PTY LTD", "PTY LTD", "PTY LTD", "PTY. LTD.", "HOLDINGS PTY LTD", "GROUP PTY LTD", "TRUST", "& CO PTY LTD"];
const STATE = [["NSW", 31, "2"], ["VIC", 26, "3"], ["QLD", 20, "4"], ["WA", 10, "6"], ["SA", 7, "5"], ["TAS", 2, "7"], ["ACT", 2, "2"], ["NT", 2, "0"]];
const state1 = () => {
  let x = rnd() * 100;
  for (const s of STATE) if ((x -= s[1]) < 0) return s;
  return STATE[0];
};
const company = () => `${pick(ADJ)} ${pick(NOUN)} ${pick(NOUN)} ${pick(SUFFIX)}`.replace(/ (\S+) \1 /, " $1 ");
const esc = (t) => t.replaceAll("&", "&amp;");
const xmlDate = () => `20${String(10 + Math.floor(rnd() * 16)).padStart(2, "0")}${String(1 + Math.floor(rnd() * 12)).padStart(2, "0")}${String(1 + Math.floor(rnd() * 28)).padStart(2, "0")}`;
const dmy = () => `${String(1 + Math.floor(rnd() * 28)).padStart(2, "0")}/${String(1 + Math.floor(rnd() * 12)).padStart(2, "0")}/${1985 + Math.floor(rnd() * 40)}`;

function writeAll(file, make, count, batch = 5000) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const fd = fs.openSync(file, "w");
  const header = make.header?.();
  if (header) fs.writeSync(fd, header);
  for (let i = 0; i < count; i += batch) {
    const lines = [];
    for (let j = i; j < Math.min(count, i + batch); j++) lines.push(make.row(j));
    fs.writeSync(fd, lines.join(""));
  }
  const footer = make.footer?.();
  if (footer) fs.writeSync(fd, footer);
  fs.closeSync(fd);
}

const out = args.out;
const queries = { abn: [], name: [], address: [], stats: [], area: [] };
const keep = (list, value, count, i, total) => {
  if (list.length < count && i % Math.max(1, Math.floor(total / count)) === 0) list.push(value);
};

// ABN XML (+ the ACNs that the company file reuses)
const acns = [];
writeAll(path.join(out, "abn", "public_split_1_10.xml"), {
  header: () => `<?xml version="1.0" encoding="UTF-8"?>\n<Transfer error="">\n  <TransferInfo>\n    <FileSequenceNumber>1</FileSequenceNumber>\n    <RecordCount>${N}</RecordCount>\n    <ExtractTime>2026-11-05T21:34:30</ExtractTime>\n  </TransferInfo>\n`,
  row: (i) => {
    const abn = abnOf(String(100000000 + i * 37).slice(0, 9));
    const [st, , lead] = state1();
    const name = company();
    const cancelled = rnd() < 0.1;
    const hasAcn = rnd() < 0.4;
    const acn = hasAcn ? acnOf(String(10000000 + i * 3).slice(0, 8)) : null;
    if (acn && acns.length < N * 0.4) acns.push({ acn, abn, name, st });
    keep(queries.abn, abn, 200, i, N);
    if (i % Math.max(1, Math.floor(N / 200)) === 1) queries.name.push(name.split(" ").slice(0, 2).join(" "));
    return `  <ABR recordLastUpdatedDate="${xmlDate()}" replaced="N">\n    <ABN status="${cancelled ? "CAN" : "ACT"}" ABNStatusFromDate="${xmlDate()}">${abn}</ABN>\n    <EntityType><EntityTypeInd>PRV</EntityTypeInd><EntityTypeText>Australian Private Company</EntityTypeText></EntityType>\n    <MainEntity><NonIndividualName type="MN"><NonIndividualNameText>${esc(name)}</NonIndividualNameText></NonIndividualName><BusinessAddress><AddressDetails><State>${st}</State><Postcode>${lead}${String(Math.floor(rnd() * 1000)).padStart(3, "0")}</Postcode></AddressDetails></BusinessAddress></MainEntity>\n${acn ? `    <ASICNumber ASICNumberType="undetermined">${acn}</ASICNumber>\n` : ""}${rnd() < 0.6 ? `    <GST status="ACT" GSTStatusFromDate="${xmlDate()}"/>\n` : ""}${rnd() < 0.25 ? `    <OtherEntity><NonIndividualName type="TRD"><NonIndividualNameText>${esc(`${pick(ADJ)} ${pick(NOUN)}`)}</NonIndividualNameText></NonIndividualName></OtherEntity>\n` : ""}  </ABR>\n`;
  },
  footer: () => "</Transfer>\n",
}, N);

// ASIC company (TSV, BOM, CRLF) and business names
const tsvRow = (cells) => `${cells.join("\t")}\r\n`;
writeAll(path.join(out, "asic", "company_202611.csv"), {
  header: () => `\uFEFF${tsvRow(["Company Name", "ACN", "Type", "Class", "Sub Class", "Status", "Date of Registration", "Date of Deregistration", "Previous State of Registration", "State Registration number", "Modified since last report", "Current Name Indicator", "ABN", "Current Name", "Current Name Start Date"])}`,
  row: (i) => {
    const c = acns[i % acns.length];
    return tsvRow([c.name, c.acn, "APTY", "LMSH", "PROP", rnd() < 0.1 ? "DRGD" : "REGD", dmy(), "", c.st, String(40000000 + i), "", "Y", c.abn, "", ""]);
  },
}, Math.floor(N * 0.4));
writeAll(path.join(out, "asic", "business_names_202611.csv"), {
  header: () => `\uFEFF${tsvRow(["REGISTER_NAME", "BN_NAME", "BN_STATUS", "BN_REG_DT", "BN_CANCEL_DT", "BN_STATE_NUM", "BN_STATE_OF_REG", "BN_ABN"])}`,
  row: (i) => tsvRow(["BUSINESS NAMES", `${pick(ADJ)} ${pick(NOUN)} ${i}`, rnd() < 0.3 ? "Deregistered" : "Registered", dmy(), "", "", "", acns[i % acns.length].abn]),
}, Math.floor(N * 0.3));

// G-NAF: one address per ABN on average, over four states
const SYL = "BAR BEL CAR DAL EDG FAIR GLEN HAM IRV JUN KEN LAN MAR NEW OAK PAR QUI RED SUM TAN UPP VAL WAR YAR".split(" ");
const place = () => `${pick(SYL)}${pick(SYL).toLowerCase()}${pick(["", "TON", "FIELD", "VALE", "BURN"])}`.toUpperCase();
const TYPES = ["STREET", "ROAD", "AVENUE", "DRIVE", "COURT", "PARADE", "LANE", "PLACE", "CLOSE", "CRESCENT", "TERRACE", "HIGHWAY"];
const psvRow = (cells) => `${cells.join("|")}\n`;
const gnafStates = STATE.slice(0, 4);
const addrTotal = Math.floor(N);
for (const [st, share, lead] of gnafStates) {
  const n = Math.floor((addrTotal * share) / gnafStates.reduce((s, x) => s + x[1], 0));
  const localities = Array.from({ length: 400 }, (_, i) => ({ pid: `${st}L${i + 1}`, name: `${place()} ${i}`, postcode: `${lead}${String(100 + i).padStart(3, "0")}` }));
  const streets = Array.from({ length: 4000 }, (_, i) => ({ pid: `${st}S${i + 1}`, name: `${place()} ${i}`, type: pick(TYPES), loc: localities[i % localities.length] }));
  const dir = path.join(out, "gnaf", "Standard");
  writeAll(path.join(dir, `${st}_STATE_psv.psv`), { header: () => psvRow(["STATE_PID", "DATE_CREATED", "DATE_RETIRED", "STATE_NAME", "STATE_ABBREVIATION"]), row: () => psvRow(["1", "2016-08-01", "", st, st]) }, 1);
  writeAll(path.join(dir, `${st}_LOCALITY_psv.psv`), { header: () => psvRow(["LOCALITY_PID", "DATE_CREATED", "DATE_RETIRED", "LOCALITY_NAME", "PRIMARY_POSTCODE", "LOCALITY_CLASS_CODE", "STATE_PID", "GNAF_LOCALITY_PID", "GNAF_RELIABILITY_CODE"]), row: (i) => psvRow([localities[i].pid, "2016-08-01", "", localities[i].name, localities[i].postcode, "G", "1", `G${localities[i].pid}`, "4"]) }, localities.length);
  writeAll(path.join(dir, `${st}_STREET_LOCALITY_psv.psv`), { header: () => psvRow(["STREET_LOCALITY_PID", "DATE_CREATED", "DATE_RETIRED", "STREET_CLASS_CODE", "STREET_NAME", "STREET_TYPE_CODE", "STREET_SUFFIX_CODE", "LOCALITY_PID", "GNAF_STREET_PID", "GNAF_STREET_CONFIDENCE", "GNAF_RELIABILITY_CODE"]), row: (i) => psvRow([streets[i].pid, "2016-08-01", "", "C", streets[i].name, streets[i].type, "", streets[i].loc.pid, `G${streets[i].pid}`, "1", "4"]) }, streets.length);
  const pidOf = (i) => `GA${st}${String(i).padStart(9, "0")}`;
  const where = [];
  writeAll(path.join(dir, `${st}_ADDRESS_DETAIL_psv.psv`), {
    header: () => psvRow(["ADDRESS_DETAIL_PID", "DATE_CREATED", "DATE_LAST_MODIFIED", "DATE_RETIRED", "BUILDING_NAME", "LOT_NUMBER_PREFIX", "LOT_NUMBER", "LOT_NUMBER_SUFFIX", "FLAT_TYPE_CODE", "FLAT_NUMBER_PREFIX", "FLAT_NUMBER", "FLAT_NUMBER_SUFFIX", "LEVEL_TYPE_CODE", "LEVEL_NUMBER_PREFIX", "LEVEL_NUMBER", "LEVEL_NUMBER_SUFFIX", "NUMBER_FIRST_PREFIX", "NUMBER_FIRST", "NUMBER_FIRST_SUFFIX", "NUMBER_LAST_PREFIX", "NUMBER_LAST", "NUMBER_LAST_SUFFIX", "STREET_LOCALITY_PID", "LOCATION_DESCRIPTION", "LOCALITY_PID", "ALIAS_PRINCIPAL", "POSTCODE", "PRIVATE_STREET", "LEGAL_PARCEL_ID", "CONFIDENCE", "ADDRESS_SITE_PID", "LEVEL_GEOCODED_CODE", "PROPERTY_PID", "GNAF_PROPERTY_PID", "PRIMARY_SECONDARY"]),
    row: (i) => {
      const s = streets[Math.floor(rnd() * streets.length)];
      const num = 1 + Math.floor(rnd() * 300);
      const flat = rnd() < 0.2 ? 1 + Math.floor(rnd() * 20) : "";
      where.push([s, num, flat]);
      if (queries.address.length < 200 && i % Math.max(1, Math.floor(n / 50)) === 0) queries.address.push(`${flat ? `${flat}/` : ""}${num} ${s.name} ${s.type.slice(0, 2) === "ST" ? "ST" : s.type} ${s.loc.name}`);
      return psvRow([pidOf(i), "2016-08-01", "2026-08-01", "", "", "", "", "", flat ? "UNIT" : "", "", flat, "", "", "", "", "", "", num, "", "", "", "", s.pid, "", s.loc.pid, "P", s.loc.postcode, "", "", "2", `${pidOf(i)}S`, "7", `${pidOf(i)}P`, `${pidOf(i)}G`, ""]);
    },
  }, n);
  writeAll(path.join(dir, `${st}_ADDRESS_DEFAULT_GEOCODE_psv.psv`), { header: () => psvRow(["ADDRESS_DEFAULT_GEOCODE_PID", "DATE_CREATED", "DATE_RETIRED", "ADDRESS_DETAIL_PID", "GEOCODE_TYPE_CODE", "LONGITUDE", "LATITUDE"]), row: (i) => psvRow([`${st}G${i}`, "2016-08-01", "", pidOf(i), "PC", (112 + rnd() * 41).toFixed(8), (-10 - rnd() * 34).toFixed(8)]) }, n);
}

// ABS: 1,228 dataflows (the real count) and five observation files
const flows = Array.from({ length: 1228 }, (_, i) => ({ id: `SCALE_FLOW_${i}`, version: "1.0.0", agencyID: "ABS", name: `Scale: ${pick(NOUN).toLowerCase()} ${pick(ADJ).toLowerCase()} statistics ${i}` }));
fs.mkdirSync(path.join(out, "abs", "data"), { recursive: true });
fs.writeFileSync(path.join(out, "abs", "dataflows.json"), JSON.stringify({ data: { dataflows: flows } }));
for (let f = 0; f < 5; f++) {
  const rows = ["DATAFLOW,MEASURE,REGION,FREQ,TIME_PERIOD,OBS_VALUE,UNIT_MEASURE,OBS_STATUS,DECIMALS"];
  for (let r = 0; r < Number(args["abs-rows"]); r++) rows.push(`ABS:SCALE_FLOW_${f}(1.0.0),1,${r % 2500},A,${1990 + (r % 36)},${Math.floor(rnd() * 1e6)},NUM,,0`);
  fs.writeFileSync(path.join(out, "abs", "data", `SCALE_FLOW_${f}.csv`), `${rows.join("\n")}\n`);
  if (queries.stats.length < 20) queries.stats.push([`SCALE_FLOW_${f}`, "--filter", `REGION=${(f * 37) % 2500}`, "--last", "3"]);
}
while (queries.stats.length < 20) queries.stats.push([`SCALE_FLOW_${queries.stats.length % 5}`, "--filter", `REGION=${(queries.stats.length * 101) % 2500}`, "--from", "2000", "--to", "2010"]);

// ASGS: 2,400 rectangles over the continent
const feats = [];
for (let r = 0; r < 40; r++) for (let c = 0; c < 60; c++) {
  const x0 = 113 + c * 0.66;
  const y0 = -44 + r * 0.85;
  feats.push({ type: "Feature", properties: { SA2_CODE21: String(100000000 + r * 60 + c), SA2_NAME21: `Scale Area ${r}-${c}`, STE_NAME21: "Scale" }, geometry: { type: "Polygon", coordinates: [[[x0, y0], [x0 + 0.66, y0], [x0 + 0.66, y0 + 0.85], [x0, y0 + 0.85], [x0, y0]]] } });
}
fs.mkdirSync(path.join(out, "asgs"), { recursive: true });
fs.writeFileSync(path.join(out, "asgs", "SA2_SCALE.geojson"), JSON.stringify({ type: "FeatureCollection", features: feats }));
for (let i = 0; i < 200; i++) queries.area.push([(-43.5 + rnd() * 33.5).toFixed(5), (113.3 + rnd() * 39.5).toFixed(5)]);

fs.writeFileSync(path.join(out, "manifest.json"), JSON.stringify({ format: 1, sources: {
  abr: { asOf: "2026-11-05", files: ["abn/public_split_1_10.xml"], licence: "CC BY 3.0 AU", publisher: "Australian Business Register" },
  asic: { asOf: "2026-11-03", files: ["asic/company_202611.csv"], licence: "CC BY 3.0 AU", publisher: "ASIC" },
  asicBn: { asOf: "2026-11-04", files: ["asic/business_names_202611.csv"], licence: "CC BY 3.0 AU", publisher: "ASIC" },
  gnaf: { asOf: "2026-08-17", files: ["gnaf/Standard"], licence: "CC BY 4.0 (Open G-NAF EULA)", publisher: "Geoscape Australia" },
  abs: { asOf: "2026-11-10", files: ["abs/dataflows.json", ...Array.from({ length: 5 }, (_, f) => `abs/data/SCALE_FLOW_${f}.csv`)], licence: "CC BY 4.0", publisher: "Australian Bureau of Statistics" },
  asgs: { asOf: "2026-07-21", files: ["asgs/SA2_SCALE.geojson"], licence: "CC BY 4.0", publisher: "Australian Bureau of Statistics" },
} }, null, 2));
queries.name = queries.name.slice(0, 200);
fs.writeFileSync(path.join(out, "queries.json"), JSON.stringify(queries));
console.log(`scale data written to ${out}: ${N} ABNs, ${acns.length} ACNs, ${queries.abn.length} abn / ${queries.name.length} name / ${queries.address.length} address / ${queries.stats.length} stats / ${queries.area.length} area queries`);
