// Parsers for the official file shapes, for the reference shim only (evals/gov-data/reference/naive.mjs). Deliberately
// naive: whole file in memory, regular expressions. The product reads the real files (a 500 MB XML, a 5 GB G-NAF) in a
// stream. See fixtures/make.mjs for the shapes and where they were checked.
import fs from "node:fs";
import path from "node:path";

const iso = (yyyymmdd) => (yyyymmdd && /^\d{8}$/.test(yyyymmdd) ? `${yyyymmdd.slice(0, 4)}-${yyyymmdd.slice(4, 6)}-${yyyymmdd.slice(6)}` : null);
const isoDmy = (dmy) => {
  const m = /^(\d\d)\/(\d\d)\/(\d{4})$/.exec(dmy ?? "");
  return m ? `${m[3]}-${m[2]}-${m[1]}` : null;
};
const unxml = (text) => text.replaceAll("&lt;", "<").replaceAll("&gt;", ">").replaceAll("&quot;", '"').replaceAll("&apos;", "'").replaceAll("&amp;", "&");

/** ABN Bulk Extract XML (bulkextract.xsd) to records. */
export function parseAbn(file) {
  const xml = fs.readFileSync(file, "utf8");
  // A file cut short must be refused whole, not indexed as fewer records: the extract says how many it holds.
  if (!/<\/Transfer>\s*$/.test(xml)) throw new Error(`${path.basename(file)} is cut short (no closing </Transfer>)`);
  const declared = Number((/<RecordCount>(\d+)<\/RecordCount>/.exec(xml) ?? [])[1]);
  const out = [];
  for (const block of xml.matchAll(/<ABR\b[^>]*>([\s\S]*?)<\/ABR>/g)) {
    const b = block[1];
    const abn = /<ABN\b([^>]*)>(\d{11})<\/ABN>/.exec(b);
    if (!abn) continue;
    const text = (re) => (re.exec(b) ?? [])[1];
    const individual = /<IndividualName\b[^>]*>([\s\S]*?)<\/IndividualName>/.exec(b);
    let name = unxml(text(/<MainEntity>[\s\S]*?<NonIndividualNameText>([\s\S]*?)<\/NonIndividualNameText>/) ?? "");
    if (individual) {
      const given = [...individual[1].matchAll(/<GivenName>([^<]*)<\/GivenName>/g)].map((m) => m[1]);
      name = `${text(/<FamilyName>([^<]*)<\/FamilyName>/)}, ${given.join(" ")}`.trim();
    }
    const gst = /<GST\b[^>]*status="(\w+)"/.exec(b);
    out.push({
      abn: abn[2],
      status: /status="ACT"/.test(abn[1]) ? "Active" : "Cancelled",
      statusFrom: iso((/ABNStatusFromDate="(\d{8})"/.exec(abn[1]) ?? [])[1]),
      entityType: unxml(text(/<EntityTypeText>([\s\S]*?)<\/EntityTypeText>/) ?? ""),
      name,
      state: text(/<State>([^<]*)<\/State>/) ?? "",
      postcode: text(/<Postcode>([^<]*)<\/Postcode>/) ?? "",
      acn: text(/<ASICNumber\b[^>]*>(\d+)<\/ASICNumber>/) ?? null,
      gst: Boolean(gst && gst[1] === "ACT"),
      names: [...b.matchAll(/<OtherEntity>\s*<NonIndividualName type="(\w+)"><NonIndividualNameText>([\s\S]*?)<\/NonIndividualNameText>/g)].map((m) => ({ type: m[1], text: unxml(m[2]) })),
    });
  }
  if (out.length !== declared) throw new Error(`${path.basename(file)} holds ${out.length} records, its header says ${declared}`);
  return out;
}

/** The ASIC datasets: tab-separated, optional BOM, CRLF. Rows as objects keyed by the header. */
export function parseTsv(file) {
  const raw = fs.readFileSync(file, "utf8").replace(/^\uFEFF/, "");
  const lines = raw.split(/\r?\n/).filter((l) => l.length);
  const header = lines[0].split("\t");
  return lines.slice(1).map((l) => Object.fromEntries(l.split("\t").map((v, i) => [header[i], v])));
}

export function parseAsicCompany(file) {
  const byAcn = new Map();
  for (const r of parseTsv(file)) {
    const acn = r.ACN;
    const entry = byAcn.get(acn) ?? { acn, name: null, status: null, registered: null, deregistered: null, abn: null, formerNames: [] };
    const statusText = { REGD: "Registered", DRGD: "Deregistered" }[r.Status] ?? r.Status;
    if (r["Current Name Indicator"] === "Y") Object.assign(entry, { name: r["Company Name"], status: statusText, registered: isoDmy(r["Date of Registration"]), deregistered: isoDmy(r["Date of Deregistration"]), abn: r.ABN || entry.abn });
    else entry.formerNames.push(r["Company Name"]);
    byAcn.set(acn, entry);
  }
  return [...byAcn.values()].map((e) => ({ ...e, formerNames: [...new Set(e.formerNames)].sort() }));
}

export function parseBusinessNames(file) {
  return parseTsv(file).map((r) => ({ name: r.BN_NAME.trim(), status: r.BN_STATUS, registered: isoDmy(r.BN_REG_DT), cancelled: isoDmy(r.BN_CANCEL_DT), abn: r.BN_ABN || null }));
}

const psvRows = (file) => {
  const lines = fs.readFileSync(file, "utf8").split(/\r?\n/).filter((l) => l.length);
  const header = lines[0].split("|");
  return lines.slice(1).map((l) => Object.fromEntries(l.split("|").map((v, i) => [header[i], v])));
};

/** G-NAF tables (<STATE>_<TABLE>_psv.psv) to address records. Retired addresses are left out. */
export function parseGnaf(dir) {
  const out = [];
  for (const state of fs.readdirSync(dir).map((f) => /^([A-Z]+)_STATE_psv\.psv$/.exec(f)?.[1]).filter(Boolean)) {
    const t = (name) => psvRows(path.join(dir, `${state}_${name}_psv.psv`));
    const localities = new Map(t("LOCALITY").map((r) => [r.LOCALITY_PID, r]));
    const streets = new Map(t("STREET_LOCALITY").map((r) => [r.STREET_LOCALITY_PID, r]));
    const geocodes = new Map(t("ADDRESS_DEFAULT_GEOCODE").map((r) => [r.ADDRESS_DETAIL_PID, r]));
    for (const a of t("ADDRESS_DETAIL")) {
      if (a.DATE_RETIRED) continue;
      const street = streets.get(a.STREET_LOCALITY_PID);
      const locality = localities.get(a.LOCALITY_PID);
      const geo = geocodes.get(a.ADDRESS_DETAIL_PID);
      if (!street || !locality || !geo) continue;
      const flat = a.FLAT_NUMBER ? `${a.FLAT_TYPE_CODE ? `${a.FLAT_TYPE_CODE} ` : ""}${a.FLAT_NUMBER}/` : "";
      out.push({
        addressId: a.ADDRESS_DETAIL_PID,
        address: `${flat}${a.NUMBER_FIRST} ${street.STREET_NAME} ${street.STREET_TYPE_CODE}, ${locality.LOCALITY_NAME} ${state} ${a.POSTCODE}`,
        flatNumber: a.FLAT_NUMBER || null,
        number: a.NUMBER_FIRST,
        street: street.STREET_NAME,
        streetType: street.STREET_TYPE_CODE,
        locality: locality.LOCALITY_NAME,
        state,
        postcode: a.POSTCODE,
        lat: Number(geo.LATITUDE),
        lon: Number(geo.LONGITUDE),
      });
    }
  }
  return out;
}

/** The dataflow list (SDMX-JSON) and the SDMX-CSV observations of each file in `data/`. */
export function parseAbs(dir) {
  const flows = JSON.parse(fs.readFileSync(path.join(dir, "dataflows.json"), "utf8")).data.dataflows.map((f) => ({ id: f.id, version: f.version, name: f.name }));
  const data = {};
  const folder = path.join(dir, "data");
  if (fs.existsSync(folder)) {
    for (const f of fs.readdirSync(folder).filter((n) => n.endsWith(".csv"))) {
      const lines = fs.readFileSync(path.join(folder, f), "utf8").split(/\r?\n/).filter((l) => l.length);
      const header = lines[0].split(",");
      data[f.replace(/\.csv$/, "")] = lines.slice(1).map((l) => Object.fromEntries(l.split(",").map((v, i) => [header[i], v])));
    }
  }
  return { flows, data };
}

export function parseAsgs(dir) {
  const out = [];
  for (const f of fs.readdirSync(dir).filter((n) => n.endsWith(".geojson")).sort()) {
    const json = JSON.parse(fs.readFileSync(path.join(dir, f), "utf8"));
    const layer = (/^([A-Za-z0-9]+)_/.exec(f) ?? [])[1] ?? "AREA";
    for (const ft of json.features) {
      const props = ft.properties;
      const code = Object.entries(props).find(([k]) => /_CODE/.test(k))?.[1];
      const name = Object.entries(props).find(([k]) => /_NAME/.test(k) && !/STE_/.test(k))?.[1];
      const state = Object.entries(props).find(([k]) => /STE_NAME/.test(k))?.[1] ?? null;
      out.push({ layer, code, name, state, rings: ft.geometry.type === "Polygon" ? [ft.geometry.coordinates] : ft.geometry.coordinates });
    }
  }
  return out;
}
