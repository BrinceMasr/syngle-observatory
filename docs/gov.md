# Australian government data lookup (`syngle-map gov`)

Look up Australian companies, businesses, addresses, statistics and statistical areas from the official sources, on your own machine, in milliseconds. Nothing is
preloaded: a source is downloaded only when you ask for it (`gov refresh`), and every answer says which data it came from.

The code is the library `crates/observatory-gov` and the verb group in `apps/rustgis-cli/src/gov.rs`. The spec is
`agents/docs/specs/map-hill-climb-and-gov-data.md` section 7 (Syngle-Ecosystem #628, #629, #630).

## What it answers

| Verb | Answer |
|---|---|
| `gov abn <ABN or ACN> [--live]` | one entity: `abn, status, statusFrom, entityType, name, state, postcode, acn, gst, names[{type,text}]`. The ABN check digit is validated (`invalid-abn`); an ACN is 9 digits |
| `gov name <text> [--state S] [--active] [--limit N]` | entities by name over the ABN extract's legal, business and trading names, the ASIC company names (current and former) and the ASIC business names |
| `gov company <ACN>` | `acn, name, status, registered, deregistered, abn, formerNames[]` |
| `gov business-names <ABN>` | the ASIC business names of an ABN |
| `gov address <text> [--limit N]` or `gov address --id <G-NAF PID>` | `addressId, address, lat, lon, locality, state, postcode`. Abbreviations (`St`, `Rd`, `Unit 3/12`) are understood; the closest match comes first |
| `gov stats find <words>` | the ABS dataflows whose id or name has every word (as the start of a word): `id, version, name` |
| `gov stats get <dataflow> [--filter DIM=VAL]... [--from P] [--to P] [--last N]` | observations of a dataflow from the snapshot: `period, value, unit, dims` |
| `gov area --lat Y --lon X` | the ASGS areas that contain a point: `layer, code, name, state` |
| `gov status` | the snapshot id and, for each source, `dataVersion`, `asOf`, `licence`, `publisher`, `records`, and the snapshots kept |
| `gov abn <ABN or ACN> --live` | the same entity as ABN Lookup holds it now, fetched on the spot and never stored (see "Live lookup") |
| `gov refresh [--source abr,asic,asicBn,gnaf,abs,asgs] [--flow ID,..] [--check]` | downloads the newest official files of the sources you turn on, builds and switches (see "Refreshing from the official hosts") |
| `gov refresh --from <folder>` | builds a snapshot from official files you already hold and a `manifest.json` (below) |

All verbs accept `--data-version <snapshot id>` (a kept older snapshot; the last 3 are kept) and `--data-dir <folder>`.
Every verb prints one line of JSON and a newline. Exit 0 means answered, 2 means refused (`"ok":false` and a fixed error
code), and `gov refresh --check` exits 10 when something is newer. A person or an agent never sees a panic or a line that is not JSON.

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
downloads/state.json    which sources are turned on and the release of each that is indexed
downloads/<source>/<id>/  the files of that release, kept until the next release is indexed
downloads/.lock         held while a refresh runs
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

## Refreshing from the official hosts

Nothing is downloaded until you name a source: `gov refresh --source asic` turns the ASIC companies on, fetches them and builds.
After that, `gov refresh` with no flags brings every source that is on up to date, and `gov refresh --check` says what is newer
and downloads nothing: exit 0 up to date (or nothing is on), 10 something is newer (the `updates` list names it), 2 an error.

```json
{"ok":true,"check":true,"upToDate":false,"updates":["asic"],"sources":[{"source":"asic","status":"update","installed":{"asOf":"2026-10-09","lastModified":"2026-10-09T13:53:06.104534","size":78120331,"files":1},"latest":{"asOf":"2026-11-09","lastModified":"2026-11-09T13:53:06.104534","size":79335223,"files":1}}]}
```

A source's `status` is `not-installed`, `current`, `update`, or `unknown` (the ABS observations: the gateway states no date for its data, so a check cannot say, and a refresh fetches them again).

| Source | Where the newest file is found | Compared by |
|---|---|---|
| `abr`, `asic`, `asicBn`, `gnaf` | CKAN `package_show` on data.gov.au (dataset ids in `ckan.rs`): the ZIPs of the ABN extract, the newest `Company Dataset` and `Business Names Dataset` ZIP, the newest `GDA2020` G-NAF ZIP | each resource's own `last_modified` and `size` (CKAN's `metadata_modified` only says the page was edited) |
| `asgs` | the ABS digital boundary files page has no CKAN record, so the address of the main structure GeoPackage ZIP is pinned in `net.rs` | the server's `Last-Modified` (it states no size) |
| `abs` | the dataflow list and one SDMX-CSV per flow from `data.api.abs.gov.au/rest`, with `Accept: application/vnd.sdmx.data+csv`; `--flow CPI,LF` picks the flows (default `CPI,LF,WPI,ERP_Q,RT`). The gateway answers at most 10 MB in 30 s, so a flow it refuses is fetched again in slices of five years | the day it was fetched |

How a refresh is kept safe:

- **Hosts.** Only `data.gov.au`, `abs.gov.au` (and the ABS gateway) and `abr.business.gov.au`, over https, at every redirect. A file address a CKAN record gives on any other host is refused, and so is a redirect that leaves the list. Nothing is scraped from a web page: CKAN, the ABS SDMX API, ABN Lookup and the official files only.
- **Disk.** A refresh is refused before it fetches a byte when the free space is under 2.5 times what is left to download (a part already on disk counts as downloaded).
- **Resume.** A file arrives as `<name>.<release>.part`; a rerun asks for the rest with a `Range` request and continues. A part of another release, or one longer than the file, is dropped. The size must match the record, the hash must match when CKAN publishes one (MD5), and a ZIP is read to its end so every entry's CRC is checked. Only then is the file renamed to its name. The ABS site (the ASGS file) says it takes ranges but answers one with 200 and the whole file, and states no length: a run that breaks there starts that file again, and a truncated ZIP is caught by the check, never indexed.
- **Nothing changes until the end.** The snapshot is built from the downloaded files (the ABN extract's `RecordCount` is checked in that build) and switched last; `state.json` moves after that and the old release's folder goes after that. A refresh that fails, or is killed, leaves the old snapshot answering and the downloaded files for the next run.
- **One at a time.** A second refresh on the same data folder is refused (`refresh-failed`, "another refresh is running").
- **Files you gave with `--from`.** A snapshot built from a folder holds sources a network refresh would leave out; it is refused, naming them, until you name them in `--source`.

A start-up check is four CKAN calls and one HEAD request: about 1.5 s the first time and under 0.1 s after.

### The schedule

Syngle runs `syngle-map gov refresh --check` when it starts and daily, and `syngle-map gov refresh` weekly. With no source turned on the check exits 0 and the refresh exits 2 with `no-data`, so the schedule downloads nothing a person did not ask for. This is the contract the scheduler (a Pi-GUI ticket) calls.

## Live lookup (`gov abn <ABN> --live`)

One call to the ABN Lookup JSON services (`https://abr.business.gov.au/json/AbnDetails.aspx`, or `AcnDetails.aspx` for an ACN), for the entity as the register holds it today. The answer has the same entity as `gov abn` plus `"live":true`, `"dataVersion":"live"` and, under `sources.abnLookup`, `"asOfKind":"fetched"` with today's date. It comes from no snapshot, is never stored, and is the one answer that is not the same twice. An ABN the register does not hold is `count:0`.

The GUID comes from the environment variable `SYNGLE_ABN_LOOKUP_GUID` and nowhere else. It is checked for its shape (8-4-4-4-12 hex digits) before any request, goes only in the request's address (the service takes it nowhere else), and is never printed, written, logged or put in an answer or an error: a message names the host, and anything the service says back is scrubbed of the GUID first. A missing GUID is `no-data` and a malformed one `bad-source`, both before anything is sent. Register for one, free, at https://abr.business.gov.au/Documentation/WebServiceRegistration (ABR e-mails it).

The web services agreement (version 9.9.7, read in full on 14 November 2026) says:

- You get a non-exclusive, non-transferable licence for the term of the agreement. **Each person registers their own GUID; no GUID ships with the program or is shared.**
- You may give relevant extracts to third parties at your own risk, must not misrepresent your rights, and must not imply the Commonwealth endorses, approves or is affiliated with you or your product. The attribution of a live answer says the Commonwealth does not endorse it.
- ABR may withdraw details (for example to protect a person's privacy). If it tells you specific information was withdrawn, you must delete every copy you hold. A live answer is never stored here, so this program holds none; a copy you or an agent saved elsewhere is yours to delete.
- The service is "as is, as available", may be altered or suspended, and is not warranted correct, current or complete. Liability is excluded to the extent the law allows, and you indemnify ABR against third party claims from your use. Either side may end the agreement (ABR with 30 days' notice); ABR may suspend your access while it investigates a breach.
- The agreement has no rate limit and no clause on caching. The program makes one call per `--live` query and repeats none.
- It defines the services as the sites `https://abr.business.gov.au/abrxmlsearch` and `abrxmlsearchRPC`, "or such other location as we may notify". The JSON services live at `/json/` and ABR documents them as its web services (https://abr.business.gov.au/json/), so they are read as covered by the same registration. The registration's own terms are what binds, and the live proof will show whether the GUID is accepted there.

What is proven: the field names (`Abn`, `AbnStatus`, `AbnStatusEffectiveFrom`, `Acn`, `AddressPostcode`, `AddressState`, `BusinessName[]`, `EntityName`, `EntityTypeName`, `Gst`, `Message`) are those the real endpoint answers with, seen from its reply to an unregistered GUID (`"The GUID entered is not recognised as a Registered Party"`, which this program turns into a message that names the variable). The values of a found entity (date formats, `Gst` as a date, `BusinessName` as a list of text) follow ABR's documentation and the tests' hand-written replies (only the unregistered-GUID reply in `tests/live.rs` is a real recording), and have not been seen from a registered GUID yet.

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

On a boat box (8 vCPU, 15 GB), over 1,353,512 generated records (500,000 ABNs, 200,000 companies, 150,000 business names, 500,000 addresses,
1,228 dataflows and 2,400 areas, `evals/gov-data/gen-scale.mjs`), one process per call, process start included, median of five runs with the range in
brackets: `abn` 3.3 ms p95 (1.9 to 6.4), `name` 4.7 (3.6 to 6.9), `address` 4 (3.7 to 7.8), `stats` 6.7 (4.4 to 9.1), `area` 3.8 (3 to 4.8), the first call
5 ms (2 to 8), a build 4 s per million records. The bars are 100, 150, 150, 100, 100, 400 ms and 120 s. The evaluator's own no-op process read 2.8 ms p95 on this box, so
most of every figure is the cost of starting a process.

On the real files (the same box):

| Source | File | Build | Memory | Index | Calls |
|---|---|---|---|---|---|
| ABN Bulk Extract, 14 Nov 2026 | both ZIPs fetched by `gov refresh --source abr` (1.0 GB), 20,658,780 records | 3 min 10 s with the download | 2.2 GB | about 3.3 GB | `abn` 0.2 ms |
| ASIC companies and business names | `gov refresh --source asic,asicBn` (79 MB and 72 MB), 4,024,956 companies and 3,312,255 business names | 23 s with the download | 1.6 GB | 0.9 GB | `company` 0.1 ms, `name` 0.2 ms, `name pty ltd` 55 ms |
| G-NAF August 2026, GDA2020 ZIP (1.85 GB) | 16,091,511 principal addresses | 66 s | 0.7 GB | 1.5 GB | a street address 0.3 to 15 ms, a lone `street` or `nsw` 60 to 100 ms |
| ASGS SA2 and state Shapefiles (72 MB) | 2,483 areas | 0.4 s | 45 MB | 100 MB | 2 to 4 ms |
| ASGS main structure GeoPackage ZIP, GDA2020 (529 MB), fetched by `gov refresh --source asgs` | 433,118 areas in 8 layers | 18 s with the download | 118 MB | 736 MB | 14 to 33 ms |
| ABS dataflow list and the CPI and WPI observations, fetched by `gov refresh --source abs --flow CPI,WPI` (22 MB) | 1,228 flows | 8 s with the download | 74 MB | small | `stats find` 0.8 ms, `stats get` 1.3 ms |

The index keeps the last three snapshots (`--data-version`), so the disk holds up to three indexes at once.

## Terms

The ABN extract, the ASIC company dataset and the ASIC business names are CC BY 3.0 AU from data.gov.au. ABN records include sole
traders' names as the ABR publishes them; `gov name` returns them as published and adds nothing. G-NAF is CC BY 4.0 and the Open G-NAF
End User Licence Agreement: it must not be used to generate or compile mailing addresses unless each is verified against a secondary
source, and every address answer says so in its attribution. ABS statistics and the ASGS boundaries are CC BY 4.0, sourced to the
Australian Bureau of Statistics. Answers carry the licence and attribution of each source they used.

The ABN Lookup web services have their own agreement, recorded under "Live lookup".
