// Shows what each rendering path really gets on this machine: the WebGPU adapter and the WebGL2 renderer. Read-only.
//
//   node evals/lib/gpu-probe.mjs [webgpu-gpu webgl2-gpu webgl2-cpu webgpu-cpu]
//
// One Chrome at a time, closed after each.
import http from "node:http";
import { launchChrome, PATHS, PROBE_EXPRESSION } from "./chrome.mjs";

// WebGPU needs a secure context: http://127.0.0.1 is one, a data: address is not.
const server = http.createServer((_, res) => (res.setHeader("content-type", "text/html"), res.end("<canvas></canvas>")));
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const url = `http://127.0.0.1:${server.address().port}/`;

const names = process.argv.slice(2).length ? process.argv.slice(2) : Object.keys(PATHS);
for (const pathName of names) {
  const chrome = await launchChrome({ pathName });
  try {
    await chrome.send("Page.enable");
    await chrome.send("Page.navigate", { url });
    await new Promise((resolve) => setTimeout(resolve, 800));
    const { result } = await chrome.send("Runtime.evaluate", { expression: PROBE_EXPRESSION, awaitPromise: true, returnByValue: true });
    console.log(JSON.stringify({ path: pathName, chrome: chrome.version.product, probe: result.value }));
  } finally {
    chrome.close();
  }
}
server.close();
