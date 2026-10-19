// Spike A (#43): the frame-pacing instrument, served into the view's own page.
//
// It is injected by the engine's gateway into `/web/index.html` (see `main.rs`), so the fork's
// source is untouched: what is measured is the stock WASM build, plus two counters.
//
//   window.__spike.frames  — every `requestAnimationFrame` timestamp the app asked for
//   window.__spike.draws   — GPU work packets: WebGPU queue submissions, or WebGL2 draw calls
//
// A frame "drew" when `draws` rose between two of its callbacks. Frame time is the gap between
// consecutive rAF callbacks: the browser only calls them when it is ready to paint, so a build
// that misses the display's deadline shows up here as a wider gap, exactly as a user sees it.
(function () {
  var S = (window.__spike = {
    t: performance.now(),
    frames: [],
    draws: 0,
    backend: null,
    errors: [],
  });
  try {
    var raf = window.requestAnimationFrame.bind(window);
    window.requestAnimationFrame = function (cb) {
      return raf(function (t) {
        try {
          S.frames.push(t);
        } catch (e) {
          S.errors.push(String(e));
        }
        return cb(t);
      });
    };
  } catch (e) {
    S.errors.push("raf: " + e);
  }
  try {
    if (window.GPUQueue && window.GPUQueue.prototype && window.GPUQueue.prototype.submit) {
      S.backend = "webgpu";
      var submit = window.GPUQueue.prototype.submit;
      window.GPUQueue.prototype.submit = function () {
        S.draws++;
        return submit.apply(this, arguments);
      };
    }
  } catch (e) {
    S.errors.push("webgpu: " + e);
  }
  try {
    var proto = window.WebGL2RenderingContext && window.WebGL2RenderingContext.prototype;
    if (proto) {
      var names = ["drawArrays", "drawElements", "drawArraysInstanced", "drawElementsInstanced", "drawRangeElements"];
      for (var i = 0; i < names.length; i++) {
        (function (name) {
          var fn = proto[name];
          if (!fn || fn.__spikeWrapped) return;
          var wrapped = function () {
            S.draws++;
            return fn.apply(this, arguments);
          };
          wrapped.__spikeWrapped = true;
          proto[name] = wrapped;
        })(names[i]);
      }
      // Whichever context the app asked for first: a webgl2 context here means the WebGL2 path.
      var gl = document.createElement("canvas").getContext("webgl2");
      if (gl && !S.backend) {
        var info = gl.getExtension("WEBGL_debug_renderer_info");
        S.webglRenderer = info ? String(gl.getParameter(info.UNMASKED_RENDERER_WEBGL)) : String(gl.getParameter(gl.RENDERER));
      }
    }
  } catch (e) {
    S.errors.push("webgl: " + e);
  }
  S.reset = function () {
    S.frames.length = 0;
    S.draws = 0;
  };
  S.snapshot = function () {
    return { frames: S.frames.slice(), draws: S.draws, t: performance.now(), backend: S.backend, errors: S.errors.slice() };
  };
})();
