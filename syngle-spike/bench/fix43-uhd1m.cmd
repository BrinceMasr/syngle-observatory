@echo off
rem Spike A (#43): the one missing cell - UHD 770, all 1M, GPU renderer. Bounded: run.mjs kills
rem any run that passes 180 s. The uncapped run reproduces the stall; the vsync run asks what a
rem user at 60 Hz would see.
setlocal
cd /d %~dp0
set COMMON=--mode pan --pan-ms 10000 --settle 1000 --settle-after 1000 --draws 1 --backend webgpu --engine-data points_1m.geoparquet --map-gpu --outdir results\fix43uhd --run-timeout 180 --warmup false
node run.mjs --runs 2 --tag fix3-pan-uhd-1m-draw-vsync --gpu low --low-power %COMMON%
node run.mjs --runs 1 --tag fix3-pan-uhd-1m-draw-uncapped --gpu low --low-power --uncapped %COMMON%
echo === uhd 1m done
