# evals: read-only evaluators for the map hill climb

Spec: `agents/docs/specs/map-hill-climb-and-gov-data.md` in the Syngle-Ecosystem workspace (sections 1 and 3). Node only (22 or newer), no packages. Nothing here changes the product.

| Folder | What |
|---|---|
| `build/` | `boat.sh` builds what the evaluators run (canvas, sidecar, `syngle-map.exe`, the point files) on a boat box and brings it back; `on-box.sh` is what runs on the box |
| `lib/` | CDP client, Chrome launcher and path probe, sidecar launcher (`stop()` answers when the process has exited; `engine.test.mjs`), `each-process.mjs` (one node process per dataset; `each-process.test.mjs`), statistics and the keep rule |
| `map-perf/` | `canvas.mjs`, `max-points.mjs`, `engine.mjs`, `features.mjs` (the gate); `frames.test.mjs` tests the frame and latency maths on synthetic frame series and `session.test.mjs` what the page-side recorder counts as an input (`node --test evals/map-perf/*.test.mjs`) |
| `data-sources/` | `check.mjs` and `catalogue.json` |
| `gov-data/` | `lookup.mjs`, its fixtures (`fixtures/`, made by `make.mjs`), `gen-scale.mjs`, `reference/` (a naive implementation that passes it) and `FROZEN.sha256` |
| `climb/` | `run.mjs` (n runs, noise, the keep rule), `baseline.mjs`, `report.mjs`, `scores.mjs` (every score and its bar) |
| `baselines/` | the measured start of each group (5 runs, with the noise) |

## Build, then measure

```sh
# 1. Build on your own boat box (never cargo on the PC or in WSL). One start; the box is reused.
evals/build/boat.sh --box <yourbox> --steps "canvas spike engine data"
#    -> C:\dev\scratch\obs-evals\<yourbox>\build   (canvas\, canvas-spike\, syngle-observatory-engine.exe, syngle-map.exe, data\)

# 2. Measure on this PC (Windows node, Windows Chrome; through `heavy`, one Chrome at a time, closed after)
cd /mnt/c && heavy cmd.exe /c "cd /d C:\dev\syngle-observatory && node evals\climb\run.mjs --group canvas:webgpu-gpu --build C:\dev\scratch\obs-evals\<yourbox>\build --runs 5 --baseline evals\baselines\canvas-webgpu-gpu.json" 2>&1 | grep -v "UNC\|CMD.EXE"
node evals/climb/run.mjs --list                    # the groups and their scores
node evals/climb/report.mjs                        # the baselines as tables
```

Groups: `canvas:<path>`, `maxpoints:<path>` for the four paths `webgpu-gpu`, `webgl2-gpu`, `webgl2-cpu`, `webgpu-cpu`; `engine`; `datasources`; `gov`. A run whose path is not the path asked for (a software run that landed on the GPU) is refused with exit 3.

`run.mjs` exit codes: 0 ran (and, with `--baseline`, the step is kept), 1 a run failed, 2 usage, 3 path mismatch, 4 not kept (the median gain does not beat the larger spread, or a gate failed under `--gates-pass`).

The gov evaluator needs no Chrome and no PC: on a boat box, `node evals/gov-data/lookup.mjs [--kinds a,b] [--scale <dir>] -- <syngle-map>`.

## Rules for a climber

- The evaluators, the fixtures, the golden cases and `climb/**` are frozen. A step that edits them is not a step. For the gov lookup, `sha256sum -c evals/gov-data/FROZEN.sha256` must be clean.
- Keep a step only if the median of 5 runs beats the larger of the baseline's and the candidate's spread, and every gate passes: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the tests of what you touched, the bridge contract tests, and `node evals/map-perf/features.mjs` (map groups).
- Headless Chrome on a PC with a display is not a user's browser: the numbers order your steps, they do not promise a frame rate.
