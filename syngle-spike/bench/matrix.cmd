@echo off
rem Spike A (#43): the measurement matrix, one batch at a time. Each batch is a fresh set of
rem Electron processes; `run.mjs` sets (and restores) the per-app GPU preference itself.
rem Usage (from this directory):  matrix.cmd
setlocal
cd /d %~dp0

echo === RTX 3080 Ti, WebGPU: cold open
node run.mjs --runs 5 --mode cold --tag rtx-webgpu-cold --backend webgpu --gpu high --high-perf --outdir results
echo === RTX 3080 Ti, WebGPU: pan
node run.mjs --runs 3 --mode pan --tag rtx-webgpu-pan --backend webgpu --gpu high --high-perf --pan-ms 10000 --outdir results

echo === RTX 3080 Ti, WebGL2: cold open
node run.mjs --runs 3 --mode cold --tag rtx-webgl2-cold --backend webgl2 --gpu high --high-perf --outdir results
echo === RTX 3080 Ti, WebGL2: pan
node run.mjs --runs 2 --mode pan --tag rtx-webgl2-pan --backend webgl2 --gpu high --high-perf --pan-ms 10000 --outdir results

echo === Intel UHD 770, WebGPU: cold open
node run.mjs --runs 5 --mode cold --tag uhd-webgpu-cold --backend webgpu --gpu low --low-power --outdir results
echo === Intel UHD 770, WebGPU: pan
node run.mjs --runs 3 --mode pan --tag uhd-webgpu-pan --backend webgpu --gpu low --low-power --pan-ms 10000 --outdir results

echo === Intel UHD 770, WebGL2: cold open
node run.mjs --runs 3 --mode cold --tag uhd-webgl2-cold --backend webgl2 --gpu low --low-power --outdir results
echo === Intel UHD 770, WebGL2: pan
node run.mjs --runs 2 --mode pan --tag uhd-webgl2-pan --backend webgl2 --gpu low --low-power --pan-ms 10000 --outdir results

echo === RTX 3080 Ti, WebGPU, GeoParquet instead of FlatGeobuf
node run.mjs --runs 3 --mode cold --tag rtx-geoparquet-cold --backend webgpu --gpu high --high-perf --engine-data points_1m.geoparquet --outdir results
node run.mjs --runs 2 --mode pan --tag rtx-geoparquet-pan --backend webgpu --gpu high --high-perf --pan-ms 10000 --engine-data points_1m.geoparquet --outdir results

echo === done
