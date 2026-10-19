@echo off
rem Spike A (#43) fix round: the draw-cost pan, with the first GPU scene waited for before the
rem drag (the UHD 1M run stalled on the scene build when it was not).
setlocal
cd /d %~dp0
set STOCK=dist\web-stock
set COMMON=--mode pan --pan-ms 10000 --settle 1000 --settle-after 1500 --draws 3 --backend webgpu --engine-data points_1m.geoparquet --map-gpu --uncapped --outdir results\fix43gpu

echo === draw cost: RTX, shipped 30k window (GPU renderer, uncapped)
node run.mjs --runs 2 --tag fix2-pan-rtx-30k-draw --engine-web %STOCK% --gpu high --high-perf %COMMON%
echo === draw cost: UHD, shipped 30k window (GPU renderer, uncapped)
node run.mjs --runs 2 --tag fix2-pan-uhd-30k-draw --engine-web %STOCK% --gpu low --low-power %COMMON%
echo === draw cost: RTX, all 1M (GPU renderer, uncapped)
node run.mjs --runs 2 --tag fix2-pan-rtx-1m-draw --gpu high --high-perf %COMMON%
echo === draw cost: UHD, all 1M (GPU renderer, uncapped)
node run.mjs --runs 2 --tag fix2-pan-uhd-1m-draw --gpu low --low-power %COMMON%
echo === draw matrix done
