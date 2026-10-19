# Syngle Observatory

**Syngle's map sub-app: fast maps of research data, with an agent that does the GIS work.**

This repository is the Rust side of Syngle Observatory: the native **engine sidecar** (loading,
indexing, geoprocessing, the MCP tools) and the **canvas-only WASM view** that draws the map and
handles direct manipulation. It is a hard fork of an upstream open-source GIS application
(credited in the repository's NOTICE file). The React chrome around the view — the layers rail,
the tool bar, the floating card and the chat — is Syngle's, and lives in the Syngle app, not here.

The fork publishes no website, so these pages document the fork's own code rather than a product
to download.

<div class="hero-actions" markdown>

[Build from source](installation.md){ .md-button .md-button--primary }
[Getting started](getting-started.md){ .md-button }
[Architecture](architecture.md){ .md-button }
[Compare](comparison/index.md){ .md-button }

</div>

## What the fork is

- **The engine** (`crates/observatory-engine`) loads and indexes files, serves tiles and byte
  ranges to the view, runs geoprocessing (catchments, joins, aggregations) and hosts the MCP
  tools. It is started and supervised per launch, on a port with a token.
- **The view** (`apps/rustgis-web`, the `canvas-only` feature) is the fork's Rust map compiled to
  WebAssembly: layers, basemap, overlays, the compare split, and the direct manipulation for the
  active tool (pan, zoom, click, box and lasso select, measure, annotate). It ships no egui
  chrome: no ribbon, panes, dialogs or title bar.
- **The bridge** between them is a typed, versioned message protocol; the view and the engine do
  not know about Syngle's React surface.
- **[Command Line](cli.md)** · **[MCP Server](mcp.md)** · **[Python](python.md)** ·
  **[Architecture](architecture.md)**: the inherited parts of the fork, each documented as it
  works in this tree.
