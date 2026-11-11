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
| `view.events` | `camera-changed`, `selection-changed`, `feature-clicked`, `tool-result`, `frame-stats`, `focus-exit`, `key`, `history` |
| `engine.commands` | `dataset.open`, `dataset.query`, `catchment`, `command`, `mcp` |
| `engine.state` | version, layer count, feature count, dataset names, revision, the fork's undo depth and the label of the step undo would take back |
| `engine.events` | `progress`, `result`, `error` |

`focus-exit` and `key` are the keyboard's way out of the map (Syngle-Ecosystem #111). The view's canvas
takes the keyboard, and a framed page's keys never reach its host, so the canvas page (same origin as the
view) watches the view's document and posts `focus-exit` (`next` for Tab, `previous` for Shift+Tab,
`escape` for Escape) and `key` (one bare letter, for the host's own one-key shortcuts). It never stops
the key: the view still sees it, so an Escape still cancels a measure. Both are optional emitters, which
is why they did not change `protocolVersion`: a host that does not know them ignores them, and an engine
that does not send them leaves the map as it was. A change a peer cannot ignore is what bumps the version.

`history` is the same kind of event for undo and redo (Syngle-Ecosystem #468): Ctrl+Z or Cmd+Z is `undo`, Ctrl+Shift+Z,
Cmd+Shift+Z and (not on a Mac) Ctrl+Y are `redo`. The page decides which modifier is the platform's, sends one event per
press (a held chord is not repeated, and a text field of the view keeps its own undo), and does not stop the key; what the
step does to the history is the host's call.

`protocolVersion` is the one number every peer has to agree on. It is carried on **every** message,
not just the handshake, so a stale page or a stale sidecar is refused the moment it speaks.

## Transports

* **View half** — `window.postMessage` between SyngleCode's renderer and the engine's canvas page
  (`/canvas/`, served from this binary). The page relays down the fork's own embed contract
  (`{"rustgis": …}`): a project document pushed whole, or one of the app's `ui.*` commands run.
* **Engine half** — JSON lines on the sidecar's stdin and stdout. SyngleCode's desktop process owns
  the pipe and forwards lines for the renderer. A host that does **not** own the pipe — the agent's
  map tools in a dev or headless run, a browser talking to a gateway directly — posts the same line
  to `POST /bridge` on the token-guarded gateway and gets back the lines the engine would have
  written, as a JSON array. It is the same dispatch (`bridge_lines` in `src/lib.rs`), the same
  session and the same history: one handshake per launch covers both, and a version refusal on
  either transport closes both. Even a body that is not JSON is refused in the bridge's own
  `bad-params` shape, never with a bare 400 page.

A `command` may name the map it acts on with `map`, an id or a name (ids are in `info.project`'s `mapList`).
The command runs on that map and the map in view stays as it was. A name that two maps share, or one that
names no map, is refused as `bad-params` with the list; a command that has a `map` of its own keeps its
meaning (Syngle-Ecosystem #470).

The **project is served in parts**, never as one document (Syngle-Ecosystem #471): `GET /project.json?shell` is the
project with no map's layers, `?map=<id>` (or none, or `?map=active`) is that map's layers with no rows, and each dataset's
`features` is a reference `{id, revision, count}` to its rows. `GET /dataset/<id>?revision=<n>` serves those rows as
FlatGeobuf in row order (`fgb_rows`), by byte range, with an `ETag` and a 304 for `If-None-Match`. A dataset's id is the
identity of its rows in memory, so maps that share rows share an id and the page fetches them once; a change to the rows
moves their revision. The state names `shellRevision`, `mapRevisions` `[id, rev]` and `datasetRevisions` `[id, rev]`. The
view (`rustgis_engine::rows_book`) keeps the rows it holds by id and fetches only a revision it has not got, showing the
older rows until the newer land.

The **state after every command** travels twice: as the `result` event's own `state`, and as the
`bridge.state` line after it. The result is what SyngleCode's bridge reads the undo depth from when
the command's promise settles, so it can tell an edit from a read.

`render-to-png` is answered by the engine, not the page: the page asks the gateway's token-guarded
`/render` route, and the command loop draws the project offscreen with the fork's own renderer
(`cmd::share::map_png`), so the image is the map the project describes and cannot race an open or an
edit.

## One script, one undo step

The engine's depth is what a *command* costs; what the user undoes is the app's step. A codemode
script is several commands, so SyngleCode's bridge opens an **agent batch** before its first command
and closes it after the last (`openAgentBatch` / `closeAgentBatch` in
`t3-fork/apps/web/src/syngle/observatory/bridge/observatoryBridge.ts`, ecosystem #76): while a batch
is open no command records a step of its own, and the close records one, labelled `Agent: …`, whose
undo takes back exactly the depth the script added (an `edit.undo` per step, through the fork's own
registry) and whose redo puts it back. A script that only read — a query, a render — records nothing,
because the depth did not move. The view commands a script ran are held in the same step and
replayed after the engine's, so one click undoes the whole script and nothing of the user's own work.

## The fork's history is the authority

`command` runs the fork's own registry, and some of those commands are edits: they push an undo
step. Others are not: `dataset.query` and `mcp` reads, the `.view()` commands (`project.sample`,
`project.open`, `edit.undo` itself), every `view.*` id. So the engine reports its own history in
`engine.state` — `undoDepth`, `undoLabel`, `redoDepth` — after every command, and SyngleCode records
a step of its own **only when the depth grew**. Undo and redo go back through `edit.undo` /
`edit.redo`, and the client checks that the depth moved before it accepts the step; a move that did
not happen rolls its cursor back. The two histories are then one history, and neither side can serve
a step the other cannot take back.

### Several maps (ADR 0022, Syngle #470 to #474)

The history is one stack **per map**: `undoDepth`, `undoLabel`, `redoDepth` and `redoLabel` are the map in view's, and
`mapUndo: [[id, undo, redo]]` has every map's depths. A change to the whole project sits on every map's stack, and a row edit
shared by several maps is one step labelled "..., also in B". `maps` describes each map for a page pinned to it (id, name,
layers, features, datasets, depths and labels), so a window that shows map B while A is in view reads B's own numbers.

Any `command` may carry `map` (an id, or a name): it runs on that map and the map in view stays. A line that is not a registered
command (`dataset.query`, `mcp`) takes `map` beside `name` and is run the same way; an unknown map is `bad-params`. The view
command `view-map {id}` re-targets a canvas page to a map without a new page or GL context, and a page opened with `?map=<id>`
shows that map from the start.

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
| `crates/observatory-engine/host/bridge.contract.test.mjs` (`node --test host/bridge.contract.test.mjs`) | the page SyngleCode frames: every view command is implemented, what it pushes down is the fork's contract, a pan comes back as `camera-changed`, Tab, Shift+Tab and Escape in the map come back as `focus-exit` and a bare letter as `key`, members the schema does not name are refused, and replies are addressed to the host's own origin once it has spoken |

The JavaScript test uses Node's built-in test runner, so the fork takes on no test dependency for
the page half; `cargo xtask ci` stays Rust-only.

SyngleCode's half of the contract tests lives with its code:
`t3-fork/apps/web/src/syngle/observatory/bridge/observatoryBridge.test.ts`.
