@echo off
rem Spike A (#43) fix round: cold open, measured on the SHIPPED build (dist/web-stock, the 30k
rem remote view window that would ship) with the view's own gate: a real render after the project
rem arrived, with features in the layer. No colour threshold can fire on the loading splash.
setlocal
cd /d %~dp0
set STOCK=dist\web-stock

echo === RTX 3080 Ti, WebGPU, GeoParquet (shipped window)
node run.mjs --runs 3 --mode cold --tag fix-rtx-webgpu-parquet --engine-web %STOCK% --engine-data points_1m.geoparquet --backend webgpu --gpu high --high-perf --outdir results\fix43
echo === Intel UHD 770, WebGPU, GeoParquet (shipped window)
node run.mjs --runs 3 --mode cold --tag fix-uhd-webgpu-parquet --engine-web %STOCK% --engine-data points_1m.geoparquet --backend webgpu --gpu low --low-power --outdir results\fix43
echo === RTX 3080 Ti, WebGPU, FlatGeobuf (shipped window)
node run.mjs --runs 3 --mode cold --tag fix-rtx-webgpu-fgb --engine-web %STOCK% --backend webgpu --gpu high --high-perf --outdir results\fix43
echo === Intel UHD 770, WebGPU, FlatGeobuf (shipped window)
node run.mjs --runs 3 --mode cold --tag fix-uhd-webgpu-fgb --engine-web %STOCK% --backend webgpu --gpu low --low-power --outdir results\fix43
echo === RTX 3080 Ti, WebGL2, FlatGeobuf (shipped window)
node run.mjs --runs 3 --mode cold --tag fix-rtx-webgl2-fgb --engine-web %STOCK% --backend webgl2 --gpu high --high-perf --outdir results\fix43
echo === Intel UHD 770, WebGL2, FlatGeobuf (shipped window)
node run.mjs --runs 3 --mode cold --tag fix-uhd-webgl2-fgb --engine-web %STOCK% --backend webgl2 --gpu low --low-power --outdir results\fix43
echo === cold matrix done
