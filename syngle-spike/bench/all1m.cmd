@echo off
rem Spike A (#43): the "all 1M drawn" pan runs, against the cap-lifted build (dist/web).
rem Each run loads the 1M-point GeoParquet in full, waits for it, then drags for 10 s.
setlocal
cd /d %~dp0
set ELECTRON=..\host\node_modules\electron\dist\electron.exe
set ENGINE=..\..\target\release\syngle-spike-engine.exe
set ROOT=..\..

node gpu-pref.mjs high
%ELECTRON% ..\host --engine %ENGINE% --root %ROOT% --mode pan --pan-ms 10000 --settle 8000 --poll 20 --backend webgpu --engine-data points_1m.geoparquet --high-perf --tag all1m-rtx-webgpu-run1 --out results\all1m-rtx-webgpu-run1.json
%ELECTRON% ..\host --engine %ENGINE% --root %ROOT% --mode pan --pan-ms 10000 --settle 8000 --poll 20 --backend webgpu --engine-data points_1m.geoparquet --high-perf --tag all1m-rtx-webgpu-run2 --out results\all1m-rtx-webgpu-run2.json
%ELECTRON% ..\host --engine %ENGINE% --root %ROOT% --mode pan --pan-ms 10000 --settle 8000 --poll 20 --backend webgpu --engine-data points_1m.geoparquet --high-perf --map-gpu --tag all1m-rtx-webgpu-mapgpu-run1 --out results\all1m-rtx-webgpu-mapgpu-run1.json

node gpu-pref.mjs low
%ELECTRON% ..\host --engine %ENGINE% --root %ROOT% --mode pan --pan-ms 10000 --settle 8000 --poll 20 --backend webgpu --engine-data points_1m.geoparquet --low-power --tag all1m-uhd-webgpu-run1 --out results\all1m-uhd-webgpu-run1.json
%ELECTRON% ..\host --engine %ENGINE% --root %ROOT% --mode pan --pan-ms 10000 --settle 8000 --poll 20 --backend webgpu --engine-data points_1m.geoparquet --low-power --tag all1m-uhd-webgpu-run2 --out results\all1m-uhd-webgpu-run2.json
node gpu-pref.mjs restore
echo === all1m done
