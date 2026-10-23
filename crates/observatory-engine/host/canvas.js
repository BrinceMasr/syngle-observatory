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

  // The host's origin, learned from its first message (see the listener below). A reply goes to
  // that origin and nowhere else; only the two messages that open the conversation (this page's
  // `ready` and the bridge's `hello`) are posted to `*`, since no host has spoken yet.
  var hostOrigin = null;

  // Relay to the host: `observatory` names the kind, mirroring the fork's own `rustgis` contract.
  function tell(message) {
    try {
      if (parent && parent !== window) parent.postMessage(message, hostOrigin === null ? "*" : hostOrigin);
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

  // --- the bridge (Syngle-Ecosystem #67, ADR 0010) --------------------------------------------
  //
  // The typed, versioned protocol between SyngleCode's React surface, this page and the engine.
  // One schema holds all of it: `bridge/observatory-bridge.schema.json` in this crate, checked by
  // the Rust contract tests and copied (and checked) into SyngleCode. This page is the view peer:
  // it owns the view half of the state (layers, camera, tool, selection, theme) and it drives the
  // view only through the fork's own embed contract — a project document pushed down, or one of the
  // app's `ui.*` commands run. A version it does not speak is refused, and then nothing is served.
  //
  // The tool's own map behaviour is #69's: the bridge carries which of the eight tools is active so
  // React and the view agree on it, and the view keeps using its own pointer tools until then.
  var BRIDGE = 1;
  // The members each view command takes: exactly the schema's `properties` for that command, so a
  // member the schema does not name is refused here too, before anything is applied. The node
  // contract test drives every command with an extra member and expects this to refuse it.
  var VIEW_ARGS = {
    layers: ["layers", "active", "reload"],
    style: ["layer", "style"],
    camera: ["camera"],
    tool: ["tool"],
    selection: ["layer", "rows"],
    theme: ["theme"],
    "render-to-png": ["width", "height"]
  };
  var bridge = {
    version: BRIDGE,
    linked: false,
    broken: false,
    project: null,
    state: { layers: [], active: null, camera: null, tool: "explore", selection: [], theme: "light" }
  };

  function bridgeTell(kind, extra) {
    var message = { observatory: "bridge", bridge: kind, protocolVersion: BRIDGE };
    for (var k in extra) message[k] = extra[k];
    tell(message);
  }

  function bridgeError(id, code, message) {
    bridgeTell("error", { id: id === undefined ? null : id, payload: { code: code, message: message } });
  }

  function activeMap() {
    if (!bridge.project || !bridge.project.maps) return null;
    return bridge.project.maps[bridge.project.active_map || 0] || null;
  }

  // The camera the bridge speaks: centre as [x, y] metres and a resolution, which is what React
  // needs and what the schema names.
  function bridgeCamera(camera) {
    if (!camera || !camera.center) return null;
    return { center: [camera.center.x, camera.center.y], res: camera.res };
  }

  function bridgeSnap() {
    var map = activeMap();
    var layers = map && map.layers ? map.layers : [];
    // `style` is the layer's renderer, as the view holds it: the surface restyles the layer it
    // knows (#71) without guessing whether it draws one symbol or a range of classes.
    bridge.state.layers = layers.map(function (l) {
      return { name: l.name, visible: l.visible !== false, style: l.renderer || null };
    });
    bridge.state.camera = map ? bridgeCamera(map.camera) : bridge.state.camera;
    var selected = null;
    for (var i = 0; i < layers.length; i++) {
      if (layers[i].selection && layers[i].selection.length) selected = { layer: layers[i].name, rows: layers[i].selection };
    }
    bridge.state.selection = selected ? [selected] : [];
    bridge.state.protocolVersion = BRIDGE;
    return {
      protocolVersion: BRIDGE,
      layers: bridge.state.layers,
      active: bridge.state.active,
      camera: bridge.state.camera,
      tool: bridge.state.tool,
      selection: bridge.state.selection,
      theme: bridge.state.theme
    };
  }

  // The project document, as the view takes it: the fork's own contract, whole. `id` is the host's
  // request, so its promise settles on this answer and not on some later event.
  function bridgeApply(id) {
    if (bridge.project) toView({ rustgis: "project", text: JSON.stringify(bridge.project) });
    bridgeSnap();
    bridgeTell("state", { id: id === undefined ? null : id, state: bridge.state });
  }

  function bridgeLayer(name) {
    var map = activeMap();
    if (!map || !map.layers) return null;
    for (var i = 0; i < map.layers.length; i++) if (map.layers[i].name === name) return map.layers[i];
    return null;
  }

  function runUi(command, params) {
    toView({ rustgis: "run", id: "bridge", command: command, params: params || {} });
  }

  // Base64 without a dependency, in pieces: a map PNG is megabytes and one spread would overflow.
  function base64(bytes) {
    var text = "";
    for (var i = 0; i < bytes.length; i += 4096) {
      text += String.fromCharCode.apply(null, bytes.subarray(i, i + 4096));
    }
    return window.btoa(text);
  }

  // One command from the host. Every member is checked against the schema's own list before
  // anything is applied: a command the schema does not name, or a member it does not have, is
  // refused by name rather than half-applied.
  function bridgeCommand(data) {
    var id = data.id === undefined ? null : data.id;
    if (data.protocolVersion !== BRIDGE || bridge.broken) {
      // Loud, and final: a peer on another version is served nothing after this, not even the next
      // command it sends, until the page is loaded again. The host shows the error state; the page
      // never silently half-works.
      bridge.broken = true;
      bridge.linked = false;
      bridgeTell("hello", { state: bridgeSnap() });
      bridgeError(id, "version-mismatch", "this page speaks protocol version " + BRIDGE + "; the host asked for " + data.protocolVersion);
      return;
    }
    bridge.linked = true;
    var command = data.command || {};
    var name = command.name;
    var allowed = VIEW_ARGS[name];
    if (!allowed) return bridgeError(id, "unknown-command", "this page has no view command '" + name + "'");
    for (var key in command) {
      if (key !== "name" && allowed.indexOf(key) === -1) {
        return bridgeError(id, "bad-params", "'" + key + "' is not part of the '" + name + "' command");
      }
    }
    switch (name) {
      case "camera": {
        var map = activeMap();
        var wanted = command.camera;
        if (!map || !wanted || !wanted.center || wanted.center.length !== 2) {
          return bridgeError(id, "bad-params", "camera needs {center: [x, y], res}");
        }
        map.camera = map.camera || {};
        map.camera.center = { x: wanted.center[0], y: wanted.center[1] };
        map.camera.res = wanted.res;
        bridgeApply(id);
        bridgeTell("event", { payload: { type: "camera-changed", camera: bridgeCamera(map.camera) } });
        return;
      }
      case "layers": {
        if (command.reload === true) {
          // After a command that changed the project, the engine's own document is the authority.
          return fetch(new URL("../project.json", window.location.href).href, { cache: "no-store" })
            .then(function (r) {
              return r.text();
            })
            .then(function (text) {
              bridge.project = JSON.parse(text);
              return bridgeApply(id);
            })
            .catch(function (e) {
              bridgeError(id, "engine-error", "the project could not be read: " + e);
            });
        }
        var map = activeMap();
        if (!map || !Array.isArray(command.layers)) return bridgeError(id, "bad-params", "layers needs an array");
        var byName = {};
        (map.layers || []).forEach(function (l) {
          byName[l.name] = l;
        });
        var ordered = [];
        for (var i = 0; i < command.layers.length; i++) {
          var entry = command.layers[i];
          var layerName = typeof entry === "string" ? entry : entry && entry.name;
          var layer = byName[layerName];
          if (!layer) return bridgeError(id, "bad-params", "there is no layer '" + layerName + "'");
          if (entry && typeof entry === "object" && typeof entry.visible === "boolean") layer.visible = entry.visible;
          ordered.push(layer);
        }
        // Whatever the host did not name keeps its place behind them: the bridge reorders, it does
        // not drop.
        (map.layers || []).forEach(function (l) {
          if (ordered.indexOf(l) === -1) ordered.push(l);
        });
        map.layers = ordered;
        if (command.active !== undefined) bridge.state.active = command.active;
        bridgeApply(id);
        return;
      }
      case "style": {
        var layer = bridgeLayer(command.layer);
        if (!layer) return bridgeError(id, "bad-params", "there is no layer '" + command.layer + "'");
        layer.renderer = command.style;
        bridgeApply(id);
        return;
      }
      case "selection": {
        var target = bridgeLayer(command.layer);
        if (!target) return bridgeError(id, "bad-params", "there is no layer '" + command.layer + "'");
        // A selection is one scope (ADR 0009): setting it replaces any other layer's, so the state
        // reports exactly the rows the agent and the user are working on.
        var mapForSelection = activeMap();
        (mapForSelection && mapForSelection.layers ? mapForSelection.layers : []).forEach(function (l) {
          if (l !== target) l.selection = [];
        });
        target.selection = Array.isArray(command.rows) ? command.rows : [];
        bridgeApply(id);
        bridgeTell("event", { payload: { type: "selection-changed", layer: command.layer, rows: target.selection } });
        return;
      }
      case "theme": {
        bridge.state.theme = command.theme;
        runUi("ui.theme", { dark: command.theme === "dark" });
        bridgeApply(id);
        return;
      }
      case "tool": {
        bridge.state.tool = command.tool;
        bridgeApply(id);
        return;
      }
      case "render-to-png": {
        var query = "?width=" + (command.width || 1024) + "&height=" + (command.height || 640);
        return fetch(new URL("../render" + query, window.location.href).href, { cache: "no-store" })
          .then(function (r) {
            if (!r.ok) throw new Error("the engine answered " + r.status);
            return r.arrayBuffer();
          })
          .then(function (buffer) {
            var bytes = new Uint8Array(buffer);
            bridgeTell("state", {
              id: id,
              state: bridgeSnap(),
              payload: { type: "render-to-png", png: base64(bytes), bytes: bytes.length }
            });
          })
          .catch(function (e) {
            bridgeError(id, "engine-error", "the map could not be drawn: " + e);
          });
      }
      default:
        return bridgeError(id, "unknown-command", "this page has no view command '" + name + "'");

    }
  }

  // The view's own contract, on its way up. A project the view changed itself (a pan, a click) is
  // where `camera-changed` and `selection-changed` really come from, so the host learns what the
  // hand did, not what the host asked for.
  function bridgeFromView(message) {
    if (message.rustgis !== "project" || typeof message.text !== "string") return;
    var next;
    try {
      next = JSON.parse(message.text);
    } catch (e) {
      return bridgeTell("event", { payload: { type: "frame-stats", features: 0, backend: null } });
    }
    var before = bridge.project;
    bridge.project = next;
    var map = activeMap();
    var was = before && before.maps ? before.maps[before.active_map || 0] : null;
    if (map && was && JSON.stringify(bridgeCamera(was.camera)) !== JSON.stringify(bridgeCamera(map.camera))) {
      bridgeTell("event", { payload: { type: "camera-changed", camera: bridgeCamera(map.camera) } });
    }
    var selected = bridgeSnap().selection;
    var wasSelected = was && was.layers ? was.layers.filter(function (l) { return l.selection && l.selection.length; }) : [];
    var nowSelected = (map && map.layers ? map.layers : []).filter(function (l) { return l.selection && l.selection.length; });
    if (JSON.stringify(wasSelected.map(function (l) { return [l.name, l.selection.length]; })) !==
        JSON.stringify(nowSelected.map(function (l) { return [l.name, l.selection.length]; }))) {
      var top = nowSelected[0];
      bridgeTell("event", { payload: { type: "selection-changed", layer: top ? top.name : "", rows: top ? top.selection : [] } });
    }
    bridgeTell("state", { id: null, state: bridgeSnap() });
  }

  // The view's own per-frame samples arrive through `window.__spikeApp` (the fork's `?spike=1`).
  // Reading them is the measurement: the view's frame timestamps, its real re-renders, its own
  // render milliseconds and the features its visible layers hold.
  var lastStats = 0;
  var devSamples = /(?:^|[?&])spike(?:=|&|$)/.test(window.location.href);

  // The bridge's `frame-stats`, once a second, from the same samples the strip used to be handed: a
  // development page adds the raw arrays, a shipped one the four counts.
  function stats(now) {
    var app = (view && view.contentWindow && view.contentWindow.__spikeApp) || null;
    var spike = (view && view.contentWindow && view.contentWindow.__spike) || null;
    var row = app && app.frames && app.frames.length ? app.frames[app.frames.length - 1] : null;
    if (!row) {
      // A shipped page has no `__spikeApp` samples — those need `?spike=1`, which a dev build asks
      // for and a shipped one never does — but the gateway injects the frame instrument into the
      // view in every build. A frame "drew" when the instrument's `draws` rose, so that counter is
      // the proof SyngleCode's "Map engine ready" cover waits for; a rAF tick alone is not a draw,
      // and an event sent before one would uncover a blank map (#95).
      if (!spike || !(spike.draws > 0) || !bridge.project || !bridge.project.maps) return;
      var visible = activeMap() && activeMap().layers ? activeMap().layers : [];
      var features = 0;
      for (var i = 0; i < visible.length; i++) {
        var layer = visible[i];
        if (layer.visible === false) continue;
        var dataset = layer.dataset;
        if (dataset && dataset.features && typeof dataset.features.length === "number") {
          features += dataset.features.length;
        }
      }
      bridgeTell("event", {
        payload: {
          type: "frame-stats",
          features: features,
          redraws: spike.draws,
          // A shipped page does not sample per-frame render milliseconds (`?spike` does), so this
          // is null rather than a guess; the schema allows null for exactly this case.
          renderMs: null,
          backend: spike.backend || null,
          at: now
        }
      });
      return;
    }
    var payload = {
      type: "frame-stats",
      features: row[4],
      redraws: row[1],
      renderMs: row[3],
      backend: spike ? spike.backend : null,
      at: now
    };
    if (devSamples) {
      payload.frames = app.frames.slice(-20000);
      if (spike && spike.frames) payload.instrument = spike.frames.slice(-20000);
    }
    bridgeTell("event", { payload: payload });
  }

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
    var now = Date.now();
    if (now - lastStats >= 1000) {
      lastStats = now;
      stats(now);
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

  // The project the engine currently holds, fetched from the gateway (never from disk here). The
  // document sits beside the launch token's path, not under the canvas page: this page is
  // `/t/<token>/canvas/`, and `../project.json` is `/t/<token>/project.json` (or `/project.json`
  // when the launch has no token) — the address the gateway actually routes.
  function projectFromEngine(done) {
    var url = new URL("../project.json", window.location.href);
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

  // Only this page's parent may drive it. The launch token guards the address; it does not decide
  // who is allowed to speak, and a `project` message repaints the map. The host's origin is learned
  // from its first message and every later one has to match it, so a page that is framed elsewhere
  // and then navigated to the gateway's port cannot take the channel over. `hostOrigin` (declared
  // by `tell`, above) is also the address every reply after that first message goes to.

  window.addEventListener("message", function (event) {
    var data = event.data;
    if (!data || typeof data !== "object") return;
    // The view's own contract, on its way to whoever framed us (only the view's messages).
    if (data.rustgis) {
      if (event.source === (view && view.contentWindow)) {
        tell({ observatory: "view", message: data });
        bridgeFromView(data);
      }
      return;
    }
    if (!data.observatory) return;
    if (event.source !== window.parent) return;
    if (hostOrigin === null) {
      hostOrigin = event.origin;
    } else if (event.origin !== hostOrigin) {
      return;
    }
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
      case "bridge":
        return bridgeCommand(data);
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

  // --- keys inside the view (Syngle-Ecosystem #111: C-01, C-05) -----------------------------------
  //
  // The view's canvas takes the keyboard, and a framed page's keys never reach its host: Tab and
  // Escape would leave the person stuck in the map, and a one-key shortcut typed there would do
  // nothing. This page shares the view's origin, so it listens on the view's own document — capture
  // phase, and never stopping the key (the view still sees it: an Escape still cancels a measure) —
  // and tells the host over the bridge what the keyboard meant: `focus-exit` for Tab, Shift+Tab and
  // Escape, `key` for one bare letter. Where focus goes, and what a letter does, is the host's call.
  // A host that does not know either event ignores it; both are optional in the schema.
  function viewKey(event) {
    if (event.isComposing) return;
    var key = event.key;
    var chord = event.ctrlKey || event.altKey || event.metaKey;
    if (key === "Tab" && !chord) {
      // The canvas would swallow Tab and keep focus; the host moves it, so the page's own focus
      // must not also step somewhere inside the view.
      if (typeof event.preventDefault === "function") event.preventDefault();
      return bridgeTell("event", { payload: { type: "focus-exit", direction: event.shiftKey ? "previous" : "next" } });
    }
    if (key === "Escape" && !chord) {
      return bridgeTell("event", { payload: { type: "focus-exit", direction: "escape" } });
    }
    if (chord || event.repeat) return;
    if (typeof key === "string" && /^[A-Za-z]$/.test(key)) {
      bridgeTell("event", { payload: { type: "key", key: key } });
    }
  }

  // The view's document, once per document: the frame navigates (a reload, a restart), so this runs
  // again on every load and marks the documents it has already taken.
  function watchViewKeys() {
    var win = view && view.contentWindow;
    var doc = win && win.document;
    if (!doc || doc.__observatoryKeys || typeof doc.addEventListener !== "function") return;
    doc.__observatoryKeys = true;
    doc.addEventListener("keydown", viewKey, true);
    // The canvas has no name or role of its own: say what it is for a screen reader.
    var canvas = typeof doc.getElementById === "function" ? doc.getElementById("rustgis_canvas") : null;
    if (canvas && typeof canvas.setAttribute === "function" && !canvas.getAttribute("aria-label")) {
      canvas.setAttribute("role", "application");
      canvas.setAttribute("aria-label", "Map");
    }
  }
  if (view && typeof view.addEventListener === "function") view.addEventListener("load", watchViewKeys);
  watchViewKeys();

  // The view's iframe address carries `?project=1` (see the engine's `view_embed`), so the view
  // fetches the project document itself; `refresh-project` is what re-pushes it after an open.
  tell({ observatory: "ready", href: window.location.href });
  // The bridge's own hello: this page's version and its view state, before anyone asks. A host on
  // another version can tell straight away, and a host on this one has the state to render.
  bridgeTell("hello", { state: bridgeSnap() });

  // The host's own copy of the document. The view fetches it itself (`?project=1`) and never echoes
  // it back, and no bridge command has run on a fresh load, so without this the page knows nothing
  // about the project it is showing — and the shipped-page `frame-stats` that clears SyngleCode's
  // cover could never fire. A failure leaves the gate closed, which is the honest state (#95).
  projectFromEngine(function (text) {
    try {
      bridge.project = JSON.parse(text);
    } catch (e) {
      bridgeTell("event", { payload: { type: "frame-stats-error", error: "project.json: " + e } });
    }
  });

  poll();
})();
