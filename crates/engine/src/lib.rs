//! L4: the session (project, undo, active map/layer/tool) and the command registry.
//!
//! **Everything is a command.** Each user-visible action is a [`CommandSpec`] with an id; the
//! ribbon, keyboard shortcuts, command search and the command line all dispatch through
//! [`Session::run`]. Programmatic calls never open dialogs.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cmd;
#[cfg(test)]
mod map_undo_tests;
pub mod model;
pub mod project;
pub mod rows_book;
pub mod sample;
pub mod sources;
#[cfg(test)]
mod tests;

pub use project::Project;
pub use rustgis_analysis as analysis;
pub use rustgis_data as data;
pub use rustgis_geom as geom;
pub use rustgis_map as map;
pub use rustgis_plugins as plugins;
pub use rustgis_render as render;

use std::path::PathBuf;

use rustgis_map::{Camera, Map};
use serde_json::Value;

pub type CmdResult = Result<Value, String>;

/// A command.
#[derive(Clone, Copy)]
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Where it lives in the UI: `Home › Navigate`, `Analyze › Geoprocessing`…
    pub location: &'static str,
    /// Default shortcut (`Mod+S`, `F5`); `Mod` = Ctrl, or ⌘ on macOS.
    pub shortcut: &'static str,
    /// Parameter description for scripts and agents.
    pub params: &'static str,
    /// Whether it changes the project (and so takes an undo snapshot).
    pub mutates: bool,
    /// `None` = enabled; `Some(reason)` = disabled.
    pub enabled: fn(&Session) -> Option<&'static str>,
    pub run: fn(&mut Session, &Value) -> CmdResult,
}

fn always(_: &Session) -> Option<&'static str> {
    None
}

impl CommandSpec {
    /// The top-level keys its [`params`](CommandSpec::params) description names, or `None` when
    /// the description is not a `{"key": …}` object (nothing to check against).
    pub fn param_keys(&self) -> Option<Vec<&'static str>> {
        param_keys(self.params)
    }

    pub const fn new(id: &'static str, label: &'static str, location: &'static str, run: fn(&mut Session, &Value) -> CmdResult) -> Self {
        CommandSpec { id, label, location, shortcut: "", params: "{}", mutates: true, enabled: always, run }
    }
    pub const fn key(mut self, s: &'static str) -> Self {
        self.shortcut = s;
        self
    }
    pub const fn params(mut self, p: &'static str) -> Self {
        self.params = p;
        self
    }
    /// Doesn't change the project (no undo snapshot, not dirtying).
    pub const fn view(mut self) -> Self {
        self.mutates = false;
        self
    }
    /// Whether `map` is one of its own parameters (a bookmark's map, the map to rename): it then
    /// says what it does with it. Any other command takes `map` as the map to act on
    /// ([`Session::run`]).
    pub fn reads_map(&self) -> bool {
        self.param_keys().is_some_and(|keys| keys.contains(&"map"))
    }
    pub const fn when(mut self, f: fn(&Session) -> Option<&'static str>) -> Self {
        self.enabled = f;
        self
    }
}

/// The keys of a `{"key": …, "other"?: …}` description: quoted words at the top level that open
/// an entry (after `{` or a top-level `,`, followed by `?`, `:`, `,`, `}` or `|`) or an
/// alternative (after `|`, followed by `?` or `:`, as in `"path": folder | "url": …` or
/// `"lon" | "x": number`).
pub fn param_keys(desc: &str) -> Option<Vec<&str>> {
    #[derive(PartialEq)]
    enum At {
        Value,
        Entry,
        Alternative,
    }
    let bytes = desc.as_bytes();
    if bytes.iter().find(|c| !c.is_ascii_whitespace()) != Some(&b'{') {
        return None;
    }
    let (mut keys, mut depth, mut at, mut i) = (Vec::new(), 0usize, At::Value, 0);
    while let Some(&c) = bytes.get(i) {
        match c {
            b'"' => {
                let end = bytes.get(i + 1..).and_then(|r| r.iter().position(|&c| c == b'"')).map_or(bytes.len(), |n| i + 1 + n);
                let next = bytes.get(end + 1..).and_then(|r| r.iter().find(|c| !c.is_ascii_whitespace()));
                let key = match at {
                    At::Entry => matches!(next, Some(b'?' | b':' | b',' | b'}' | b'|')),
                    At::Alternative => matches!(next, Some(b'?' | b':')),
                    At::Value => false,
                };
                if depth == 1 && key {
                    keys.extend(desc.get(i + 1..end));
                }
                at = At::Value;
                i = end;
            }
            b'{' | b'[' | b'(' => {
                depth += 1;
                at = if depth == 1 { At::Entry } else { At::Value };
            }
            b'}' | b']' | b')' => {
                depth = depth.saturating_sub(1);
                at = At::Value;
            }
            b',' if depth == 1 => at = At::Entry,
            b'|' if depth == 1 => at = At::Alternative,
            c if c.is_ascii_whitespace() => {}
            _ => at = At::Value,
        }
        i += 1;
    }
    Some(keys)
}

/// `s` cut to `n` characters, with `…` when cut.
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n { s.chars().take(n).chain(['…']).collect() } else { s.to_string() }
}

pub struct Registry {
    pub commands: Vec<CommandSpec>,
}

impl Registry {
    pub fn get(&self, id: &str) -> Option<&CommandSpec> {
        self.commands.iter().find(|c| c.id == id)
    }
}

/// The interactive map tool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Explore,
    Select,
    ZoomIn,
    Measure,
    MeasureArea,
    CreatePoint,
    CreateLine,
    CreatePolygon,
    /// Moves, inserts and deletes the vertices of the selected features.
    EditVertices,
    /// Draws a graphic: the shape is one of [`GRAPHIC_TOOLS`]' shapes.
    Graphic(&'static str),
}

/// The graphic drawing tools: (shape, tool code, label).
pub const GRAPHIC_TOOLS: &[(&str, &str, &str)] = &[
    ("point", "graphicPoint", "Point"),
    ("text", "graphicText", "Text"),
    ("line", "graphicLine", "Line"),
    ("curve", "graphicCurve", "Curve"),
    ("freehand", "graphicFreehand", "Freehand"),
    ("polygon", "graphicPolygon", "Polygon"),
    ("lasso", "graphicLasso", "Lasso"),
    ("rectangle", "graphicRectangle", "Rectangle"),
    ("circle", "graphicCircle", "Circle"),
    ("ellipse", "graphicEllipse", "Ellipse"),
    ("picture", "graphicPicture", "Picture"),
];

impl Tool {
    pub fn code(self) -> &'static str {
        match self {
            Tool::Explore => "explore",
            Tool::Select => "select",
            Tool::ZoomIn => "zoomIn",
            Tool::Measure => "measure",
            Tool::MeasureArea => "measureArea",
            Tool::CreatePoint => "createPoint",
            Tool::CreateLine => "createLine",
            Tool::CreatePolygon => "createPolygon",
            Tool::EditVertices => "editVertices",
            Tool::Graphic(shape) => GRAPHIC_TOOLS.iter().find(|g| g.0 == shape).map_or("graphicPoint", |g| g.1),
        }
    }
    pub fn from_code(s: &str) -> Option<Tool> {
        if let Some(g) = GRAPHIC_TOOLS.iter().find(|g| g.1.eq_ignore_ascii_case(s.trim())) {
            return Some(Tool::Graphic(g.0));
        }
        [
            Tool::Explore,
            Tool::Select,
            Tool::ZoomIn,
            Tool::Measure,
            Tool::MeasureArea,
            Tool::CreatePoint,
            Tool::CreateLine,
            Tool::CreatePolygon,
            Tool::EditVertices,
        ]
        .into_iter()
        .find(|t| t.code().eq_ignore_ascii_case(s.trim()))
    }
    pub fn label(self) -> &'static str {
        match self {
            Tool::Explore => "Explore",
            Tool::Select => "Select",
            Tool::ZoomIn => "Zoom In",
            Tool::Measure => "Measure Distance",
            Tool::MeasureArea => "Measure Area",
            Tool::CreatePoint => "Create Point",
            Tool::CreateLine => "Create Line",
            Tool::CreatePolygon => "Create Polygon",
            Tool::EditVertices => "Edit Vertices",
            Tool::Graphic(shape) => GRAPHIC_TOOLS.iter().find(|g| g.0 == shape).map_or("Point", |g| g.2),
        }
    }
    pub fn creates(self) -> bool {
        matches!(self, Tool::CreatePoint | Tool::CreateLine | Tool::CreatePolygon)
    }
}

/// One geoprocessing history row.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GpRecord {
    pub tool: String,
    pub params: Value,
    pub ok: bool,
    pub messages: Vec<String>,
    pub millis: u64,
    /// The layers the run added (by name at the time).
    #[serde(default)]
    pub outputs: Vec<String>,
}

/// A topology problem found by `topology.validate` (shown in the Error Inspector).
#[derive(Clone, Debug, PartialEq)]
pub struct TopologyIssue {
    pub rule: String,
    pub layer: String,
    pub rows: Vec<usize>,
    pub lon: f64,
    pub lat: f64,
}

/// Something worth telling the user (status bar / notifications).
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

const UNDO_LIMIT: usize = 40;

/// An undo or redo step: the project before it, the layout open then, its label and who made it, and whose it is (#472, ADR 0022).
///
/// History is one stack per map: a step belongs to the map it was made in (`owner`), and the map in view undoes the last
/// step that is its own. A step that changed more than one map's own content (a map added or removed, a table, a layout, the
/// project's settings) belongs to the project (`owner` is `None`) and is on every map's stack, undone by putting the whole
/// project back. A step that is a map's own keeps only that map (and the maps whose rows it changed), and undoing it puts that
/// map back and the rows it changed in every map that holds them.
struct Step {
    project: Project,
    layout: Option<usize>,
    label: &'static str,
    actor: Option<String>,
    owner: Option<u32>,
    /// The layers (map id, layer id) whose rows the step replaced: a row edit is one change to the data in every map that holds it.
    rows: Vec<(u32, u32)>,
    /// The other maps the step's rows changed in, for its label.
    also: Vec<String>,
}

impl Step {
    fn label(&self) -> String {
        if self.also.is_empty() { self.label.to_string() } else { format!("{}, also in {}", self.label, self.also.join(", ")) }
    }
}

/// The project with no map's content: what a step that is one map's own leaves alone.
fn outside_maps(p: &mut Project) -> Value {
    let (maps, active) = (std::mem::take(&mut p.maps), p.active_map);
    p.active_map = 0;
    let doc = rustgis_data::without_rows(|| serde_json::to_value(&*p)).unwrap_or(Value::Null);
    p.maps = maps;
    p.active_map = active;
    doc
}

pub struct Session {
    pub project: Project,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    /// Favourite, recent and saved data sources (kept with the app's preferences, not the
    /// project).
    pub sources: sources::Sources,
    /// How rasters added from now on are drawn (`raster.setDefaultStyle`; kept with the app's
    /// preferences, not the project).
    pub raster_defaults: rustgis_map::RasterDefaults,
    pub registry: Registry,
    /// Bumped on every change so views can cache.
    pub revision: u64,
    /// Bumped each time another project replaces this one (new, open, sample, template), however
    /// the command came (the app, an agent, a script): the interface drops what it kept for the
    /// old one.
    pub opened: u64,
    pub tool: Tool,
    /// The layer selected in the Layers pane (the one the Style and Data tabs act on).
    pub active_layer: Option<u32>,
    /// Map view size in logical points (set by the UI each frame).
    pub view_size: (f64, f64),
    /// A MapLibre style tile layer (by its address) whose own view the map goes to once it loads
    /// (`map.addTiles` with `"view": true`), and the camera then: moved since, the map stays.
    pub style_view: Option<(String, Camera)>,
    pub notices: Vec<Notice>,
    /// Notices given in this session, including ones no longer kept (see [`Session::notices_since`]),
    /// less those a failed command withdrew.
    notices_given: u64,
    /// Layer whose attribute table is open.
    pub table_layer: Option<u32>,
    /// The id of the map in view while a command acts on another one ([`Session::on_map`]).
    in_view: Option<u32>,
    /// The standalone table shown in the attribute table instead of a layer, by its id.
    pub table_standalone: Option<u32>,
    /// Edits per layer id since the project was last saved (Edit › Status). A failed command's
    /// are rolled back; undo doesn't subtract them, since they count edits made, not the net change.
    pub edit_counts: std::collections::HashMap<u32, cmd::design::EditCounts>,
    /// The feature cache fills itself after projection changes (Data › Feature Cache).
    pub auto_cache: bool,
    /// Mensuration results (Analyze › Mensuration › Results).
    pub measurements: Vec<serde_json::Value>,
    /// The last topology check's errors (Edit › Error Inspector).
    pub topology_errors: Vec<TopologyIssue>,
    /// The layout shown instead of the map view, if any (an index into `project.layouts`).
    pub active_layout: Option<usize>,
    /// Tasks made in this session (lower-cased names). Others came from files, and their steps
    /// may not touch files when run.
    pub trusted_tasks: std::collections::HashSet<String>,
    /// Models made in this session (lower-cased names); the steps of others may not touch files.
    pub trusted_models: std::collections::HashSet<String>,
    /// What the last model run did, node by node (`model::RunReport`), kept even when the run
    /// failed and the project was restored, so ModelBuilder can show which node stopped it.
    pub model_run: Option<model::RunReport>,
    /// Counts the projects started in this session (new, open): front ends holding state about
    /// the project (ModelBuilder's canvas) drop it when this changes.
    pub project_starts: u64,
    /// Notebooks made in this session (lower-cased names); the cells of others may not touch files.
    pub trusted_notebooks: std::collections::HashSet<String>,
    /// A notebook is running: it may not start another, even through a task or model (no
    /// unbounded recursion).
    pub(crate) notebook_running: bool,
    /// How many geoprocessing history records came with the opened project (from its file).
    pub history_from_file: usize,
    /// The report open in the Report pane, if any.
    pub active_report: Option<usize>,
    /// The presentation and notebook open in their panes, if any.
    pub active_presentation: Option<usize>,
    pub active_notebook: Option<usize>,
    /// Drawing tools stay active after a graphic is drawn (Edit › Graphics).
    pub keep_tool: bool,
    /// Copied features (Edit › Clipboard).
    pub clipboard: Option<cmd::clipboard::Clipboard>,
    /// Copied layers (Layers › Copy, #491).
    pub layer_clipboard: Option<cmd::layer_copy::LayerClip>,
    /// Each step keeps the layout that was open with its project (`active_layout` is the
    /// session's, so a step that added or moved layouts opens the right one when undone).
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Who runs commands now: `None` is the user; the MCP relay names an attached agent around
    /// its calls. Each undo step remembers who made it ([`Session::undo_actor`]).
    pub actor: Option<String>,
    back: Vec<Camera>,
    forward: Vec<Camera>,
    /// Last camera recorded in the extent history.
    last_camera: Option<Camera>,
    /// The projection the extent history's cameras are in (their units change with it).
    history_projection: String,
    /// Finished asynchronous tool runs waiting for `gp.finish` (the browser's GeoLibre tools).
    pub(crate) pending: Vec<PendingRun>,
    pub(crate) next_pending: u64,
    /// Asynchronous runs started and not yet complete: (id, job, resolved params, start time).
    #[cfg(feature = "geolibre")]
    pub(crate) started: Vec<(u64, rustgis_geolibre::Job, serde_json::Value, u64)>,
    /// Plugin tool runs started and not yet complete (`gp::plugin_start`).
    pub(crate) plugin_runs: Vec<cmd::gp::StartedRun>,
    /// Plugins' panels, by plugin id: the widgets' values and what the plugin last said about
    /// them (it keeps nothing between calls). Not saved with the project: a plugin's state
    /// command reads what it needs from the map when the panel opens.
    pub plugin_panels: std::collections::BTreeMap<String, rustgis_plugins::PanelState>,
    /// Data downloads of this project under way (Add Data › From URL), by id.
    downloading: Vec<u64>,
    /// The rows of a project opened in parts (#471).
    pub rows_book: rows_book::RowsBook,
    /// Downloads that landed, waiting for `layer.addDownload`.
    pub(crate) downloaded: Vec<(u64, Vec<u8>)>,
    /// Downloads a command asked for where nothing may wait (the browser): the parameters of
    /// `layer.addUrl` calls, for the front end to fetch and add (see [`Session::take_url_requests`]).
    pub(crate) url_requests: Vec<serde_json::Value>,
    /// Set by [`Session::request_url`] for [`Session::guarded`]: the command running changed
    /// nothing, it only left a request (a plugin's own answer can't claim that).
    request_only: bool,
    /// Plugin commands waiting for their `host.fetch` answers to run again (the browser).
    pub(crate) plugin_retries: Vec<cmd::plugins::fetch::Retry>,
    /// How many times the plugin command running now has been run again for its fetches.
    pub(crate) retry_tries: u32,
    /// Set for [`Session::guarded`] by a command whose failure is no news (a plugin command
    /// undone to run again once its fetches land): no error notice.
    pub(crate) quiet_failure: bool,
    /// Query results the app ran off the frame (the SQL pane), waiting for `layer.addSql {"ran"}`:
    /// the id, the query and database (label) they came from, and the result.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) sql_results: Vec<(u64, String, Option<String>, rustgis_data::source::duckdb::Output)>,
}

/// The most `layer.addUrl` requests kept for the front end at a time.
const MAX_URL_REQUESTS: usize = 16;

/// A finished tool run, held until `gp.finish` applies it.
pub(crate) struct PendingRun {
    pub id: u64,
    pub tool: String,
    pub params: serde_json::Value,
    pub result: Result<rustgis_analysis::ToolResult, String>,
    pub millis: u64,
}

/// Empties the windows of a project's streamed layers (they are read again from their sources).
fn strip_windows(p: &mut Project) {
    for l in p.maps.iter_mut().flat_map(|m| m.layers.iter_mut()) {
        if let Some(src) = &mut l.source {
            src.reset();
            l.dataset.features = Vec::new().into();
            l.selection.clear();
            l.touch_geometry();
        }
    }
}

/// The first streamed layer whose features or fields differ between two versions of a project
/// for a reason other than a read of its source.
fn edited_streamed(before: &Project, after: &Project) -> Option<String> {
    let schema = |l: &rustgis_map::Layer| l.dataset.fields.iter().map(|f| (f.name.clone(), f.ty)).collect::<Vec<_>>();
    for (mb, ma) in before.maps.iter().zip(&after.maps) {
        for la in ma.layers.iter().filter(|l| l.is_streamed()) {
            let Some(lb) = mb.layers.iter().find(|l| l.id == la.id && l.is_streamed()) else { continue };
            let reads = |l: &rustgis_map::Layer| l.source.as_ref().map(|s| s.reads);
            if reads(la) == reads(lb) && (la.dataset.features != lb.dataset.features || schema(la) != schema(lb)) {
                return Some(la.name.clone());
            }
        }
    }
    None
}

/// Why command `label` can't edit streamed layer `layer`, refused up front or undone after.
fn streamed_refusal(label: &str, layer: &str) -> String {
    format!("{label}: {layer} is read in place from its file; Load All Features (layer.loadAll) to edit it")
}

/// Commands that change a layer's features or fields, refused on streamed layers (whose features
/// are a window read again from the file as the view moves, so edits would be lost).
fn edits_features(id: &str) -> bool {
    const SETTINGS: &[&str] =
        &["edit.enable", "edit.settings", "edit.status", "edit.template", "edit.snapping", "edit.snap", "edit.tools", "edit.copy"];
    id.starts_with("edit.") && !SETTINGS.contains(&id)
}

impl Session {
    pub fn new(project: Project) -> Self {
        // Cheap: the suite's tools load the first time a tool list is needed.
        #[cfg(feature = "geolibre")]
        rustgis_geolibre::register();
        rustgis_plugins::tools::register();
        // FlatGeobuf and GeoParquet by their web address are read through the tile fetchers.
        render::rustgis_tiles::remote::install();
        Session {
            project,
            path: None,
            dirty: false,
            sources: sources::Sources::default(),
            raster_defaults: rustgis_map::RasterDefaults::default(),
            registry: Registry { commands: cmd::all() },
            revision: 1,
            opened: 0,
            tool: Tool::Explore,
            active_layer: None,
            view_size: (1200.0, 720.0),
            style_view: None,
            notices: Vec::new(),
            notices_given: 0,
            table_layer: None,
            in_view: None,
            table_standalone: None,
            clipboard: None,
            layer_clipboard: None,
            keep_tool: false,
            active_layout: None,
            trusted_tasks: Default::default(),
            trusted_models: Default::default(),
            model_run: None,
            project_starts: 0,
            trusted_notebooks: Default::default(),
            notebook_running: false,
            history_from_file: 0,
            active_report: None,
            active_presentation: None,
            active_notebook: None,
            edit_counts: std::collections::HashMap::new(),
            topology_errors: Vec::new(),
            measurements: Vec::new(),
            auto_cache: false,
            undo: Vec::new(),
            redo: Vec::new(),
            actor: None,
            back: Vec::new(),
            forward: Vec::new(),
            last_camera: None,
            history_projection: String::new(),
            pending: Vec::new(),
            next_pending: 0,
            #[cfg(feature = "geolibre")]
            started: Vec::new(),
            plugin_runs: Vec::new(),
            plugin_panels: std::collections::BTreeMap::new(),
            downloading: Vec::new(),
            rows_book: rows_book::RowsBook::default(),
            downloaded: Vec::new(),
            url_requests: Vec::new(),
            request_only: false,
            plugin_retries: Vec::new(),
            retry_tries: 0,
            quiet_failure: false,
            #[cfg(not(target_arch = "wasm32"))]
            sql_results: Vec::new(),
        }
    }

    /// Keeps a query's result the app ran off the frame, for `layer.addSql {"ran": id}`, with the
    /// query and database it came from: the command takes it only for that same query, so the
    /// layer's saved query is the one that made it. A few at a time: past that many, the oldest
    /// are forgotten.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn sql_ran(&mut self, sql: &str, database: Option<&str>, out: rustgis_data::source::duckdb::Output) -> u64 {
        self.next_pending = self.next_pending.wrapping_add(1);
        if self.sql_results.len() >= 8 {
            self.sql_results.remove(0);
        }
        let database = database.and_then(|d| cmd::layer::database_param(&serde_json::json!({"database": d}))).map(|d| d.label());
        self.sql_results.push((self.next_pending, sql.to_string(), database, out));
        self.next_pending
    }

    /// Starts a data download for this project (the front end fetches it): its id, for
    /// [`Session::download_landed`].
    pub fn download_started(&mut self) -> u64 {
        self.next_pending = self.next_pending.wrapping_add(1);
        // A few at a time: past that many, the oldest are forgotten.
        if self.downloading.len() >= 64 {
            self.downloading.remove(0);
        }
        self.downloading.push(self.next_pending);
        self.next_pending
    }

    /// Hands over a download's bytes, for `layer.addDownload {"id"}`; `false` when it was dropped
    /// (another project was opened meanwhile).
    pub fn download_landed(&mut self, id: u64, bytes: Vec<u8>) -> bool {
        let Some(i) = self.downloading.iter().position(|d| *d == id) else { return false };
        self.downloading.remove(i);
        self.downloaded.push((id, bytes));
        true
    }

    /// Forgets a download that failed.
    pub fn download_failed(&mut self, id: u64) {
        self.downloading.retain(|d| *d != id);
    }

    /// Forgets a download the front end opens itself (a project by its address), saying whether
    /// it still belongs to this project: `false` when another project was opened meanwhile.
    pub fn download_done(&mut self, id: u64) -> bool {
        let current = self.downloading.contains(&id);
        self.download_failed(id);
        current
    }

    /// Keeps a `layer.addUrl` request for the front end to download off the frame and add when
    /// it lands (the browser, where a command may not wait): the command's answer, which says
    /// so. An address asked for again while it waits is asked for once, and past a few waiting
    /// requests the next is refused rather than silently dropping one already promised.
    pub fn request_url(&mut self, p: &serde_json::Value) -> Result<serde_json::Value, String> {
        let url = p.get("url").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        if !self.url_requests.iter().any(|q| q.get("url").and_then(serde_json::Value::as_str) == Some(url.as_str())) {
            if self.url_requests.len() >= MAX_URL_REQUESTS {
                return Err(format!("{url}: {MAX_URL_REQUESTS} downloads are already waiting; try again when they have landed"));
            }
            self.url_requests.push(p.clone());
        }
        self.request_only = true;
        Ok(serde_json::json!({"pending": true, "url": url}))
    }

    /// The `layer.addUrl` requests waiting for the front end ([`Session::request_url`]), taken.
    pub fn take_url_requests(&mut self) -> Vec<serde_json::Value> {
        std::mem::take(&mut self.url_requests)
    }

    /// Whether background work still belongs to this project.
    pub fn download_is_pending(&self, id: u64) -> bool {
        self.downloading.contains(&id)
    }

    pub fn map(&self) -> &Map {
        self.project.map()
    }
    pub fn map_mut(&mut self) -> &mut Map {
        self.project.map_mut()
    }

    /// Moves the map to the view of a style added with `"view": true` once the style has loaded
    /// (the UI asks each frame). True when the map moved.
    pub fn follow_style_view(&mut self) -> bool {
        cmd::tiles::follow_style_view(self, web_time::Duration::ZERO)
    }

    /// Runs a command by id. Mutating commands snapshot the project for undo; an error or a
    /// panic restores it.
    pub fn run(&mut self, id: &str, params: &Value) -> CmdResult {
        // A style that loaded since: its view first, so the command sees where the map is.
        if self.style_view.is_some() {
            self.follow_style_view();
        }
        if id.starts_with(rustgis_plugins::PREFIX) {
            return self.run_plugin(id, params);
        }
        let Some(&spec) = self.registry.get(id) else { return Err(format!("unknown command '{id}'")) };
        // A command may name the map it acts on: it runs with that map active and leaves the map
        // in view as it was, so two windows on one project never fight over it.
        match self.other_map(id, params).map_err(|e| e.to_string())? {
            Some(map) => self.on_map(map, |s| s.run_spec(spec, id, params)),
            None => self.run_spec(spec, id, params),
        }
    }

    /// The map a command acts on when that is not the one in view: `params.map`, an id or a
    /// name, for a command that has no `map` parameter of its own. `None` is the map in view.
    pub fn other_map(&self, id: &str, params: &Value) -> Result<Option<usize>, project::NoMap> {
        let Some(named) = params.get("map").filter(|m| !m.is_null()) else { return Ok(None) };
        if id.starts_with(rustgis_plugins::PREFIX) || self.registry.get(id).is_none_or(|spec| spec.reads_map()) {
            return Ok(None);
        }
        let map = project::find_map(&self.project.maps, named)?;
        Ok((map != self.project.active_map).then_some(map))
    }

    /// Runs `f` with the map at `map` active, then puts back the map that was in view, with its
    /// active layer and open table. The map in view is found again by id: a command may add or
    /// remove maps. If it removed that one, the view is wherever the command left it.
    pub fn on_map<R>(&mut self, map: usize, f: impl FnOnce(&mut Session) -> R) -> R {
        if map == self.project.active_map || map >= self.project.maps.len() {
            return f(self);
        }
        let seen = (self.map().id, self.active_layer, self.table_layer);
        // Only the outermost call knows which map is in view (a script's line may name a third).
        let outer = self.in_view.is_none();
        if outer {
            self.in_view = Some(seen.0);
        }
        self.project.active_map = map;
        self.active_layer = self.map().layers.first().map(|l| l.id);
        self.table_layer = None;
        let res = f(self);
        if outer {
            self.in_view = None;
        }
        if let Some(back) = self.project.map_index(seen.0) {
            self.project.active_map = back;
            (self.active_layer, self.table_layer) = (seen.1, seen.2);
        }
        self.fix_active_layer();
        res
    }

    fn run_spec(&mut self, spec: CommandSpec, id: &str, params: &Value) -> CmdResult {
        // Keys the command does not read: said, so a script or an agent can correct itself.
        if let (Value::Object(given), Some(known)) = (params, spec.param_keys()) {
            // Bounded: the keys come from the caller (the first few, each clipped). `map` is every
            // command's: the map it acts on.
            let unknown: Vec<&String> = given.keys().filter(|k| !known.contains(&k.as_str()) && k.as_str() != "map").collect();
            if !unknown.is_empty() {
                let mut said: Vec<String> = unknown.iter().take(5).map(|k| format!("'{}'", clip(k, 40))).collect();
                if unknown.len() > 5 {
                    said.push(format!("{} more", unknown.len() - 5));
                }
                let reads = if known.is_empty() { "no parameters".to_string() } else { known.join(", ") };
                self.notify(format!("{id} ignores {} (it reads {reads})", said.join(", ")), false);
            }
        }
        // Layer conditions apply to the layer the parameters name, falling back to the active one.
        let named = params.get("layer").and_then(|_| self.layer_id(params).ok());
        let active = self.active_layer;
        if named.is_some() {
            self.active_layer = named;
        }
        // Restored on every path, even if a condition were to panic.
        let enabled = spec.enabled;
        let blocked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| enabled(self))).unwrap_or(Some("unavailable"));
        self.active_layer = active;
        if let Some(why) = blocked {
            return Err(format!("{}: {why}", spec.label));
        }
        let (run, mutates, label) = (spec.run, spec.mutates, spec.label);
        if let Some(e) = self.streamed_edit(id, mutates, label, named.or(active)) {
            // Said, as a failure in `guarded` is: this returns before it (#468).
            self.notify(e.clone(), true);
            return Err(e);
        }
        let res = self.guarded(label, mutates, |s| run(s, params));
        // What was added, for the Data Sources pane's Recent list.
        if res.is_ok()
            && let Some(src) = sources::added_by(id, params)
        {
            self.sources.note(src);
        }
        res
    }

    /// Why a command can't run on `layer`: it edits features, and the layer is read in place from
    /// its file, whose next read would lose the edit.
    pub(crate) fn streamed_edit(&self, id: &str, mutates: bool, label: &str, layer: Option<u32>) -> Option<String> {
        if !(mutates && edits_features(id)) {
            return None;
        }
        let l = layer.and_then(|l| self.map().layer(l)).filter(|l| l.is_streamed())?;
        Some(streamed_refusal(label, &l.name))
    }

    /// Runs `f` as the command `label`: when it `mutates`, the project is snapshotted for undo
    /// and restored if `f` fails or panics.
    pub(crate) fn guarded(&mut self, label: &'static str, mutates: bool, f: impl FnOnce(&mut Session) -> CmdResult) -> CmdResult {
        // The map the command acts on owns the step it makes.
        let acting = self.project.maps.get(self.project.active_map).map(|m| m.id);
        // The step undo goes back to shows the map that was in view, not the one a command named.
        let snapshot = mutates.then(|| {
            let mut before = self.project.clone();
            if let Some(seen) = self.in_view.and_then(|id| before.map_index(id)) {
                before.active_map = seen;
            }
            before
        });
        let open_layout = self.active_layout;
        // The project's generation rolls back with it: a project opened part way (a plugin's
        // command through its bridge) and then undone is the one before, still open.
        let opened = self.opened;
        // Edit counts roll back with the project when a command fails part way.
        let counts = mutates.then(|| self.edit_counts.clone());
        self.request_only = false;
        self.quiet_failure = false;
        // Requests a failed command left for the front end go with its other changes.
        let requests = self.url_requests.len();
        let said = self.notices_given;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)))
            .unwrap_or_else(|_| Err(format!("{label} failed unexpectedly; the project was restored")));
        // A map the command made (new, a copy, an imported one) gets its id.
        self.project.give_map_ids();
        // A command that changed a streamed layer's features or fields (not by reading its
        // source) would lose the change at the next read: undone, with a way to edit it instead.
        let res = match (&res, &snapshot) {
            (Ok(_), Some(snap)) => match edited_streamed(snap, &self.project) {
                Some(name) => Err(streamed_refusal(label, &name)),
                None => res,
            },
            _ => res,
        };
        match (&res, snapshot) {
            // A command that only left a request for the front end (a download in the browser,
            // [`Session::request_url`]) changed nothing: no undo step, nothing to save yet. The
            // session's own flag says so, not the answer, which a plugin's command could forge.
            (Ok(_), Some(_)) if std::mem::take(&mut self.request_only) => {}
            (Ok(_), Some(mut snap)) => {
                let (owner, rows, also) = self.scope_of(&mut snap, acting);
                // Undo keeps streamed layers' sources, not their windows: they read the view again.
                strip_windows(&mut snap);
                if owner.is_some() {
                    // A map's own step keeps that map and the maps whose rows it changed, not the whole project.
                    snap.maps.retain(|m| Some(m.id) == owner || rows.iter().any(|(map, _)| *map == m.id));
                }
                self.undo.push(Step { project: snap, layout: open_layout, label, actor: self.actor.clone(), owner, rows, also });
                // Each stack keeps its last steps; a project step counts for the project.
                while self.undo.iter().filter(|u| u.owner == owner).count() > UNDO_LIMIT {
                    if let Some(i) = self.undo.iter().position(|u| u.owner == owner) {
                        self.undo.remove(i);
                    }
                }
                // A new step ends the redo of its own stack, and of the project's.
                if owner.is_none() {
                    self.redo.clear();
                } else {
                    self.redo.retain(|r| r.owner.is_some() && r.owner != owner);
                }
                self.dirty = true;
            }
            (Err(_), Some(snap)) => {
                // The project as it was before the command, so the layout open then, whatever
                // the command opened on the way (a task's steps).
                self.project = snap;
                self.active_layout = open_layout;
                self.opened = opened;
                if let Some(c) = counts {
                    self.edit_counts = c;
                }
                // What it said it had done ("Deleted 3 features") didn't happen: those notices
                // go; its errors stay.
                let new = usize::try_from(self.notices_given.saturating_sub(said)).unwrap_or(usize::MAX).min(self.notices.len());
                let start = self.notices.len() - new;
                let errors: Vec<Notice> = self.notices.drain(start..).filter(|n| n.error).collect();
                self.notices_given -= (new - errors.len()) as u64;
                self.notices.extend(errors);
            }
            _ => {}
        }
        if res.is_err() {
            self.url_requests.truncate(requests);
        }
        let quiet = std::mem::take(&mut self.quiet_failure);
        if let Err(e) = &res
            && !quiet
        {
            self.notify(e.clone(), true);
        }
        self.fix_active_layer();
        self.revision = self.revision.wrapping_add(1);
        res
    }

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notices.push(Notice { text: text.into(), error });
        self.notices_given += 1;
        if self.notices.len() > 200 {
            self.notices.remove(0);
        }
    }

    /// A mark to pass to [`Session::notices_since`].
    pub fn notice_mark(&self) -> u64 {
        self.notices_given
    }

    /// The notices given since `mark` was taken (those still kept): what a command said beside its
    /// result, for front ends without a status bar (the CLI, the MCP server, the AI Assistant).
    pub fn notices_since(&self, mark: u64) -> &[Notice] {
        let n = usize::try_from(self.notices_given.saturating_sub(mark)).unwrap_or(usize::MAX).min(self.notices.len());
        self.notices.get(self.notices.len() - n..).unwrap_or_default()
    }

    /// The map in view: the one whose stack Undo and Redo work on.
    fn view_map(&self) -> Option<u32> {
        self.project.maps.get(self.project.active_map).map(|m| m.id)
    }

    fn on_stack(&self, step: &Step) -> bool {
        step.owner.is_none() || step.owner == self.view_map()
    }

    /// The step undo would take back for the map in view, as its place in the history.
    fn top_undo(&self) -> Option<usize> {
        self.undo.iter().rposition(|u| self.on_stack(u))
    }

    fn top_redo(&self) -> Option<usize> {
        self.redo.iter().rposition(|u| self.on_stack(u))
    }

    pub fn can_undo(&self) -> bool {
        self.top_undo().is_some()
    }
    pub fn can_redo(&self) -> bool {
        self.top_redo().is_some()
    }
    /// How many steps redo would bring back for the map in view, beside [`Session::undo_depth`]. The Observatory bridge
    /// reports both: a client tells a step that was undone (and can be redone) from a command that only read, or replaced
    /// the project and cleared the history: only a grown depth is a step of the shared history, and redo has to put the
    /// depth back up.
    pub fn redo_depth(&self) -> usize {
        self.redo.iter().filter(|u| self.on_stack(u)).count()
    }
    /// Who made the step undo would take back: `Some(None)` for the user, `Some(Some(name))`
    /// for an agent ([`Session::actor`]), `None` when there is nothing to undo.
    pub fn undo_actor(&self) -> Option<Option<&str>> {
        self.top_undo().and_then(|i| self.undo.get(i)).map(|u| u.actor.as_deref())
    }
    /// Who made the step redo would bring back, as [`Session::undo_actor`].
    pub fn redo_actor(&self) -> Option<Option<&str>> {
        self.top_redo().and_then(|i| self.redo.get(i)).map(|u| u.actor.as_deref())
    }
    pub fn undo_label(&self) -> Option<&'static str> {
        self.top_undo().and_then(|i| self.undo.get(i)).map(|u| u.label)
    }
    pub fn redo_label(&self) -> Option<&'static str> {
        self.top_redo().and_then(|i| self.redo.get(i)).map(|u| u.label)
    }
    /// The label with the other maps a row edit changed ("Add Feature, also in Overview").
    pub fn undo_label_full(&self) -> Option<String> {
        self.top_undo().and_then(|i| self.undo.get(i)).map(Step::label)
    }
    pub fn redo_label_full(&self) -> Option<String> {
        self.top_redo().and_then(|i| self.redo.get(i)).map(Step::label)
    }
    /// How many steps undo and redo hold for a map (its own and the project's).
    pub fn map_history(&self, map: u32) -> (usize, usize) {
        let on = |u: &Step| u.owner.is_none() || u.owner == Some(map);
        (self.undo.iter().filter(|u| on(u)).count(), self.redo.iter().filter(|u| on(u)).count())
    }

    /// Forgets undo/redo and extent history and marks the project clean (after new/open, or a
    /// front end setting up how the project starts).
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.back.clear();
        self.forward.clear();
        self.last_camera = None;
        self.dirty = false;
        self.trusted_tasks.clear();
        self.trusted_models.clear();
        self.model_run = None;
        self.project_starts = self.project_starts.wrapping_add(1);
        self.trusted_notebooks.clear();
        self.edit_counts.clear();
        self.topology_errors.clear();
        self.measurements.clear();
        // Runs of the previous project mustn't land in this one.
        self.pending.clear();
        #[cfg(feature = "geolibre")]
        self.started.clear();
        self.plugin_runs.clear();
        self.downloading.clear();
        self.downloaded.clear();
        self.url_requests.clear();
        self.plugin_retries.clear();
        // A style of the last project still loading doesn't move this one.
        self.style_view = None;
        #[cfg(not(target_arch = "wasm32"))]
        self.sql_results.clear();
    }

    pub(crate) fn undo(&mut self) -> bool {
        while let Some(i) = self.top_undo() {
            let step = self.undo.remove(i);
            if let Some(counterpart) = self.swap_step(step) {
                self.redo.push(counterpart);
                return true;
            }
        }
        false
    }

    pub(crate) fn redo(&mut self) -> bool {
        while let Some(i) = self.top_redo() {
            let step = self.redo.remove(i);
            if let Some(counterpart) = self.swap_step(step) {
                self.undo.push(counterpart);
                return true;
            }
        }
        false
    }

    /// Puts a step's project back and answers the step that undoes that. A project step swaps the whole project; a map's own
    /// step puts that map back, and the rows it changed back in every map that holds them, and leaves the other maps as
    /// they are. `None` when the map the step belongs to has gone (it is dropped).
    fn swap_step(&mut self, step: Step) -> Option<Step> {
        let Step { project: before, layout: saved, label, actor, owner, rows, also } = step;
        let (open, was) = (self.open_layout_name(), self.active_layout);
        let cur = match owner {
            None => {
                let cur = std::mem::replace(&mut self.project, before);
                self.project.keep_table_ids(&cur);
                self.project.keep_map_ids(&cur);
                cur
            }
            Some(map) => {
                let (here, there) = (self.project.map_index(map)?, before.map_index(map)?);
                // What undoes this one: the same step's other side, holding only the maps it touches.
                let touched = |id: u32| id == map || rows.iter().any(|(r, _)| *r == id);
                let all = std::mem::take(&mut self.project.maps);
                let mut cur = self.project.clone();
                self.project.maps = all;
                cur.maps = self.project.maps.iter().filter(|m| touched(m.id)).cloned().collect();
                self.project.maps[here] = before.maps[there].clone();
                for (map_id, layer_id) in &rows {
                    let Some(rows_before) =
                        before.map_index(*map_id).and_then(|m| before.maps[m].layer(*layer_id)).map(|l| l.dataset.features.clone())
                    else {
                        continue;
                    };
                    if let Some(layer) = self.project.map_index(*map_id).and_then(|m| self.project.maps[m].layer_mut(*layer_id)) {
                        layer.dataset.features = rows_before;
                    }
                }
                self.fix_active_layer();
                self.touch_geometry_of(|id| id == map || rows.iter().any(|(r, _)| *r == id));
                cur
            }
        };
        self.reopen_layout(open, saved);
        if owner.is_none() {
            self.touch_all_geometry();
        }
        Some(Step { project: cur, layout: was, label, actor, owner, rows, also })
    }

    /// Whose step a command's was, from the project before it (`before`) and now: the map it acted on, unless it changed the
    /// project beyond maps' own content (then the project's). Rows a command changed in one map's layer are put in every
    /// other map that held the same rows, so the data is one thing across maps; the layers that took the change are returned
    /// with the names of the maps they are in.
    fn scope_of(&mut self, before: &mut Project, acting: Option<u32>) -> (Option<u32>, Vec<(u32, u32)>, Vec<String>) {
        let same_maps =
            before.maps.len() == self.project.maps.len() && before.maps.iter().zip(&self.project.maps).all(|(a, b)| a.id == b.id);
        let layer_before =
            |p: &Project, map: u32, layer: u32| p.map_index(map).and_then(|m| p.maps[m].layer(layer)).map(|l| l.dataset.features.clone());
        // The layers whose rows are not the rows they were.
        let mut changed: Vec<(u32, u32)> = Vec::new();
        if same_maps {
            for m in &self.project.maps {
                for l in &m.layers {
                    if layer_before(before, m.id, l.id).is_some_and(|b| !b.shares(&l.dataset.features)) {
                        changed.push((m.id, l.id));
                    }
                }
            }
        }
        // Other maps' layers that held the same rows as a changed one, and have not changed themselves, take its new rows.
        let mut taken: Vec<((u32, u32), rustgis_data::Features)> = Vec::new();
        for (map, layer) in &changed {
            let (Some(old), Some(new)) = (
                layer_before(before, *map, *layer),
                self.project.map_index(*map).and_then(|m| self.project.maps[m].layer(*layer)).map(|l| l.dataset.features.clone()),
            ) else {
                continue;
            };
            for m in &self.project.maps {
                for l in &m.layers {
                    let key = (m.id, l.id);
                    if key != (*map, *layer)
                        && !changed.contains(&key)
                        && !taken.iter().any(|(k, _)| *k == key)
                        && layer_before(before, m.id, l.id).is_some_and(|b| b.shares(&old) && b.shares(&l.dataset.features))
                    {
                        taken.push((key, new.clone()));
                    }
                }
            }
        }
        for ((map, layer), features) in taken {
            if let Some(l) = self.project.map_index(map).and_then(|m| self.project.maps[m].layer_mut(layer)) {
                l.dataset.features = features;
                l.touch_geometry();
                changed.push((map, layer));
            }
        }
        let project_wide = !same_maps || acting.is_none() || outside_maps(before) != outside_maps(&mut self.project);
        if project_wide {
            return (None, Vec::new(), Vec::new());
        }
        let mut also: Vec<String> = Vec::new();
        for (map, _) in &changed {
            if Some(*map) != acting
                && let Some(name) = self.project.map_index(*map).map(|m| self.project.maps[m].name.clone())
                && !also.contains(&name)
            {
                also.push(name);
            }
        }
        (acting, changed, also)
    }

    fn open_layout_name(&self) -> Option<String> {
        self.active_layout.and_then(|i| self.project.layouts.get(i)).map(|l| l.name.clone())
    }

    /// After undo or redo swaps the project: the open layout stays open wherever it now is (so
    /// redo doesn't move the view to a layout it brings back); one the swap took away gives way to
    /// the layout open with the restored project (`saved`). With no layout open, none opens.
    fn reopen_layout(&mut self, open: Option<String>, saved: Option<usize>) {
        let ls = &self.project.layouts;
        self.active_layout = open.and_then(|n| ls.iter().position(|l| l.name.eq_ignore_ascii_case(&n)).or(saved.filter(|i| *i < ls.len())));
    }

    /// Marks the geometry of the layers of the maps `which` picks as changed, so they are drawn again.
    fn touch_geometry_of(&mut self, which: impl Fn(u32) -> bool) {
        for m in self.project.maps.iter_mut().filter(|m| which(m.id)) {
            for l in &mut m.layers {
                l.touch_geometry();
            }
        }
        self.revision = self.revision.wrapping_add(1);
    }

    fn touch_all_geometry(&mut self) {
        for m in &mut self.project.maps {
            for l in &mut m.layers {
                l.touch_geometry();
            }
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Drops the extent history when the map has switched projection since it was recorded (by a
    /// command, undo or opening a project): its cameras are in the old projection's units.
    fn check_history_projection(&mut self) {
        let code = self.map().projection.code();
        if code != self.history_projection {
            self.back.clear();
            self.forward.clear();
            self.last_camera = None;
            self.history_projection = code;
        }
    }

    /// How many changes can be undone.
    pub fn undo_depth(&self) -> usize {
        self.undo.iter().filter(|u| self.on_stack(u)).count()
    }

    /// Moves every other map to the active map's centre and scale (View › Link Views).
    pub fn sync_linked_views(&mut self) {
        let (c, scale, active) = (self.map().center_lonlat(), self.scale(), self.project.active_map);
        for (i, m) in self.project.maps.iter_mut().enumerate() {
            if i == active || m.projection.is_globe() {
                continue;
            }
            if let Some(mc) = m.to_map(c) {
                m.camera.center = mc;
                m.set_scale(scale);
            }
        }
    }

    /// Records the camera in the previous/next extent history when it settles somewhere new.
    pub fn record_extent(&mut self) {
        if self.project.link_views {
            self.sync_linked_views();
        }
        self.check_history_projection();
        let cam = self.map().camera;
        if let Some(last) = self.last_camera {
            let moved = (last.center.x - cam.center.x).abs() > last.res * 2.0
                || (last.res / cam.res - 1.0).abs() > 0.01
                || rustgis_map::camera::normalize_bearing(last.bearing - cam.bearing).abs() > 0.5
                || (last.pitch - cam.pitch).abs() > 0.5;
            if !moved {
                return;
            }
            self.back.push(last);
            if self.back.len() > 50 {
                self.back.remove(0);
            }
            self.forward.clear();
        }
        self.last_camera = Some(cam);
    }

    pub(crate) fn extent_back(&mut self) -> bool {
        self.check_history_projection();
        let Some(c) = self.back.pop() else { return false };
        let cur = self.map().camera;
        self.forward.push(cur);
        self.map_mut().camera = c;
        self.last_camera = Some(c);
        true
    }

    pub(crate) fn extent_forward(&mut self) -> bool {
        self.check_history_projection();
        let Some(c) = self.forward.pop() else { return false };
        let cur = self.map().camera;
        self.back.push(cur);
        self.map_mut().camera = c;
        self.last_camera = Some(c);
        true
    }

    pub fn can_extent_back(&self) -> bool {
        !self.back.is_empty() && self.map().projection.code() == self.history_projection
    }
    pub fn can_extent_forward(&self) -> bool {
        !self.forward.is_empty() && self.map().projection.code() == self.history_projection
    }

    /// Keeps `active_layer` pointing at a layer of the active map.
    pub(crate) fn fix_active_layer(&mut self) {
        if self.active_layout.is_some_and(|i| i >= self.project.layouts.len()) {
            self.active_layout = None;
        }
        if self.active_report.is_some_and(|i| i >= self.project.reports.len()) {
            self.active_report = None;
        }
        if self.active_presentation.is_some_and(|i| i >= self.project.presentations.len()) {
            self.active_presentation = None;
        }
        if self.active_notebook.is_some_and(|i| i >= self.project.notebooks.len()) {
            self.active_notebook = None;
        }
        let map = self.project.map();
        if let Some(id) = self.active_layer
            && map.layer(id).is_none()
        {
            self.active_layer = map.layers.first().map(|l| l.id);
        }
        if let Some(id) = self.table_layer
            && map.layer(id).is_none()
        {
            self.table_layer = None;
        }
        if self.table_standalone.is_some_and(|id| self.project.table(id).is_none()) {
            self.table_standalone = None;
        }
    }

    /// A colour ramp by name: one of the built-in ones, else one of the project's custom ramps.
    pub fn ramp(&self, name: &str) -> Result<rustgis_map::Ramp, String> {
        rustgis_map::Ramp::from_name(name)
            .or_else(|| {
                self.project
                    .ramps
                    .iter()
                    .find(|r| r.name().eq_ignore_ascii_case(name.trim()))
                    .map(|r| rustgis_map::Ramp::Custom(Box::new(r.clone())))
            })
            .ok_or_else(|| format!("unknown colour scheme '{name}'"))
    }

    /// Resolves a `layer` parameter (name or id), defaulting to the active layer.
    pub fn layer_id(&self, p: &Value) -> Result<u32, String> {
        let map = self.map();
        match p.get("layer") {
            Some(Value::Number(n)) => n
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|id| map.layer(*id).is_some())
                .ok_or_else(|| format!("no layer with id {n}")),
            Some(Value::String(s)) => map
                .layer_by_name(s)
                .map(|l| l.id)
                .or_else(|| s.parse::<u32>().ok().filter(|id| map.layer(*id).is_some()))
                .ok_or_else(|| format!("no layer named '{s}'")),
            _ => {
                self.active_layer.filter(|id| map.layer(*id).is_some()).ok_or_else(|| "select a layer in the Layers pane first".to_string())
            }
        }
    }

    /// Resolves a `table` parameter to a standalone table's position in the project: by its id,
    /// or by text as [`project::find_table`] reads it (a name, else an id).
    pub fn table_index(&self, p: &Value) -> Result<usize, String> {
        let tables = &self.project.tables;
        match p.get("table") {
            Some(Value::Number(n)) => n
                .as_u64()
                .and_then(|id| tables.iter().position(|t| u64::from(t.id) == id))
                .ok_or_else(|| format!("no standalone table with id {n}")),
            Some(Value::String(s)) => project::find_table(tables, s).ok_or_else(|| format!("no standalone table named '{s}'")),
            _ => Err("give the table's name or id (\"table\")".to_string()),
        }
    }

    /// The scale denominator now.
    pub fn scale(&self) -> f64 {
        self.map().scale()
    }

    /// Fits the camera to a lon/lat extent in the current view size.
    pub fn zoom_to(&mut self, e: &rustgis_geom::Extent) {
        let (w, h) = self.view_size;
        let m = self.map_mut();
        if m.projection.is_globe() {
            rustgis_map::Globe::fit(&mut m.camera, e, w, h);
        } else {
            m.fit_lonlat(e, w, h);
        }
        self.record_extent();
    }

    pub fn title(&self) -> String {
        self.project.name.clone()
    }
}
