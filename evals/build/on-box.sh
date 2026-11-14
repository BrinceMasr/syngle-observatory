#!/usr/bin/env bash
# Runs ON a boat box, in the Observatory checkout (`boat.sh` syncs it and calls this). It makes everything the
# evaluators run against, so no cargo ever runs on the PC or in WSL (AGENTS.md of the workspace).
#
#   STEPS="canvas engine data" evals/build/on-box.sh      (default: all three; add "spike" for the uncapped canvas)
#
# Output, under $OUT (default /home/user/out/build), the folder `evals/*` calls a build (`--build <dir>`):
#   canvas/                              the canvas-only wasm view, as the release ships it
#   canvas-spike/                        the same with the `spike` feature (the 1M view windows): headroom study only
#   syngle-observatory-engine.exe        the sidecar for Windows (cargo-xwin)
#   syngle-map.exe                       rustgis-cli, which the app ships as `syngle-map`
#   data/points_<n>.fgb                  the max-points ladder (syngle-spike/gen: deterministic, clustered over Australia)
#   data/countries.geojson               the sample the release stages
#   build.json                           commit, tree state, toolchain versions, when
set -euo pipefail

OUT="${OUT:-/home/user/out/build}"
STEPS="${STEPS:-canvas engine data}"
LADDER="${LADDER:-10000 20000 30000 50000 100000 200000 500000 1000000}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
mkdir -p "$OUT"
has() { [[ " $STEPS " == *" $1 "* ]]; }
say() { printf '[on-box] %s\n' "$*"; }

if has canvas || has spike; then
  rustup target add wasm32-unknown-unknown
  if ! command -v trunk >/dev/null 2>&1; then
    say "installing trunk 0.22.0 (the version the release builds with)"
    curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.22.0/trunk-x86_64-unknown-linux-gnu.tar.gz | tar xz -C "$HOME/.cargo/bin"
  fi
fi
if has canvas; then
  say "trunk build (canvas-only, release)"
  (cd apps/rustgis-web && trunk build --release --features canvas-only --dist "$OUT/canvas")
fi
if has spike; then
  say "trunk build (canvas-only,spike)"
  (cd apps/rustgis-web && trunk build --release --features canvas-only,spike --dist "$OUT/canvas-spike")
fi
if has engine; then
  say "cargo xwin build (engine and syngle-map for Windows)"
  cargo xwin build --release --locked --target x86_64-pc-windows-msvc -p syngle-observatory-engine -p rustgis-cli
  cp target/x86_64-pc-windows-msvc/release/syngle-observatory-engine.exe "$OUT/"
  cp target/x86_64-pc-windows-msvc/release/rustgis-cli.exe "$OUT/syngle-map.exe"
fi
if has data; then
  mkdir -p "$OUT/data"
  say "points ladder: $LADDER"
  cargo build --release --manifest-path syngle-spike/gen/Cargo.toml
  for n in $LADDER; do
    syngle-spike/gen/target/release/syngle-spike-gen --count "$n" --out "$OUT/data/points_$n"
    rm -f "$OUT/data/points_$n.geoparquet"
  done
  # The sample the release stages is written by the engine crate's build script (build/*/out/countries.geojson).
  sample="$(ls -t target/*/release/build/*/out/countries.geojson target/release/build/*/out/countries.geojson 2>/dev/null | head -1 || true)"
  if [ -n "$sample" ]; then cp "$sample" "$OUT/data/countries.geojson"; else say "no countries.geojson yet (run the engine step first)"; fi
fi
{
  printf '{"commit":"%s","dirty":%s,"builtAt":"%s","rustc":"%s","steps":"%s"}\n' \
    "$(git rev-parse --short HEAD 2>/dev/null || echo unknown)" \
    "$([ -n "$(git status --porcelain 2>/dev/null)" ] && echo true || echo false)" \
    "$(date -u +%FT%TZ)" "$(rustc --version)" "$STEPS"
} >"$OUT/build.json"
say "done: $OUT"
