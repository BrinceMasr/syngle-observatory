// Writes the gov-lookup fixtures: SYNTHETIC records in the exact file shapes the official sources publish, in two data
// versions (v1, then v2 a week later with known changes), and golden.json, the answers every build of `syngle-map gov` must give.
//
//   node evals/gov-data/fixtures/make.mjs        (deterministic: the files in this folder are its output; rerun, git diff is empty)
//
// Shapes (checked 2026-11-14 against the published files, not against remembered layouts):
//   ABN Bulk Extract   XML, root <Transfer>, per the official bulkextract.xsd (data.gov.au, ABN Bulk Extract, CC BY 3.0 AU)
//   ASIC company       TAB-separated, UTF-8 BOM, CRLF, dd/mm/yyyy, header "Company Name<TAB>ACN<TAB>Type..." (CC BY 3.0 AU)
//   ASIC business names TAB-separated, same envelope, header REGISTER_NAME, BN_NAME, BN_STATUS, ... (CC BY 3.0 AU)
//   G-NAF              pipe-separated tables <STATE>_<TABLE>_psv.psv (Open G-NAF, CC BY 4.0 + EULA)
//   ABS Data API       SDMX-CSV (`Accept: application/vnd.sdmx.data+csv`) and the dataflow list as SDMX-JSON (CC BY 4.0)
//   ASGS               one GeoJSON layer of three rectangles, the test input for point lookup (the official files are Shapefile
//                      and GeoPackage: the build reads those too, proven on the real file by its ticket, not here)
// Names are invented. ABN and ACN numbers pass the official check digits and may coincide with a real registration by chance.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));

// ---- check digits -----------------------------------------------------------------------------------------------
const ABN_W = [10, 1, 3, 5, 7, 9, 11, 13, 15, 17, 19];
export const abnValid = (abn) => {
  const d = String(abn).replace(/\D/g, "");
  if (d.length !== 11) return false;
  const digits = [...d].map(Number);
  digits[0] -= 1;
  return digits.reduce((sum, n, i) => sum + n * ABN_W[i], 0) % 89 === 0;
};
const abnFrom = (nine) => {
  for (let p = 10; p <= 99; p++) if (abnValid(`${p}${nine}`)) return `${p}${nine}`;
  throw new Error(`no ABN for ${nine}`);
};
const ACN_W = [8, 7, 6, 5, 4, 3, 2, 1];
const acnFrom = (eight) => {
  const sum = [...eight].map(Number).reduce((s, n, i) => s + n * ACN_W[i], 0);
  return `${eight}${(10 - (sum % 10)) % 10}`;
};
const group = (abn) => `${abn.slice(0, 2)} ${abn.slice(2, 5)} ${abn.slice(5, 8)} ${abn.slice(8)}`;

// ---- the entities -----------------------------------------------------------------------------------------------
// `v2` overrides what changes a week later. An entity with `person` is a sole trader (LegalEntity).
const entities = [
  { key: "harbour", name: "HARBOUR ROOFING PTY LTD", nine: "100200301", acn8: "10020030", type: ["PRV", "Australian Private Company"], state: "NSW", postcode: "2000", gst: "2012-07-01", trading: ["HARBOUR ROOFING"], bn: [] },
  { key: "harboursup", name: "HARBOUR ROOFING SUPPLIES PTY LTD", nine: "100200402", acn8: "10020040", type: ["PRV", "Australian Private Company"], state: "NSW", postcode: "2150", gst: "2015-03-01", trading: [], bn: [] },
  { key: "harbourco", name: "HARBOUR & CO PTY LTD", nine: "100200503", acn8: "10020050", type: ["PRV", "Australian Private Company"], state: "VIC", postcode: "3000", gst: "2018-01-01", trading: [], bn: [] },
  { key: "bluegum", name: "BLUE GUM CONSULTING PTY LTD", nine: "100200604", acn8: "10020060", type: ["PRV", "Australian Private Company"], state: "VIC", postcode: "3000", gst: "2019-07-01", trading: ["BLUE GUM ADVISORY"], bn: [], v2: { name: "BLUE GUM ADVISORY GROUP PTY LTD", formerName: "BLUE GUM CONSULTING PTY LTD" } },
  { key: "redkangaroo", name: "RED KANGAROO LOGISTICS PTY LTD", nine: "100200705", acn8: "10020070", type: ["PRV", "Australian Private Company"], state: "QLD", postcode: "4000", gst: "2010-07-01", trading: [], bn: [] },
  { key: "southerncross", name: "SOUTHERN CROSS DIGITAL PTY LTD", nine: "100200806", acn8: "10020080", type: ["PRV", "Australian Private Company"], state: "WA", postcode: "6000", gst: "2016-07-01", trading: [], bn: [], cancelled: "2024-05-15" },
  { key: "tasmanian", name: "TASMANIAN TIMBER TRUST", nine: "100200907", acn8: null, type: ["TRT", "Other Trust"], state: "TAS", postcode: "7000", gst: "2014-07-01", trading: ["TASSIE TIMBER"], bn: [] },
  { key: "ntsurvey", name: "NORTHERN TERRITORY SURVEY SERVICES PTY LTD", nine: "100201008", acn8: "10020100", type: ["PRV", "Australian Private Company"], state: "NT", postcode: "0800", gst: "2017-07-01", trading: [], bn: [] },
  { key: "capital", name: "CAPITAL FIXTURE HOLDINGS PTY LTD", nine: "100201109", acn8: "10020110", type: ["PRV", "Australian Private Company"], state: "ACT", postcode: "2600", gst: "2013-07-01", trading: [], bn: [], v2: { cancelled: "2026-11-02" } },
  { key: "adelaide", name: "ADELAIDE FIXTURE BAKERY PTY LTD", nine: "100201200", acn8: "10020120", type: ["PRV", "Australian Private Company"], state: "SA", postcode: "5000", gst: "2011-07-01", trading: [], bn: ["FIXTURE LOAF"] },
  { key: "alex", person: { given: ["ALEX", "J"], family: "TESTPERSON" }, nine: "100201301", acn8: null, type: ["IND", "Individual/Sole Trader"], state: "NSW", postcode: "2000", gst: null, trading: [], bn: ["TESTPERSON GARDENING"] },
  { key: "newco", name: "NEW HORIZON FIXTURES PTY LTD", nine: "100201402", acn8: "10020140", type: ["PRV", "Australian Private Company"], state: "NSW", postcode: "2000", gst: "2026-11-03", trading: [], bn: [], onlyV2: true },
];
for (const e of entities) {
  e.abn = abnFrom(e.nine);
  e.acn = e.acn8 ? acnFrom(e.acn8) : null;
}
const by = Object.fromEntries(entities.map((e) => [e.key, e]));

const ymd = (iso) => iso.replaceAll("-", "");
const dmy = (iso) => iso.split("-").reverse().join("/");
const esc = (text) => text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");

const VERSIONS = {
  v1: { abr: "2026-11-05", asic: "2026-11-03", asicBn: "2026-11-04", gnaf: "2026-08-17", abs: "2026-11-10", asgs: "2026-07-21" },
  v2: { abr: "2026-11-12", asic: "2026-11-10", asicBn: "2026-11-11", gnaf: "2026-08-17", abs: "2026-11-10", asgs: "2026-07-21" },
};

function entityIn(e, version) {
  const o = version === "v2" && e.v2 ? e.v2 : {};
  return { ...e, ...o, cancelled: o.cancelled ?? e.cancelled };
}
const present = (e, version) => !(e.onlyV2 && version === "v1");

// ---- ABN Bulk Extract XML ---------------------------------------------------------------------------------------
function abnXml(version) {
  const rows = [];
  for (const base of entities.filter((e) => present(e, version))) {
    const e = entityIn(base, version);
    const status = e.cancelled ? "CAN" : "ACT";
    const from = e.cancelled ?? (e.gst ?? "2000-01-01");
    const lines = [`  <ABR recordLastUpdatedDate="${ymd(VERSIONS[version].abr)}" replaced="N">`, `    <ABN status="${status}" ABNStatusFromDate="${ymd(from)}">${e.abn}</ABN>`, `    <EntityType>`, `      <EntityTypeInd>${e.type[0]}</EntityTypeInd>`, `      <EntityTypeText>${e.type[1]}</EntityTypeText>`, `    </EntityType>`];
    if (e.person) {
      lines.push(`    <LegalEntity>`, `      <IndividualName type="LGL">`);
      for (const g of e.person.given) lines.push(`        <GivenName>${g}</GivenName>`);
      lines.push(`        <FamilyName>${e.person.family}</FamilyName>`, `      </IndividualName>`, `      <BusinessAddress><AddressDetails><State>${e.state}</State><Postcode>${e.postcode}</Postcode></AddressDetails></BusinessAddress>`, `    </LegalEntity>`);
    } else {
      lines.push(`    <MainEntity>`, `      <NonIndividualName type="MN"><NonIndividualNameText>${esc(e.name)}</NonIndividualNameText></NonIndividualName>`, `      <BusinessAddress><AddressDetails><State>${e.state}</State><Postcode>${e.postcode}</Postcode></AddressDetails></BusinessAddress>`, `    </MainEntity>`);
    }
    if (e.acn) lines.push(`    <ASICNumber ASICNumberType="undetermined">${e.acn}</ASICNumber>`);
    if (e.gst) lines.push(`    <GST status="${e.cancelled ? "CAN" : "ACT"}" GSTStatusFromDate="${ymd(e.cancelled ?? e.gst)}"/>`);
    for (const n of e.trading) lines.push(`    <OtherEntity><NonIndividualName type="TRD"><NonIndividualNameText>${esc(n)}</NonIndividualNameText></NonIndividualName></OtherEntity>`);
    for (const n of e.bn) lines.push(`    <OtherEntity><NonIndividualName type="BN"><NonIndividualNameText>${esc(n)}</NonIndividualNameText></NonIndividualName></OtherEntity>`);
    lines.push(`  </ABR>`);
    rows.push(lines.join("\n"));
  }
  return `<?xml version="1.0" encoding="UTF-8"?>\n<Transfer error="">\n  <TransferInfo>\n    <FileSequenceNumber>1</FileSequenceNumber>\n    <RecordCount>${rows.length}</RecordCount>\n    <ExtractTime>${VERSIONS[version].abr}T21:34:30</ExtractTime>\n  </TransferInfo>\n${rows.join("\n")}\n</Transfer>\n`;
}

// ---- ASIC company and business names (TSV, BOM, CRLF) -----------------------------------------------------------
const tsv = (rows) => `\uFEFF${rows.map((r) => r.join("\t")).join("\r\n")}\r\n`;
function asicCompany(version) {
  const rows = [["Company Name", "ACN", "Type", "Class", "Sub Class", "Status", "Date of Registration", "Date of Deregistration", "Previous State of Registration", "State Registration number", "Modified since last report", "Current Name Indicator", "ABN", "Current Name", "Current Name Start Date"]];
  for (const base of entities.filter((e) => e.acn && present(e, version))) {
    const e = entityIn(base, version);
    const status = e.cancelled ? "DRGD" : "REGD";
    const dereg = e.cancelled ? dmy(e.cancelled) : "";
    const stateNumber = String(40000000 + (Number(e.acn.slice(0, 7)) % 99999));
    const row = (name, indicator, current, currentStart) => [name, e.acn, "APTY", "LMSH", "PROP", status, "08/01/1990", dereg, "NSW", stateNumber, "", indicator, e.abn, current, currentStart];
    // As published: a renamed company has one row per name; the current name's row has the indicator "Y", a former name's
    // row names the current one and when it began.
    if (e.formerName) rows.push(row(e.formerName, "", e.name, dmy("2026-11-09")));
    rows.push(row(e.name, "Y", "", ""));
  }
  return tsv(rows);
}
function asicBusinessNames(version) {
  const rows = [["REGISTER_NAME", "BN_NAME", "BN_STATUS", "BN_REG_DT", "BN_CANCEL_DT", "BN_STATE_NUM", "BN_STATE_OF_REG", "BN_ABN"]];
  for (const base of entities.filter((e) => present(e, version))) {
    const e = entityIn(base, version);
    for (const n of e.bn) rows.push(["BUSINESS NAMES", n, e.cancelled ? "Deregistered" : "Registered", "15/11/2019", e.cancelled ? dmy(e.cancelled) : "", "", "", e.abn]);
  }
  rows.push(["BUSINESS NAMES", "ORPHAN FIXTURE TRADING", "Registered", "01/02/2020", "", "", "", ""]);
  return tsv(rows);
}

// ---- G-NAF (NSW and VIC subset, real table and column names) ----------------------------------------------------
const addresses = [
  { pid: "GANSW000000001", state: "NSW", flat: ["UNIT", "3"], num: "12", street: "SMITH", type: "STREET", locality: "SYDNEY", postcode: "2000", lon: 151.2093, lat: -33.8688 },
  { pid: "GANSW000000002", state: "NSW", num: "12", street: "SMITH", type: "STREET", locality: "SYDNEY", postcode: "2000", lon: 151.2094, lat: -33.8689 },
  { pid: "GANSW000000003", state: "NSW", num: "14", street: "SMITH", type: "STREET", locality: "SYDNEY", postcode: "2000", lon: 151.2095, lat: -33.869 },
  { pid: "GANSW000000004", state: "NSW", num: "1", street: "MACQUARIE", type: "ROAD", locality: "PARRAMATTA", postcode: "2150", lon: 151.0011, lat: -33.8148 },
  { pid: "GANSW000000005", state: "NSW", num: "200", street: "GEORGE", type: "STREET", locality: "SYDNEY", postcode: "2000", lon: 151.2073, lat: -33.8651 },
  { pid: "GANSW000000006", state: "NSW", num: "7", street: "WATTLE", type: "AVENUE", locality: "NEWCASTLE", postcode: "2300", lon: 151.7817, lat: -32.9267 },
  { pid: "GAVIC000000001", state: "VIC", num: "12", street: "SMITH", type: "STREET", locality: "MELBOURNE", postcode: "3000", lon: 144.9631, lat: -37.8136 },
  { pid: "GAVIC000000002", state: "VIC", num: "88", street: "COLLINS", type: "STREET", locality: "MELBOURNE", postcode: "3000", lon: 144.9654, lat: -37.8142 },
  { pid: "GAVIC000000003", state: "VIC", flat: ["FLAT", "2"], num: "5", street: "BEACH", type: "ROAD", locality: "ST KILDA", postcode: "3182", lon: 144.9807, lat: -37.8676 },
  { pid: "GAVIC000000004", state: "VIC", num: "30", street: "GRAMPIANS", type: "DRIVE", locality: "HALLS GAP", postcode: "3381", lon: 142.5193, lat: -37.1366 },
];
const psv = (rows) => `${rows.map((r) => r.join("|")).join("\n")}\n`;
function gnafFiles() {
  const out = {};
  for (const state of ["NSW", "VIC"]) {
    const list = addresses.filter((a) => a.state === state);
    const localities = [...new Set(list.map((a) => a.locality))];
    const streets = [...new Set(list.map((a) => `${a.street}|${a.type}|${a.locality}`))];
    const lpid = (n) => `${state}L${String(localities.indexOf(n) + 1).padStart(4, "0")}`;
    out[`${state}_STATE_psv.psv`] = psv([["STATE_PID", "DATE_CREATED", "DATE_RETIRED", "STATE_NAME", "STATE_ABBREVIATION"], [state === "NSW" ? "1" : "2", "2016-08-01", "", state === "NSW" ? "NEW SOUTH WALES" : "VICTORIA", state]]);
    out[`${state}_LOCALITY_psv.psv`] = psv([["LOCALITY_PID", "DATE_CREATED", "DATE_RETIRED", "LOCALITY_NAME", "PRIMARY_POSTCODE", "LOCALITY_CLASS_CODE", "STATE_PID", "GNAF_LOCALITY_PID", "GNAF_RELIABILITY_CODE"], ...localities.map((n) => [lpid(n), "2016-08-01", "", n, list.find((a) => a.locality === n).postcode, "G", state === "NSW" ? "1" : "2", `G${lpid(n)}`, "4"])]);
    out[`${state}_STREET_LOCALITY_psv.psv`] = psv([["STREET_LOCALITY_PID", "DATE_CREATED", "DATE_RETIRED", "STREET_CLASS_CODE", "STREET_NAME", "STREET_TYPE_CODE", "STREET_SUFFIX_CODE", "LOCALITY_PID", "GNAF_STREET_PID", "GNAF_STREET_CONFIDENCE", "GNAF_RELIABILITY_CODE"], ...streets.map((s, i) => { const [name, type, loc] = s.split("|"); return [`${state}S${String(i + 1).padStart(4, "0")}`, "2016-08-01", "", "C", name, type, "", lpid(loc), `G${state}S${i + 1}`, "1", "4"]; })]);
    const spid = (a) => `${state}S${String(streets.indexOf(`${a.street}|${a.type}|${a.locality}`) + 1).padStart(4, "0")}`;
    out[`${state}_ADDRESS_DETAIL_psv.psv`] = psv([["ADDRESS_DETAIL_PID", "DATE_CREATED", "DATE_LAST_MODIFIED", "DATE_RETIRED", "BUILDING_NAME", "LOT_NUMBER_PREFIX", "LOT_NUMBER", "LOT_NUMBER_SUFFIX", "FLAT_TYPE_CODE", "FLAT_NUMBER_PREFIX", "FLAT_NUMBER", "FLAT_NUMBER_SUFFIX", "LEVEL_TYPE_CODE", "LEVEL_NUMBER_PREFIX", "LEVEL_NUMBER", "LEVEL_NUMBER_SUFFIX", "NUMBER_FIRST_PREFIX", "NUMBER_FIRST", "NUMBER_FIRST_SUFFIX", "NUMBER_LAST_PREFIX", "NUMBER_LAST", "NUMBER_LAST_SUFFIX", "STREET_LOCALITY_PID", "LOCATION_DESCRIPTION", "LOCALITY_PID", "ALIAS_PRINCIPAL", "POSTCODE", "PRIVATE_STREET", "LEGAL_PARCEL_ID", "CONFIDENCE", "ADDRESS_SITE_PID", "LEVEL_GEOCODED_CODE", "PROPERTY_PID", "GNAF_PROPERTY_PID", "PRIMARY_SECONDARY"], ...list.map((a) => [a.pid, "2016-08-01", "2026-08-01", "", "", "", "", "", a.flat?.[0] ?? "", "", a.flat?.[1] ?? "", "", "", "", "", "", "", a.num, "", "", "", "", spid(a), "", lpid(a.locality), "P", a.postcode, "", "", "2", `${a.pid}S`, "7", `${a.pid}P`, `${a.pid}G`, ""])]);
    out[`${state}_ADDRESS_DEFAULT_GEOCODE_psv.psv`] = psv([["ADDRESS_DEFAULT_GEOCODE_PID", "DATE_CREATED", "DATE_RETIRED", "ADDRESS_DETAIL_PID", "GEOCODE_TYPE_CODE", "LONGITUDE", "LATITUDE"], ...list.map((a, i) => [`${state}G${i + 1}`, "2016-08-01", "", a.pid, "PC", a.lon.toFixed(8), a.lat.toFixed(8)])]);
  }
  return out;
}

// ---- ABS (SDMX-CSV observations, SDMX-JSON dataflow list) and ASGS ----------------------------------------------
const STATES = [["1", "New South Wales", 8_400_000], ["2", "Victoria", 6_800_000], ["3", "Queensland", 5_500_000], ["5", "Western Australia", 2_900_000]];
function absData(version) {
  const rows = ["DATAFLOW,MEASURE,REGION,FREQ,TIME_PERIOD,OBS_VALUE,UNIT_MEASURE,OBS_STATUS,DECIMALS"];
  const years = version === "v1" ? ["2023", "2024"] : ["2023", "2024", "2025"];
  for (const [code, , pop] of STATES) years.forEach((y, i) => rows.push(`ABS:FIXTURE_POP(1.0.0),1,${code},A,${y},${pop + i * 100000},NUM,,0`));
  return `${rows.join("\n")}\n`;
}
const absFlows = () => JSON.stringify({ meta: { schema: "https://raw.githubusercontent.com/sdmx-twg/sdmx-json/develop/structure-message/tools/schemas/1.0/sdmx-json-structure-schema.json" }, data: { dataflows: [{ id: "FIXTURE_POP", version: "1.0.0", agencyID: "ABS", name: "Fixture: estimated resident population by state, annual" }, { id: "FIXTURE_CPI", version: "2.0.0", agencyID: "ABS", name: "Fixture: consumer price index, monthly" }, { id: "FIXTURE_LABOUR", version: "1.1.0", agencyID: "ABS", name: "Fixture: labour force, monthly" }] } }, null, 1) + "\n";
const asgsGeojson = () =>
  JSON.stringify({ type: "FeatureCollection", name: "SA2_FIXTURE", features: [
    { type: "Feature", properties: { SA2_CODE21: "100000001", SA2_NAME21: "Fixture Sydney", STE_NAME21: "New South Wales" }, geometry: { type: "Polygon", coordinates: [[[151.0, -34.0], [151.4, -34.0], [151.4, -33.7], [151.0, -33.7], [151.0, -34.0]]] } },
    { type: "Feature", properties: { SA2_CODE21: "200000001", SA2_NAME21: "Fixture Melbourne", STE_NAME21: "Victoria" }, geometry: { type: "Polygon", coordinates: [[[144.8, -37.95], [145.1, -37.95], [145.1, -37.7], [144.8, -37.7], [144.8, -37.95]]] } },
    { type: "Feature", properties: { SA2_CODE21: "300000001", SA2_NAME21: "Fixture Outback", STE_NAME21: "Northern Territory" }, geometry: { type: "Polygon", coordinates: [[[130.0, -25.0], [134.0, -25.0], [134.0, -22.0], [130.0, -22.0], [130.0, -25.0]]] } },
  ] }, null, 1) + "\n";

// ---- write ------------------------------------------------------------------------------------------------------
function write(file, text) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
}
for (const version of ["v1", "v2"]) {
  const v = VERSIONS[version];
  const root = path.join(here, version);
  write(path.join(root, "abn", "public_split_1_10.xml"), abnXml(version));
  write(path.join(root, "asic", `company_${v.asic.slice(0, 4)}${v.asic.slice(5, 7)}.csv`), asicCompany(version));
  write(path.join(root, "asic", `business_names_${v.asicBn.slice(0, 4)}${v.asicBn.slice(5, 7)}.csv`), asicBusinessNames(version));
  for (const [name, text] of Object.entries(gnafFiles())) write(path.join(root, "gnaf", "Standard", name), text);
  write(path.join(root, "abs", "dataflows.json"), absFlows());
  write(path.join(root, "abs", "data", "FIXTURE_POP.csv"), absData(version));
  write(path.join(root, "asgs", "SA2_FIXTURE.geojson"), asgsGeojson());
  write(path.join(root, "manifest.json"), JSON.stringify({ format: 1, sources: {
    abr: { asOf: v.abr, files: ["abn/public_split_1_10.xml"], licence: "CC BY 3.0 AU", publisher: "Australian Business Register" },
    asic: { asOf: v.asic, files: [`asic/company_${v.asic.slice(0, 4)}${v.asic.slice(5, 7)}.csv`], licence: "CC BY 3.0 AU", publisher: "ASIC" },
    asicBn: { asOf: v.asicBn, files: [`asic/business_names_${v.asicBn.slice(0, 4)}${v.asicBn.slice(5, 7)}.csv`], licence: "CC BY 3.0 AU", publisher: "ASIC" },
    gnaf: { asOf: v.gnaf, files: ["gnaf/Standard"], licence: "CC BY 4.0 (Open G-NAF EULA)", publisher: "Geoscape Australia" },
    abs: { asOf: v.abs, files: ["abs/dataflows.json", "abs/data/FIXTURE_POP.csv"], licence: "CC BY 4.0", publisher: "Australian Bureau of Statistics" },
    asgs: { asOf: v.asgs, files: ["asgs/SA2_FIXTURE.geojson"], licence: "CC BY 4.0", publisher: "Australian Bureau of Statistics" },
  } }, null, 2) + "\n");
}

// ---- golden: ground truth from the table above -------------------------------------------------------------------
// A case is { id, args, expect } where each expect key is a dotted path into the answer and its value must match exactly.
// `kind` groups the pass rates. `v` is the data version the case runs on. Paths use array indexes ("results.0.abn").
const G = [];
const add = (kind, id, args, expect, v = "v1") => G.push({ kind, id, v, args, expect });
const H = by.harbour;
const ABSENT_ABN = abnFrom("100999991");
add("abn", "abn exact, spaced", ["abn", group(H.abn)], { ok: true, count: 1, "results.0.abn": H.abn, "results.0.name": H.name, "results.0.status": "Active", "results.0.state": "NSW", "results.0.postcode": "2000", "results.0.acn": H.acn, "results.0.gst": true, "results.0.entityType": "Australian Private Company" });
add("abn", "abn exact, digits", ["abn", H.abn], { ok: true, count: 1, "results.0.abn": H.abn });
add("abn", "abn by acn", ["abn", H.acn], { ok: true, count: 1, "results.0.abn": H.abn, "results.0.acn": H.acn });
add("abn", "abn lists trading and business names", ["abn", by.adelaide.abn], { ok: true, "results.0.names.0.type": "BN", "results.0.names.0.text": "FIXTURE LOAF" });
add("abn", "abn trading name", ["abn", by.tasmanian.abn], { ok: true, "results.0.names.0.type": "TRD", "results.0.names.0.text": "TASSIE TIMBER", "results.0.acn": null });
add("abn", "abn sole trader", ["abn", by.alex.abn], { ok: true, "results.0.entityType": "Individual/Sole Trader", "results.0.name": "TESTPERSON, ALEX J", "results.0.gst": false });
add("abn", "abn cancelled shows status and date", ["abn", by.southerncross.abn], { ok: true, "results.0.status": "Cancelled", "results.0.statusFrom": "2024-05-15" });
add("abn", "abn not found", ["abn", ABSENT_ABN], { ok: true, count: 0 });
add("abn", "abn bad check digit", ["abn", "12345678901"], { ok: false, "error.code": "invalid-abn" });
add("abn", "abn not digits", ["abn", "hello"], { ok: false, "error.code": "invalid-abn" });
add("name", "name exact", ["name", "HARBOUR ROOFING PTY LTD"], { ok: true, "results.0.name": H.name, "results.0.kind": "abn", "results.0.abn": H.abn });
add("name", "name case and punctuation insensitive", ["name", "harbour  roofing, pty ltd"], { "results.0.abn": H.abn });
add("name", "name prefix lists in rank order", ["name", "harbour"], { count: 3, "results.0.abn": H.abn, "results.1.abn": by.harbourco.abn, "results.2.abn": by.harboursup.abn });
add("name", "name all tokens any order", ["name", "kangaroo red"], { count: 1, "results.0.abn": by.redkangaroo.abn });
add("name", "name finds a trading name", ["name", "tassie timber"], { "results.0.abn": by.tasmanian.abn, "results.0.matched": "TASSIE TIMBER", "results.0.matchedType": "TRD" });
add("name", "name finds a business name", ["name", "fixture loaf"], { "results.0.abn": by.adelaide.abn, "results.0.matchedType": "BN" });
add("name", "name state filter", ["name", "harbour", "--state", "VIC"], { count: 1, "results.0.abn": by.harbourco.abn });
add("name", "name limit", ["name", "harbour", "--limit", "2"], { count: 2 });
add("name", "name active only", ["name", "southern cross digital", "--active"], { count: 0 });
add("name", "name includes cancelled by default", ["name", "southern cross digital"], { count: 1, "results.0.status": "Cancelled" });
add("name", "name ampersand reads as and", ["name", "harbour and co"], { "results.0.abn": by.harbourco.abn });
add("name", "name no match", ["name", "zzzzzz qqqq"], { ok: true, count: 0 });
add("name", "name empty", ["name", ""], { ok: false, "error.code": "empty-query" });
add("company", "company by acn", ["company", H.acn], { ok: true, count: 1, "results.0.acn": H.acn, "results.0.name": H.name, "results.0.status": "Registered", "results.0.abn": H.abn });
add("company", "company renamed shows current and former", ["company", by.bluegum.acn], { ok: true, "results.0.name": "BLUE GUM CONSULTING PTY LTD", "results.0.formerNames": [] }, "v1");
add("company", "company renamed in v2", ["company", by.bluegum.acn], { ok: true, "results.0.name": "BLUE GUM ADVISORY GROUP PTY LTD", "results.0.formerNames.0": "BLUE GUM CONSULTING PTY LTD" }, "v2");
add("company", "company deregistered", ["company", by.southerncross.acn], { ok: true, "results.0.status": "Deregistered", "results.0.deregistered": "2024-05-15" });
add("company", "business name by abn", ["business-names", by.adelaide.abn], { ok: true, count: 1, "results.0.name": "FIXTURE LOAF", "results.0.status": "Registered", "results.0.abn": by.adelaide.abn });
add("address", "address exact with unit", ["address", "unit 3/12 smith st, sydney nsw 2000"], { ok: true, "results.0.addressId": "GANSW000000001", "results.0.lat": -33.8688, "results.0.lon": 151.2093, "results.0.locality": "SYDNEY", "results.0.state": "NSW", "results.0.postcode": "2000" });
add("address", "address abbreviations and no postcode", ["address", "12 Smith St Sydney"], { "results.0.addressId": "GANSW000000002", count: 2 });
add("address", "address ranks the closer match first", ["address", "12 Smith Street Sydney NSW"], { "results.0.addressId": "GANSW000000002", "results.1.addressId": "GANSW000000001" });
add("address", "address same street other state", ["address", "12 smith street melbourne vic 3000"], { count: 1, "results.0.addressId": "GAVIC000000001" });
add("address", "address flat prefix form", ["address", "2/5 beach rd st kilda"], { "results.0.addressId": "GAVIC000000003" });
add("address", "address by id", ["address", "--id", "GAVIC000000004"], { count: 1, "results.0.address": "30 GRAMPIANS DRIVE, HALLS GAP VIC 3381" });
add("address", "address not found", ["address", "999 nowhere lane atlantis"], { ok: true, count: 0 });
add("address", "address limit", ["address", "smith street", "--limit", "3"], { count: 3 });
add("stats", "stats find", ["stats", "find", "population"], { ok: true, count: 1, "results.0.id": "FIXTURE_POP", "results.0.version": "1.0.0" });
add("stats", "stats get one series", ["stats", "get", "FIXTURE_POP", "--filter", "REGION=2", "--from", "2023", "--to", "2024"], { ok: true, count: 2, "results.0.period": "2023", "results.0.value": 6800000, "results.1.period": "2024", "results.1.value": 6900000 });
add("stats", "stats get latest", ["stats", "get", "FIXTURE_POP", "--filter", "REGION=1", "--last", "1"], { count: 1, "results.0.period": "2024", "results.0.value": 8500000 });
add("stats", "stats get latest in v2", ["stats", "get", "FIXTURE_POP", "--filter", "REGION=1", "--last", "1"], { count: 1, "results.0.period": "2025", "results.0.value": 8600000 }, "v2");
add("stats", "stats unknown dataflow", ["stats", "get", "NOPE"], { ok: false, "error.code": "unknown-dataflow" });
add("area", "area point in a polygon", ["area", "--lat", "-33.87", "--lon", "151.2"], { ok: true, count: 1, "results.0.code": "100000001", "results.0.name": "Fixture Sydney", "results.0.layer": "SA2" });
add("area", "area point in no polygon", ["area", "--lat", "-10", "--lon", "100"], { ok: true, count: 0 });
add("area", "area bad coordinates", ["area", "--lat", "95", "--lon", "0"], { ok: false, "error.code": "invalid-coordinates" });

// Hostile inputs: the answer is JSON, the exit code is 0 (answered) or 2 (refused), never a crash. No expected values.
const HOSTILE = [
  ["abn", "\u0001\u0002"], ["name", "A".repeat(20000)], ["name", "'; DROP TABLE abn; --"], ["name", "\u202e\u0001harbour"], ["name", "%s%s%s%n"],
  ["address", "\u{1F4A5}".repeat(500)], ["address", "../../../../etc/passwd"], ["address", "--id", ""], ["stats", "get", "FIXTURE_POP", "--filter", "REGION"],
  ["stats", "get", "FIXTURE_POP", "--last", "-5"], ["area", "--lat", "NaN", "--lon", "Infinity"], ["area", "--lat", "1e999", "--lon", "0"], ["company", "9".repeat(5000)],
  ["name", "harbour", "--limit", "99999999999999999999"], ["name", "harbour", "--state", "\u0001"], ["bogus-verb"], [],
];

write(path.join(here, "golden.json"), JSON.stringify({ format: 1, note: "Generated by make.mjs. Synthetic data; see the header of make.mjs.", versions: VERSIONS, cases: G, hostile: HOSTILE }, null, 1) + "\n");
console.log(`fixtures written: ${G.length} golden cases, ${HOSTILE.length} hostile inputs`);
