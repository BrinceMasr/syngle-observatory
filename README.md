# Syngle Observatory

Syngle Observatory is Syngle's map sub-app: fast maps of research data, with an agent that does
the GIS work. It is a **hard fork of [RustGIS](https://github.com/opengeos/RustGIS)** (Apache-2.0,
pinned at `3aabc83`), owned by Syngle, changed freely and **never merged back**. Upstream is a
source to read, not a merge target; see [NOTICE](NOTICE) for the fork and modification notice and
[ATTRIBUTION.md](ATTRIBUTION.md) for every third-party work.

## What the fork ships

Per [ADR 0005 and ADR 0010](https://github.com/BrinceMasr/Syngle-Ecosystem/blob/main/agents/docs/adr/)
in the Syngle-Ecosystem repo:

- **A canvas-only WASM view.** The fork's Rust map view compiled to WebAssembly, drawing the map
  canvas and direct manipulation (pan, zoom, select, drag) and nothing else. SyngleCode's React UI
  is the chrome: a layers rail, a slim 8-tool bar, a floating Style/Data/Source card and the chat.
- **A native engine sidecar.** `crates/observatory-engine` (binary `syngle-observatory-engine`)
  owns the datasets, loads them with the fork's own readers, serves them to the view over one
  loopback HTTP surface with byte ranges, and speaks the JSON-lines control protocol on
  stdin/stdout.
- **Everything else** — ribbon, panes, dialogs, title bar, plugins UI — is the fork's stock egui
  app. It still builds, but Syngle does not ship it.

## Build it

Requirements: Rust 1.90+ with the `wasm32-unknown-unknown` target, [Trunk](https://trunkrs.dev)
0.21+, and a C compiler on `PATH` named `cc` (the Zstandard decoder is C and must compile for
WebAssembly too; clang works, e.g. `CC_wasm32_unknown_unknown=clang`).

```sh
# The canvas-only WASM view -> dist/canvas
cd apps/rustgis-web
trunk build --release --features canvas-only --dist ../../dist/canvas

# The native engine sidecar (release)
cargo build --release -p syngle-observatory-engine

# Fast crate checks (the whole workspace is Rust 2024; unsafe is forbidden)
cargo test -p syngle-observatory-engine
cargo xtask ci        # fmt + clippy -D warnings + tests + the layer check
cargo xtask rebrand   # no upstream branding may creep back into the fork
```

The sidecar serves the view and the datasets from one origin; see the crate header in
`crates/observatory-engine/src/lib.rs` for the protocol and the flags.

## Repository layout

| Path | What |
|---|---|
| `crates/geom … ui-egui` | the fork's layered Rust crates (architecture table in [AGENTS.md](AGENTS.md)) |
| `crates/observatory-engine` | the native engine sidecar (fork crate; supersedes spike A) |
| `apps/rustgis-web` | the WASM view; `--features canvas-only` is the Syngle build |
| `apps/rustgis`, `apps/rustgis-cli`, `apps/rustgis-python` | the stock desktop, CLI and Python apps |
| `syngle-spike/` | spike A's host, bench and dataset generator (measurement harness) |
| `docs/` | the upstream user and developer docs, kept as reference |

## Provenance and licence

Syngle Observatory is a fork of RustGIS, Copyright the RustGIS contributors, licensed under the
Apache License 2.0. The [LICENSE](LICENSE) is unchanged; the [NOTICE](NOTICE) records the fork and
the modifications; [ATTRIBUTION.md](ATTRIBUTION.md) lists every third-party work and its licence.
Upstream's own documentation, website and services are not part of this fork.
