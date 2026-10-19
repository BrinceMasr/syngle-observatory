@echo off
rem Spike A (#43): the measurement matrix, one batch at a time. Each batch is a fresh set of
rem Electron processes; `run.mjs` sets (and restores) the per-app GPU preference itself.
rem Runs after the detector fix: the first-data test now counts the layer's own colour, and every
rem mode loads the project first (a pan over an empty map is not a measurement).
rem Usage (from this directory):  matrix.cmd
setlocal
cd /d %~dp0

echo === RTX 3080 Ti, WebGPU: cold open
node run.mjs --runs 5 --mode cold --tag rtx-webgpu2-cold --backend webgpu --gpu high --high-perf --outdir results
echo === RTX 3080 Ti, WebGPU: pan
node run.mjs --runs 3 --mode pan --tag rtx-webgpu2-pan --backend webgpu --gpu high --high-perf --pan-ms 10000 --outdir results

echo === RTX 3080 Ti, WebGL2: cold open
node run.mjs --runs 3 --mode cold --tag rtx-webgl2two-cold --backend webgl2 --gpu high --high-perf --outdir results
echo === RTX 3080 Ti, WebGL2: pan
node run.mjs --runs 2 --mode pan --tag rtx-webgl2two-pan --backend webgl2 --gpu high --high-perf --pan-ms 10000 --outdir results

echo === Intel UHD 770, WebGPU: cold open
node run.mjs --runs 3 --mode cold --tag uhd-webgpu2-cold --backend webgpu --gpu low --low-power --outdir results
echo === Intel UHD 770, WebGPU: pan
node run.mjs --runs 2 --mode pan --tag uhd-webgpu2-pan --backend webgpu --gpu low --low-power --pan-ms 10000 --outdir results

echo === Intel UHD 770, WebGL2: cold open
node run.mjs --runs 3 --mode cold --tag uhd-webgl2two-cold --backend webgl2 --gpu low --low-power --outdir results
echo === Intel UHD 770, WebGL2: pan
node run.mjs --runs 2 --mode pan --tag uhd-webgl2two-pan --backend webgl2 --gpu low --low-power --pan-ms 10000 --outdir results

echo === RTX 3080 Ti, WebGPU, GeoParquet instead of FlatGeobuf
node run.mjs --runs 3 --mode cold --tag rtx-parquet2-cold --backend webgpu --gpu high --high-perf --engine-data points_1m.geoparquet --outdir results
node run.mjs --runs 2 --mode pan --tag rtx-parquet2-pan --backend webgpu --gpu high --high-perf --pan-ms 10000 --engine-data points_1m.geoparquet --outdir results

echo === done
