// Spike A (#43): the embedding page's driver.
//
// The host page holds the WASM view in an iframe and speaks the fork's documented `?embed`
// postMessage contract with it (see apps/rustgis-web/src/embed.rs): the page sends a whole project,
// the view posts `ready`, `project` (600 ms after a change settles) and `result`.
//
// Everything below is measurement scaffolding for the Electron main process, which drives this
// page with `webContents.executeJavaScript`: `spike.waitReady()`, `spike.sendProject(text)`,
// `spike.reset()`, `spike.snapshot()`, `spike.info()`.
(function () {
  var frame = document.getElementById("view");
  var hud = document.getElementById("hud");
  var state = {
    ready: null,
    readyAt: 0,
    sentAt: 0,
    posts: [],
    results: [],
    pageT0: performance.now(),
    pageDate0: Date.now(),
  };
  var waiters = [];

  window.addEventListener("message", function (event) {
    if (event.origin !== window.location.origin) return;
    var data = event.data;
    if (!data || typeof data !== "object" || typeof data.rustgis !== "string") return;
    var at = performance.now();
    var date = Date.now();
    if (data.rustgis === "ready") {
      state.ready = { at: at, date: date };
      state.readyAt = at;
      waiters.splice(0).forEach(function (resolve) {
        resolve({ at: at, date: date });
      });
    } else if (data.rustgis === "project") {
      var short = { at: at, date: date, bytes: (data.text || "").length };
      try {
        var project = JSON.parse(data.text);
        var layer = project.maps && project.maps[0] && project.maps[0].layers && project.maps[0].layers[0];
        short.layers = project.maps && project.maps[0] ? project.maps[0].layers.length : 0;
        short.features = layer && layer.dataset && layer.dataset.features ? layer.dataset.features.length : 0;
        short.source = layer && layer.source ? layer.source.path : null;
        short.res = project.maps && project.maps[0] ? project.maps[0].camera.res : null;
        short.center = project.maps && project.maps[0] ? [project.maps[0].camera.center.x, project.maps[0].camera.center.y] : null;
        short.revision = project.maps ? project.revision : null;
      } catch (e) {
        short.parseError = String(e);
      }
      state.posts.push(short);
      hud.textContent =
        "frames=" + (spike.snapshot().frames.length) + " posts=" + state.posts.length + " features=" + (short.features || 0);
    } else if (data.rustgis === "result") {
      state.results.push({ at: at, id: data.id, ok: data.ok });
    }
  });

  var spike = {
    state: state,
    frame: frame,

    /// Resolves when the view posted `ready` (it is listening for the page's messages).
    waitReady: function (timeoutMs) {
      if (state.ready) return Promise.resolve(state.ready);
      return new Promise(function (resolve, reject) {
        var timer = setTimeout(function () {
          reject(new Error("the view never posted ready"));
        }, timeoutMs || 60000);
        waiters.push(function (r) {
          clearTimeout(timer);
          resolve(r);
        });
      });
    },

    /// Sends a whole project over the documented contract; returns when the message is posted.
    sendProject: function (text) {
      frame.contentWindow.postMessage({ rustgis: "project", text: text }, window.location.origin);
      var at = performance.now();
      var date = Date.now();
      state.sentAt = at;
      return { at: at, date: date, bytes: text.length };
    },

    /// The longest the page waits for the *next* project post, so `waitForPost` can time out.
    waitForPost: function (afterIndex, timeoutMs) {
      var start = state.posts.length;
      var from = afterIndex === undefined ? start : afterIndex;
      return new Promise(function (resolve, reject) {
        var deadline = performance.now() + (timeoutMs || 30000);
        var tick = setInterval(function () {
          if (state.posts.length > from) {
            clearInterval(tick);
            resolve(state.posts[state.posts.length - 1]);
          } else if (performance.now() > deadline) {
            clearInterval(tick);
            reject(new Error("no project post arrived"));
          }
        }, 10);
      });
    },

    /// Runs one of the view's own commands (the `run` half of the embed contract) and resolves with
    /// its result. Used for `ui.map.gpu`, the fork's GPU point renderer.
    runUiCommand: function (command, params, timeoutMs) {
      var id = "spike-" + Date.now();
      var results = state.results;
      frame.contentWindow.postMessage({ rustgis: "run", id: id, command: command, params: params || {} }, window.location.origin);
      return new Promise(function (resolve, reject) {
        var deadline = performance.now() + (timeoutMs || 30000);
        var tick = setInterval(function () {
          var hit = null;
          for (var i = results.length - 1; i >= 0; i--) {
            if (results[i].id === id) {
              hit = results[i];
              break;
            }
          }
          if (hit) {
            clearInterval(tick);
            resolve(hit);
          } else if (performance.now() > deadline) {
            clearInterval(tick);
            reject(new Error("no result for " + command));
          }
        }, 20);
      });
    },

    /// Spike A (#43): the view's own per-frame samples, from the app itself (`?spike=1`):
    /// [t, real renders, GPU uniform repaints, last render ms, features held, visible layers,
    /// windowed layers, GPU point instances, resolution, centre x, centre y]. `null` without
    /// the flag: the stock page carries no samples.
    appFrames: function () {
      try {
        var w = frame.contentWindow;
        var app = w && w.__spikeApp;
        if (!app || !app.frames) return null;
        var out = [];
        for (var i = 0; i < app.frames.length; i++) {
          var r = app.frames[i];
          out.push(Array.prototype.slice.call(r));
        }
        return out;
      } catch (e) {
        return null;
      }
    },

    /// The newest sample as named fields, for gates and progress (cheap: one small array).
    appState: function () {
      try {
        var w = frame.contentWindow;
        var app = w && w.__spikeApp;
        if (!app || !app.frames || app.frames.length === 0) return null;
        var r = app.frames[app.frames.length - 1];
        return {
          count: app.frames.length,
          t: r[0],
          renders: r[1],
          fast: r[2],
          ms: r[3],
          features: r[4],
          visible: r[5],
          windowed: r[6],
          points: r[7],
          res: r[8],
          cx: r[9],
          cy: r[10],
        };
      } catch (e) {
        return null;
      }
    },

    /// Drops the samples (the injection lets one drag be measured on its own).
    resetApp: function () {
      try {
        var w = frame.contentWindow;
        if (w && w.__spikeApp && w.__spikeApp.frames) w.__spikeApp.frames.length = 0;
        return true;
      } catch (e) {
        return String(e);
      }
    },

    /// The view's own frame/draw counters (the injected instrument).
    snapshot: function () {
      try {
        var w = frame.contentWindow;
        if (w && w.__spike) return w.__spike.snapshot();
      } catch (e) {
        return { error: String(e) };
      }
      return { frames: [], draws: 0, error: "no instrument" };
    },

    reset: function () {
      try {
        var w = frame.contentWindow;
        if (w && w.__spike) w.__spike.reset();
        return true;
      } catch (e) {
        return String(e);
      }
    },

    /// What the renderer actually got: the WebGPU adapter or the WebGL2 renderer string, plus the
    /// backend the instrument saw the app use.
    info: async function () {
      var out = { href: window.location.href, ready: state.ready, posts: state.posts.length };
      // The cold-open breakdown, from the view's own page: navigation, the WASM fetch (a resource
      // entry with its start and end), and the first frame the app asked for. Same origin, so the
      // parent may read the child's performance timeline directly.
      try {
        var w = frame.contentWindow;
        out.timings = {
          now: Math.round(w.performance.now()),
          navigation: w.performance.getEntriesByType("navigation").map(function (e) {
            return { startTime: Math.round(e.startTime), responseEnd: Math.round(e.responseEnd), domContentLoaded: Math.round(e.domContentLoadedEventEnd), loadEventEnd: Math.round(e.loadEventEnd), transferSize: e.transferSize, encodedBodySize: e.encodedBodySize };
          }),
          resources: w.performance.getEntriesByType("resource").map(function (e) {
            return { name: String(e.name).split("/").pop(), startTime: Math.round(e.startTime), responseEnd: Math.round(e.responseEnd), duration: Math.round(e.duration), transferSize: e.transferSize, encodedBodySize: e.encodedBodySize, decodedBodySize: e.decodedBodySize };
          }),
          marks: w.performance.getEntriesByType("mark").map(function (e) {
            return { name: e.name, startTime: Math.round(e.startTime) };
          }),
          firstFrame: w.__spike && w.__spike.frames.length ? Math.round(w.__spike.frames[0]) : null,
          frameCount: w.__spike ? w.__spike.frames.length : null,
        };
      } catch (e) {
        out.perfError = String(e);
      }
      try {
        out.instrument = this.snapshot();
      } catch (e) {
        out.instrumentError = String(e);
      }
      try {
        if (navigator.gpu) {
          var adapter = await navigator.gpu.requestAdapter();
          // GPUAdapterInfo's fields live on the prototype, so they are read one by one.
          var info = adapter && adapter.info;
          out.webgpu = adapter
            ? {
                vendor: info && info.vendor,
                architecture: info && info.architecture,
                device: info && info.device,
                description: info && info.description,
                featureCount: adapter.features ? adapter.features.size : null,
                limits: adapter.limits ? { maxTextureDimension2D: adapter.limits.maxTextureDimension2D, maxBufferSize: adapter.limits.maxBufferSize } : null,
              }
            : null;
        } else {
          out.webgpu = "navigator.gpu missing";
        }
      } catch (e) {
        out.webgpuError = String(e);
      }
      try {
        var canvas = document.createElement("canvas");
        var gl = canvas.getContext("webgl2");
        if (gl) {
          var ext = gl.getExtension("WEBGL_debug_renderer_info");
          out.webgl2 = ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
        }
      } catch (e) {
        out.webglError = String(e);
      }
      // Which backend the app really took: a canvas keeps the first context type it is given, so
      // asking for `webgl2` answers "webgl2" only when the app itself created a WebGL2 context.
      try {
        var doc = frame.contentWindow.document;
        var appCanvas = doc.getElementById("rustgis_canvas") || doc.querySelector("canvas");
        if (appCanvas) {
          out.appCanvas = { width: appCanvas.width, height: appCanvas.height };
          var existing = appCanvas.getContext("webgl2");
          out.appCanvasContext = existing ? "webgl2" : "not webgl2 (WebGPU, or another context)";
          if (existing) {
            var info2 = existing.getExtension("WEBGL_debug_renderer_info");
            out.appCanvasRenderer = info2 ? existing.getParameter(info2.UNMASKED_RENDERER_WEBGL) : existing.getParameter(existing.RENDERER);
            out.appCanvasHasWebgpuMarker = typeof WebGPU !== "undefined" && appCanvas.__webgpu !== undefined;
          }
        } else {
          out.appCanvas = "none found";
        }
      } catch (e) {
        out.appCanvasError = String(e);
      }
      out.userAgent = navigator.userAgent;
      return out;
    },
  };
  window.spike = spike;
})();
