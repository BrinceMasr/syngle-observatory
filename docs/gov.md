# Australian government data lookup (`syngle-map gov`)

Look up Australian companies and businesses from the official registers, on your own machine, in milliseconds. Nothing is
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
| `gov status` | the snapshot id and, for each source, `dataVersion`, `asOf`, `licence`, `publisher`, `records`, and the snapshots kept |
| `gov refresh --from <folder>` | builds a snapshot from official files and a `manifest.json` (below) |

All verbs accept `--data-version <snapshot id>` (a kept older snapshot; the last 3 are kept) and `--data-dir <folder>`.
Every verb prints one line of JSON and a newline. Exit 0 means answered, 2 means refused (`"ok":false` and a fixed error
code). A person or an agent never sees a panic or a line that is not JSON.

Ranking of `gov name` is total and fixed, with no fuzzy matching: class (the normalised name equals the query, or starts
with it, or has every query word as a word in any order), then active before cancelled, then the matched name's length, then
the matched name, then the ABN. A name is normalised by upper-casing, reading `&` as `AND`, turning every other character
that is not A to Z or 0 to 9 into a space and collapsing spaces. `--limit` defaults to 5 and is at most 50.

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

Building streams: the XML is a pull tokenizer over a `BufRead` (a ZIP is read entry by entry), and every table that has to be
sorted goes through an external sort that writes runs to disk past 256 MB, so memory does not grow with the 18 million entities
of the real ABN extract. The ASIC files are held in memory while they are grouped (about 4 million rows).

## Refreshing from a folder

`gov refresh --from <folder>` reads `<folder>/manifest.json`:

```json
{"format":1,"sources":{"abr":{"asOf":"2026-11-05","files":["abn/public_split_1_10.xml"],"licence":"CC BY 3.0 AU","publisher":"Australian Business Register"}}}
```

`asOf` is `yyyy-mm-dd`; `files` are paths inside the folder (a `.zip` is read, each file in it in name order); the source names are
`abr`, `asic`, `asicBn` (and the sources of the later tickets). It prints the new `dataVersion`, `built` (false when the same files
were already indexed) and each source's version, as-of date and record count. The ABN extract states its `RecordCount`: a file
with another number of `ABR` records, or with no closing `Transfer`, is refused.

## Measured

On a boat box (4 vCPU), over 500,000 ABNs, 200,000 companies and 150,000 business names (`evals/gov-data/gen-scale.mjs`), process
start included: `abn` p95 4 ms, `name` p95 5 ms, the first call 4 ms, a build of 850,000 records 3 s. The bars are 100, 150, 400 ms
and 120 s per million records.

## Terms

The ABN extract, the ASIC company dataset and the ASIC business names are CC BY 3.0 AU from data.gov.au. ABN records include sole
traders' names as the ABR publishes them; `gov name` returns them as published and adds nothing. Answers carry the licence and
attribution of each source they used.
