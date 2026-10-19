@echo off
rem Spike A (#43) fix round: pan, measured on real draw cost.
rem
rem Two rows per GPU, clearly different:
rem   perceived pan  - the default CPU renderer, vsync on, what a user sees: the view shifts a
rem                    cached texture while dragging and only re-renders when the texture no
rem                    longer covers the view. The run reports how many full renders happened.
rem   draw cost      - the fork's GPU point renderer (View > Toggle GPU Rendering) on an uncapped
rem                    frame loop: every frame redraws all point markers with the new camera,
rem                    so the frame cadence and the app's own render ms are real draw cost.
rem
rem --draws N turns the wheel N times after the drag: each is one full re-render at rest, the
rem number the cached-texture drag hides.
setlocal
cd /d %~dp0
set STOCK=dist\web-stock
set COMMON=--mode pan --pan-ms 10000 --settle 2000 --settle-after 2000 --draws 3 --backend webgpu --engine-data points_1m.geoparquet --outdir results\fix43

echo === perceived pan: RTX, shipped 30k window (default renderer, vsync)
node run.mjs --runs 2 --tag fix-pan-rtx-30k-perceived --engine-web %STOCK% --gpu high --high-perf %COMMON%
echo === perceived pan: UHD, shipped 30k window (default renderer, vsync)
node run.mjs --runs 2 --tag fix-pan-uhd-30k-perceived --engine-web %STOCK% --gpu low --low-power %COMMON%
echo === perceived pan: RTX, all 1M (default renderer, vsync)
node run.mjs --runs 2 --tag fix-pan-rtx-1m-perceived --gpu high --high-perf %COMMON%
echo === perceived pan: UHD, all 1M (default renderer, vsync)
node run.mjs --runs 2 --tag fix-pan-uhd-1m-perceived --gpu low --low-power %COMMON%

echo === draw cost: RTX, shipped 30k window (GPU renderer, uncapped)
node run.mjs --runs 2 --tag fix-pan-rtx-30k-draw --engine-web %STOCK% --gpu high --high-perf --map-gpu --uncapped %COMMON%
echo === draw cost: UHD, shipped 30k window (GPU renderer, uncapped)
node run.mjs --runs 2 --tag fix-pan-uhd-30k-draw --engine-web %STOCK% --gpu low --low-power --map-gpu --uncapped %COMMON%
echo === draw cost: RTX, all 1M (GPU renderer, uncapped)
node run.mjs --runs 2 --tag fix-pan-rtx-1m-draw --gpu high --high-perf --map-gpu --uncapped %COMMON%
echo === draw cost: UHD, all 1M (GPU renderer, uncapped)
node run.mjs --runs 2 --tag fix-pan-uhd-1m-draw --gpu low --low-power --map-gpu --uncapped %COMMON%
echo === pan matrix done
