# Australian government data lookup (`syngle-map gov`)

Look up Australian companies, businesses, addresses, statistics and statistical areas from the official sources, on your own machine, in milliseconds. Nothing is
preloaded: a source is downloaded only when you ask for it (`gov refresh`), and every answer says which data it came from.

The code is the library `crates/observatory-gov` and the verb group in `apps/rustgis-cli/src/gov.rs`. The spec is
`agents/docs/specs/map-hill-climb-and-gov-data.md` section 7 (Syngle-Ecosystem #628, #629, #630).

## What it answers

| Verb | Answer |
|---|---|
| `gov abn <ABN or ACN>` | one entity: `abn, status, statusFrom, entityType, name, state, postcode, acn, gst, names[{type,text}]`. The ABN check digit is validated (`invalid-abn`); an ACN is 9 digits |
| `gov name <text> [--state S] [--active] [--limit N]` | entities by name over the ABN extract's legal, business and trading names, the ASIC company names (current and former) and the ASIC business names |
| `gov company <ACN>` | `acn, name, status, registered, deregistered, abn, formerNames[]` |
| `gov business-names <ABN>` | the ASIC business names of an ABN |
| `gov address <text> [--limit N]` or `gov address --id <G-NAF PID>` | `addressId, address, lat, lon, locality, state, postcode`. Abbreviations (`St`, `Rd`, `Unit 3/12`) are understood; the closest match comes first |
| `gov stats find <words>` | the ABS dataflows whose id or name has every word (as the start of a word): `id, version, name` |
| `gov stats get <dataflow> [--filter DIM=VAL]... [--from P] [--to P] [--last N]` | observations of a dataflow from the snapshot: `period, value, unit, dims` |
| `gov area --lat Y --lon X` | the ASGS areas that contain a point: `layer, code, name, state` |
| `gov status` | the snapshot id and, for each source, `dataVersion`, `asOf`, `licence`, `publisher`, `records`, and the snapshots kept |
| `gov refresh --from <folder>` | builds a snapshot from official files and a `manifest.json` (below) |

All verbs accept `--data-version <snapshot id>` (a kept older snapshot; the last 3 are kept) and `--data-dir <folder>`.
Every verb prints one line of JSON and a newline. Exit 0 means answered, 2 means refused (`"ok":false` and a fixed error
code). A person or an agent never sees a panic or a line that is not JSON.

Ranking of `gov name` is total and fixed, with no fuzzy matching: class (the normalised name equals the query, or starts
with it, or has every query word as a word in any order), then active before cancelled, then the matched name's length, then
the matched name, then the ABN. A name is normalised by upper-casing, reading `&` as `AND`, turning every other character
that is not A to Z or 0 to 9 into a space and collapsing spaces. `--limit` defaults to 5 and is at most 50.

Address matching: a query's words (a `unit`, `flat`, `apt`, `suite` or `shop` word dropped, `/` read as a space) must each be a
word of the address or its long form (`ST` for `STREET`, `RD`, `AVE`, `DR`, `CT`, `PDE`, `HWY`, `LN`, `PL`, `CL`, `CRES`, `TCE`,
`BLVD`, `SQ`). The address with the fewest words beyond the query's comes first and the G-NAF PID breaks ties. The street suffix
(`N`, `S`, `EAST`) is part of the address and of the match, so `10 main rd n` finds `10 MAIN ROAD N`.

`stats get` returns every observation that passes the filters, in period order (equal periods in the file's order): use
`--filter` or `--last` for a series, because a whole dataflow is thousands of rows. A blank observation is `value:null`, never 0.
`--from` and `--to` compare periods as text (`2025-Q4`, `2026-03`, `2024`).

`gov area` needs the ASGS layers you indexed (an SA2 file gives SA2 areas; the main structure GeoPackage gives every level). A
point on a boundary is in whichever side the even-odd test puts it; holes are respected.

## The answer

```json
{"ok":true,"dataVersion":"gov-1a2b3c4d","sources":{"abr":{"dataVersion":"abr-2026-11-05-e17180cc","asOf":"2026-11-05","licence":"CC BY 3.0 AU","attribution":"Australian Business Register, via data.gov.au"}},"query":{"verb":"abn","abn":"14100200301"},"count":1,"results":[{"abn":"14100200301","status":"Active"}]}
```

`dataVersion` names the snapshot and `sources` lists, for the sources the answer used, each one's version, as-of date,
licence and attribution. Always tell the person the `dataVersion` and `asOf`.

Deterministic means the same query on the same `dataVersion` gives the same bytes on stdout, from any working folder, time
zone or locale. The version is a hash of the source files (never the clock), no answer holds a time, an elapsed time or a
random id, every ranking ends on a stable id, and keys are written in a fixed order. Timing goes to stderr under `--timing`.

Error codes: `invalid-abn`, `invalid-acn`, `invalid-coordinates`, `invalid-limit`, `invalid-filter`, `empty-query`,
`unknown-verb`, `unknown-dataflow`, `unknown-data-version`, `no-data`, `refresh-failed`, `bad-source`.

## Where the data lives

`SYNGLE_GOV_DIR`, else `--data-dir`, else `<OS local data folder>/syngle-map/gov` (`%LOCALAPPDATA%` on Windows,
`~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or `~/.local/share` elsewhere). Never the installed Syngle's folder.

```
current                 the id answers come from (switched last, by a rename)
history                 the ids kept, oldest first
snapshots/<id>/         meta.json and the table files; written once, in a temporary folder, then renamed
```

A refresh that fails (a truncated file, a count that does not match, a file of another shape) changes nothing: no snapshot
appears, `current` and the answers stay as they were.

## How the index is read

A snapshot is sorted tables of fixed-width entries and record files. `unsafe` is forbidden in this fork, so there is no memory
map: a table is read at an offset (`pread` on Unix, `seek_read` on Windows) and found by binary search, and the operating
system's page cache is the cache. A call is one process that opens only the tables its verb needs.

| File | Holds |
|---|---|
| `abn.rec`, `abn.idx`, `acn.idx` | the ABN extract's entities; ABN to record; ACN to ABN |
| `co.rec`, `co.idx` | the ASIC companies (one per ACN, former names with it) |
| `bn.rec`, `bn.idx` | the ASIC business names; ABN to record |
| `nm.ent`, `nm.str` | every name, normalised and sorted: an exact or prefix match is one range of the table |
| `tok.key`, `tok.str`, `tok.post` | for each word, the entries whose name has it: "every word, any order" is a list intersection |
| `addr.rec`, `addr.ent`, `gn.street.*`, `gn.loc.*` | the addresses (with their coordinates), each one's word count, and the streets and localities they point at |
| `atok.key`, `atok.str`, `atok.post` | the address word lists: the shortest lists are read and the long ones (`STREET`, `NSW`) are only probed |
| `abs.flows`, `abs.dat`, `abs.cat` | the dataflow catalogue, and one block per dataflow: dictionary-coded columns, rows in period order |
| `area.rec`, `area.idx`, `area.cell` | the ASGS areas with their rings, and a 0.1 degree grid of bounding boxes: a point reads one cell and tests only the areas whose box holds it |

Building streams: the XML is a pull tokenizer over a `BufRead` (a ZIP is read entry by entry), and every table that has to be
sorted goes through an external sort that writes runs to disk past 256 MB, so memory does not grow with the 18 million entities
of the real ABN extract. The ASIC files are held in memory while they are grouped (about 4 million rows).

## Refreshing from a folder

`gov refresh --from <folder>` reads `<folder>/manifest.json`:

```json
{"format":1,"sources":{"abr":{"asOf":"2026-11-05","files":["abn/public_split_1_10.xml"],"licence":"CC BY 3.0 AU","publisher":"Australian Business Register"}}}
```

`asOf` is `yyyy-mm-dd`; `files` are paths inside the folder (a folder stands for the files in it; a `.zip` is read, each file in it
in name order). The source names, and what their files are:

| Source | Files |
|---|---|
| `abr` | the ABN Bulk Extract XML, or its ZIPs |
| `asic`, `asicBn` | the ASIC company and business names datasets (tab separated, `.csv`), or their ZIPs |
| `gnaf` | the G-NAF release ZIP, or its `Standard` folder: `<STATE>_ADDRESS_DETAIL_psv.psv`, `STREET_LOCALITY`, `LOCALITY`, `ADDRESS_DEFAULT_GEOCODE` for each state. Retired addresses and alias rows (`ALIAS_PRINCIPAL` = `A`) are left out, so the count is the principal addresses |
| `abs` | `dataflows.json` (the SDMX-JSON list from `data.api.abs.gov.au/rest/dataflow/all`) and one SDMX-CSV file per dataflow fetched, named `<DATAFLOW>.csv` |
| `asgs` | boundary files, any mix of GeoJSON, Shapefile (`.shp` with its `.dbf`, CP1252 or UTF-8) and GeoPackage, or ZIPs of them. A file's layer is the first word of its name (`SA2_2021_AUST_GDA2020` is `SA2`); the code and name are the layer's own `<LAYER>_CODE` and `<LAYER>_NAME` attributes (`STATE_` for `STE`) |

It prints the new `dataVersion`, `built` (false when the same files were already indexed) and each source's version, as-of date and
record count. The ABN extract states its `RecordCount`: a file with another number of `ABR` records, or with no closing `Transfer`, is
refused, and so is a G-NAF table without its columns, an observation file without `TIME_PERIOD` or `OBS_VALUE`, and a boundary file
that is not valid.

## Measured

On a boat box (8 vCPU), over 500,000 ABNs, 200,000 companies, 150,000 business names, 500,000 addresses, 1,228 dataflows and 2,400
areas (`evals/gov-data/gen-scale.mjs`), process start included, p95 of five runs: `abn` 6 ms, `name` 6 ms, `address` 6 ms, `stats` 8 ms,
`area` 6 ms, the first call 3 ms, a build 4 s per million records. The bars are 100, 150, 150, 100, 100, 400 ms and 120 s.

On the real files (boat box, 8 vCPU, 15 GB):

| Source | File | Build | Memory | Index | Calls |
|---|---|---|---|---|---|
| ABN extract and ASIC | generated at 6 million ABNs, 2.4 million companies (the real ABN file is about 18 million) | 34 s | 1.2 GB | 1.7 GB | `abn` 0.1 to 1 ms, `name` 3 to 5 ms, `name pty` (a word in most names) 80 ms |
| G-NAF August 2026, GDA2020 ZIP (1.85 GB) | 16,091,511 principal addresses | 66 s | 0.7 GB | 1.5 GB | a street address 0.3 to 15 ms, a lone `street` or `nsw` 60 to 100 ms |
| ASGS SA2 and state Shapefiles (72 MB) | 2,483 areas | 0.4 s | 45 MB | 100 MB | 2 to 4 ms |
| ASGS main structure GeoPackage GDA2020 (529 MB) | 433,118 areas in 8 layers | 4.5 s | 118 MB | 736 MB | 20 to 30 ms |

## Terms

The ABN extract, the ASIC company dataset and the ASIC business names are CC BY 3.0 AU from data.gov.au. ABN records include sole
traders' names as the ABR publishes them; `gov name` returns them as published and adds nothing. G-NAF is CC BY 4.0 and the Open G-NAF
End User Licence Agreement: it must not be used to generate or compile mailing addresses unless each is verified against a secondary
source, and every address answer says so in its attribution. ABS statistics and the ASGS boundaries are CC BY 4.0, sourced to the
Australian Bureau of Statistics. Answers carry the licence and attribution of each source they used.
