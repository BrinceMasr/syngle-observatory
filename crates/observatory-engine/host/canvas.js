// The engine's canvas-only host page (ADR 0010): the parent half of the two channels SyngleCode
// needs, with no chrome of its own.
//
//  * **The view's channel.** This page frames `/web/?embed`, so it *is* the view's parent: the
//    `{"rustgis": …}` messages the fork's embed contract posts arrive here. It relays them on to
//    SyngleCode as `{"observatory":"view", …}`, and pushes a project SyngleCode sends down to the
//    view as the contract's own `project` message.
//  * **The frame's own clock.** With `?spike` on this page's URL the view keeps its own per-frame
//    samples (`window.__spikeApp`, the fork's `?spike=1`), and the gateway injects the frame
//    instrument into the view's page (`window.__spike`). Both are same-origin here, so SyngleCode
//    asks for a snapshot and gets the view's own numbers rather than a proxy for them.
//  * **First data.** One message is posted the moment the view has really drawn a layer with
//    features in it — the "first frame with data" gate spike A (#43) settled on, which a loading
//    splash or an empty map cannot satisfy.
//
// SyngleCode is the page's host, whatever it is: every message is addressed to the parent window.
// This page never talks to the engine's control channel; that is SyngleCode's sidecar link.

(function () {
  var view = document.getElementById("view");
  var parent = window.parent;

  // Relay to the host: `observatory` names the kind, mirroring the fork's own `rustgis` contract.
  function tell(message) {
    try {
      if (parent && parent !== window) parent.postMessage(message, "*");
    } catch (e) {
      // A host that went away needs no answer.
    }
  }

  function toView(message) {
    try {
      if (view && view.contentWindow) view.contentWindow.postMessage(message, "*");
    } catch (e) {
      // The frame is between documents; the host can ask again.
    }
  }

  var firstData = null;
  var lastSample = null;
  var lastSeen = 0;

  // The view's own per-frame samples arrive through `window.__spikeApp` (the fork's `?spike=1`).
  // Reading them is the measurement: the view's frame timestamps, its real re-renders, its own
  // render milliseconds and the features its visible layers hold.
  function poll() {
    var win = view && view.contentWindow;
    if (win) {
      var app = win.__spikeApp;
      var frames = app && app.frames;
      if (frames && frames.length > lastSeen) {
        for (var i = lastSeen; i < frames.length; i++) {
          var row = frames[i];
          lastSample = row;
          if (!firstData && row[1] >= 1 && row[4] > 0) {
            firstData = {
              ms: row[0],
              renders: row[1],
              renderMs: row[3],
              features: row[4],
              windowed: row[6],
            };
            tell({ observatory: "first-data", at: firstData });
          }
        }
        lastSeen = frames.length;
      }
    }
    window.setTimeout(poll, 100);
  }

  // The frame instrument: rAF timestamps and GPU work packets for the view's own page.
  function instrument() {
    var win = view && view.contentWindow;
    var spike = win && win.__spike;
    if (!spike) return null;
    return { backend: spike.backend, frames: spike.frames.slice(-20000), draws: spike.draws };
  }

  // The view's samples, as the view wrote them: one row per frame.
  function appFrames() {
    var win = view && view.contentWindow;
    var app = win && win.__spikeApp;
    return app && app.frames ? app.frames.slice(-20000) : [];
  }

  // The project the engine currently holds, fetched from the gateway (never from disk here).
  function projectFromEngine(done) {
    var url = new URL("project.json", window.location.href);
    fetch(url.href, { cache: "no-store" })
      .then(function (r) {
        return r.text();
      })
      .then(function (text) {
        tell({ observatory: "engine-project", text: text });
        done(text);
      })
      .catch(function (e) {
        tell({ observatory: "error", error: "project.json: " + e });
      });
  }

  // A synthetic pointer or wheel event into the view's canvas: the same DOM path a real mouse
  // takes, for a harness that has to drive the map without a hand on the mouse.
  function drive(message) {
    var win = view && view.contentWindow;
    var doc = win && win.document;
    var canvas = doc && doc.getElementById("rustgis_canvas");
    if (!canvas) return tell({ observatory: "error", error: "no view canvas yet" });
    var at = function (type, x, y, extra) {
      var init = { bubbles: true, cancelable: true, clientX: x, clientY: y, button: 1, buttons: 4 };
      for (var k in extra) init[k] = extra[k];
      // Pointer events, which is what the browser-backed egui listens to.
      canvas.dispatchEvent(new win.PointerEvent(type, init));
      var mouse = { pointerdown: "mousedown", pointermove: "mousemove", pointerup: "mouseup" };
      if (mouse[type]) canvas.dispatchEvent(new win.MouseEvent(mouse[type], init));
    };
    if (message.kind === "wheel") {
      for (var i = 0; i < (message.steps || 1); i++) {
        canvas.dispatchEvent(
          new win.WheelEvent("wheel", {
            bubbles: true,
            cancelable: true,
            clientX: message.x || canvas.clientWidth / 2,
            clientY: message.y || canvas.clientHeight / 2,
            deltaY: message.deltaY || -120,
            deltaMode: 0,
          }),
        );
      }
      return tell({ observatory: "drove", kind: "wheel" });
    }
    if (message.kind === "drag") {
      var fromX = message.fromX || 0;
      var fromY = message.fromY || 0;
      var toX = message.toX || 0;
      var toY = message.toY || 0;
      var steps = message.steps || 12;
      at("pointerdown", fromX, fromY);
      for (var s = 1; s <= steps; s++) {
        at("pointermove", fromX + ((toX - fromX) * s) / steps, fromY + ((toY - fromY) * s) / steps);
      }
      at("pointerup", toX, toY, { buttons: 0 });
      return tell({ observatory: "drove", kind: "drag" });
    }
    tell({ observatory: "error", error: "unknown input " + message.kind });
  }

  window.addEventListener("message", function (event) {
    var data = event.data;
    if (!data || typeof data !== "object") return;
    // The view's own contract, on its way to whoever framed us (only the view's messages).
    if (data.rustgis && event.source === (view && view.contentWindow)) {
      return tell({ observatory: "view", message: data });
    }
    if (!data.observatory) return;
    switch (data.observatory) {
      case "hello":
        return tell({ observatory: "ready", href: window.location.href, view: view && view.src });
      case "project":
        // The host's own copy of the project (it is the side that opened the dataset).
        return toView({ rustgis: "project", text: data.text });
      case "refresh-project":
        return projectFromEngine(function (text) {
          toView({ rustgis: "project", text: text });
        });
      case "snapshot":
        return tell({
          observatory: "snapshot",
          firstData: firstData,
          hasView: Boolean(view && view.contentWindow),
          app: appFrames(),
          instrument: instrument(),
          queue: lastSeen,
        });
      case "input":
        return drive(data);
      default:
        return tell({ observatory: "error", error: "unknown request " + data.observatory });
    }
  });

  // The view's iframe address carries `?project=1` (see the engine's `view_embed`), so the view
  // fetches the project document itself; `refresh-project` is what re-pushes it after an open.
  tell({ observatory: "ready", href: window.location.href });
  poll();
})();
