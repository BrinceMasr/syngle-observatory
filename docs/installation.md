# Installation

The fork publishes nothing on its own: no installers, no store listing, no package-manager
packages, no website and no download mirror. Syngle Observatory ships inside Syngle, where
SyngleCode's desktop process starts the native engine and hosts the canvas-only WASM view. To
work on the fork itself, build it from source.

## Build from source

A recent stable Rust toolchain (1.90 or later) with the `wasm32-unknown-unknown` target, and
[Trunk](https://trunkrs.dev) 0.21 or later for the view.

```sh
git clone https://github.com/BrinceMasr/syngle-observatory.git
cd syngle-observatory

# The canvas-only WASM view (ADR 0010), written to dist/canvas
cd apps/rustgis-web
trunk build --release --features canvas-only --dist ../../dist/canvas

# The native engine sidecar, release
cargo build --release -p syngle-observatory-engine

# Fast crate checks: fmt, clippy -D warnings, tests, the layer check and the rebrand gate
cargo xtask ci
```

The WebAssembly build needs a C compiler on `PATH` named `cc` for the `wasm32` target as well, for
the C Zstandard decoder of the GeoTIFF path; clang works (`CC_wasm32_unknown_unknown=clang`).
Linux distributions' clang does; on macOS, Apple's does not, so install `llvm` and put its `bin`
first on your `PATH`.

The build downloads the Natural Earth sample data (a few hundred KB) from the upstream
project's `rustgis-assets` repository (credited in [ATTRIBUTION.md](../ATTRIBUTION.md)). To build
offline, download those files once and point the build at them:

```sh
RUSTGIS_DATA_DIR=/path/to/data cargo build --release
```

## Platforms

| Platform | Status |
| --- | --- |
| Windows | Supported, and first-class: the platform Syngle ships |
| macOS | Supported |
| Linux | The crates build, but Syngle does not ship Linux (ADR 0004) |

The fork's own checks run on Windows and Linux; there is no continuous integration on this
repository (GitHub Actions is off), so everything is verified locally.

See the [README](../README.md) for the checks to run, and [Architecture](architecture.md) for how
the crates fit together.
