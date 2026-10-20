# The Observatory bridge

The typed, versioned channel between SyngleCode's React surface, this repo's canvas-only view and
the engine sidecar (Syngle-Ecosystem ADR 0010, spec #64, ticket #67).

## One schema, one version

`crates/observatory-engine/bridge/observatory-bridge.schema.json` is the only place the protocol is
written down. It carries:

| Section | What it holds |
|---|---|
| `envelope` | the four wire shapes (host→view, view→host, host→engine, engine→host) |
| `view.commands` | `layers`, `style`, `camera`, `tool`, `selection`, `theme`, `render-to-png` |
| `view.state` | what the view reports: version, layers, active, camera, tool, selection, theme |
| `view.events` | `camera-changed`, `selection-changed`, `feature-clicked`, `tool-result`, `frame-stats` |
| `engine.commands` | `dataset.open`, `dataset.query`, `catchment`, `command`, `mcp` |
| `engine.state` | version, layer count, feature count, dataset names, revision, the fork's undo depth and the label of the step undo would take back |
| `engine.events` | `progress`, `result`, `error` |

`protocolVersion` is the one number every peer has to agree on. It is carried on **every** message,
not just the handshake, so a stale page or a stale sidecar is refused the moment it speaks.

## Transports

* **View half** — `window.postMessage` between SyngleCode's renderer and the engine's canvas page
  (`/canvas/`, served from this binary). The page relays down the fork's own embed contract
  (`{"rustgis": …}`): a project document pushed whole, or one of the app's `ui.*` commands run.
* **Engine half** — JSON lines on the sidecar's stdin and stdout. SyngleCode's desktop process owns
  the pipe and forwards lines for the renderer.

The **state after every command** travels twice: as the `result` event's own `state`, and as the
`bridge.state` line after it. The result is what SyngleCode's bridge reads the undo depth from when
the command's promise settles, so it can tell an edit from a read.

`render-to-png` is answered by the engine, not the page: the page asks the gateway's token-guarded
`/render` route, and the command loop draws the project offscreen with the fork's own renderer
(`cmd::share::map_png`), so the image is the map the project describes and cannot race an open or an
edit.

## The fork's history is the authority

`command` runs the fork's own registry, and some of those commands are edits: they push an undo
step. Others are not: `dataset.query` and `mcp` reads, the `.view()` commands (`project.sample`,
`project.open`, `edit.undo` itself), every `view.*` id. So the engine reports its own history in
`engine.state` — `undoDepth`, `undoLabel`, `redoDepth` — after every command, and SyngleCode records
a step of its own **only when the depth grew**. Undo and redo go back through `edit.undo` /
`edit.redo`, and the client checks that the depth moved before it accepts the step; a move that did
not happen rolls its cursor back. The two histories are then one history, and neither side can serve
a step the other cannot take back.

## Failing loudly

A peer whose `protocolVersion` is not this bridge's is refused with `error` /
`version-mismatch`, and is **not served afterwards**:

* the engine drops its link and answers `no-handshake` to any command until a `hello` agrees;
* the page stops answering commands at all until it is loaded again;
* SyngleCode's bridge goes into its error state, which the surface shows in the map's place.

Nothing is coercible, guessed or half-applied. A command name or member the schema does not have is
`unknown-command` / `bad-params`, from the page and the engine both.

## Tests

| Where | What it proves |
|---|---|
| `crates/observatory-engine/src/bridge.rs` (`cargo test -p syngle-observatory-engine`) | the code and the schema agree: every command parses from its schema entry, every error code is in the schema, a wrong version is refused |
| `crates/observatory-engine/tests/bridge_contract.rs` | a **real** engine process, driven on its stdin: the five commands answer with values, state and events; a version mismatch is refused; `dataset.open` reads a real file; `/render` draws a real PNG; an edit grows the reported undo depth, a read does not, and undo/redo move it both ways |
| `crates/observatory-engine/host/bridge.contract.test.mjs` (`node --test host/bridge.contract.test.mjs`) | the page SyngleCode frames: every view command is implemented, what it pushes down is the fork's contract, a pan comes back as `camera-changed`, members the schema does not name are refused, and replies are addressed to the host's own origin once it has spoken |

The JavaScript test uses Node's built-in test runner, so the fork takes on no test dependency for
the page half; `cargo xtask ci` stays Rust-only.

SyngleCode's half of the contract tests lives with its code:
`t3-fork/apps/web/src/syngle/observatory/bridge/observatoryBridge.test.ts`.
