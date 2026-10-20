//! The Observatory bridge, engine half (Syngle-Ecosystem #67, ADR 0010).
//!
//! One schema, one version, three peers: SyngleCode's React surface (the client), the engine's
//! canvas page (the view half) and this process (the engine half). The canonical document is
//! [`SCHEMA_TEXT`] — `bridge/observatory-bridge.schema.json` — and every name, required field and
//! enum this module accepts is read back out of it, so the code and the schema cannot drift apart
//! without a test failing:
//!
//! * [`engine_command_names`] is the schema's list, and [`EngineCommand::parse`] refuses anything
//!   that is not on it.
//! * [`check_params`] validates a command's members against that command's own `required`,
//!   `properties`, `type` and `enum`, and refuses members the schema does not name.
//! * [`check_version`] refuses a peer whose `protocolVersion` is not [`PROTOCOL_VERSION`], with the
//!   `version-mismatch` error the client turns into a visible state: a bridge that cannot agree on
//!   a version serves nothing, rather than guessing.
//!
//! The engine's five commands are the spec's bridge section, engine side: `dataset.open`,
//! `dataset.query`, `catchment`, `command` (the fork's own command registry, unchanged) and `mcp`
//! (one JSON-RPC message in, the fork's own MCP server's reply out). No new GIS code lives here:
//! every one of them dispatches into the fork.

use std::sync::OnceLock;

use rustgis_engine::Session;
use serde_json::{Value, json};

/// The bridge version this engine speaks. A peer must send the same number.
pub const PROTOCOL_VERSION: u64 = 1;

/// The canonical schema: the one place the protocol is written down.
pub const SCHEMA_TEXT: &str = include_str!("../bridge/observatory-bridge.schema.json");

/// The most rows one `dataset.query` returns, whatever it asks for.
pub const MAX_QUERY_LIMIT: usize = 1_000;

/// The schema, parsed once.
pub fn schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| serde_json::from_str(SCHEMA_TEXT).unwrap_or(Value::Null))
}

/// The version the schema document declares. Its own `protocolVersion` has to be
/// [`PROTOCOL_VERSION`]; a test holds the two together.
pub fn schema_version() -> Option<u64> {
    schema().get("protocolVersion").and_then(Value::as_u64)
}

/// A refusal, in the schema's own `error` shape: a code the client can switch on and a message a
/// person can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeError {
    pub code: &'static str,
    pub message: String,
}

impl BridgeError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    /// The peer speaks another version (or named none).
    pub fn version_mismatch(got: Option<u64>) -> Self {
        match got {
            Some(got) => {
                Self::new("version-mismatch", format!("the bridge speaks protocol version {PROTOCOL_VERSION}; this peer asked for {got}"))
            }
            None => Self::new("version-mismatch", format!("the peer sent no protocolVersion; this bridge speaks {PROTOCOL_VERSION}")),
        }
    }

    /// A command arrived before the peer said hello: the bridge does not guess a version.
    pub fn no_handshake() -> Self {
        Self::new("no-handshake", "send {\"cmd\":\"bridge\",\"bridge\":\"hello\",\"protocolVersion\":1} before any command")
    }

    /// A command name the schema does not have.
    pub fn unknown_command(name: &str) -> Self {
        Self::new("unknown-command", format!("the bridge has no engine command '{name}'"))
    }

    pub fn bad_params(message: impl Into<String>) -> Self {
        Self::new("bad-params", message)
    }

    /// A real command the fork cannot do yet: refused by name, never faked.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new("unsupported", message)
    }

    /// The fork's own refusal (a command's `Err`, a dataset that will not load).
    pub fn engine(message: impl Into<String>) -> Self {
        Self::new("engine-error", message)
    }

    /// The `error` event's payload: `{"type": "error", …}`.
    pub fn payload(&self, id: &Value, name: Option<&str>) -> Value {
        json!({"type": "error", "id": id, "name": name, "code": self.code, "message": self.message})
    }

    /// The whole `bridge.error` line, for a peer that asked for something this engine refused.
    pub fn line(&self, id: &Value, name: Option<&str>) -> Value {
        json!({
            "event": "bridge.error",
            "protocolVersion": PROTOCOL_VERSION,
            "payload": self.payload(id, name),
        })
    }
}

/// Refuses a message whose `protocolVersion` is not this bridge's. A message with no version is
/// refused too: a peer that does not say which protocol it speaks has not joined the bridge.
pub fn check_version(msg: &Value) -> Result<(), BridgeError> {
    let got = msg.get("protocolVersion").and_then(Value::as_u64);
    match got {
        Some(v) if v == PROTOCOL_VERSION => Ok(()),
        other => Err(BridgeError::version_mismatch(other)),
    }
}

/// The schema's engine commands, in the order they are written down.
pub fn engine_command_names() -> Vec<String> {
    engine_commands().keys().cloned().collect()
}

fn engine_commands() -> &'static serde_json::Map<String, Value> {
    static EMPTY: OnceLock<serde_json::Map<String, Value>> = OnceLock::new();
    static COMMANDS: OnceLock<&'static serde_json::Map<String, Value>> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        schema()
            .get("engine")
            .and_then(|e| e.get("commands"))
            .and_then(Value::as_object)
            .unwrap_or_else(|| EMPTY.get_or_init(serde_json::Map::new))
    })
}

/// One engine command's schema entry, or `None` when the schema has no such command.
pub fn engine_command_spec(name: &str) -> Option<&'static Value> {
    engine_commands().get(name)
}

/// The view's commands, from the schema: what the canvas page answers (the client needs the list
/// without reading the whole document).
pub fn view_command_names() -> Vec<String> {
    schema().get("view").and_then(|v| v.get("commands")).and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

/// Whether the schema's engine section has this event (`progress`, `result`, `error`).
pub fn engine_event_spec(name: &str) -> Option<&'static Value> {
    schema().get("engine").and_then(|e| e.get("events")).and_then(|e| e.get(name))
}

fn kind_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// One member against its schema property: `enum`, `const`, `type` (one or a list).
fn check_value(prop: &Value, value: &Value, at: &str) -> Result<(), BridgeError> {
    if let Some(allowed) = prop.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return Err(BridgeError::bad_params(format!("{at} must be one of {allowed:?}, not {value}")));
    }
    if let Some(want) = prop.get("const")
        && want != value
    {
        return Err(BridgeError::bad_params(format!("{at} must be {want}, not {value}")));
    }
    match prop.get("type") {
        Some(Value::String(kind)) if !kind_matches(kind, value) => {
            Err(BridgeError::bad_params(format!("{at} must be a {kind}, not {value}")))
        }
        Some(Value::Array(kinds)) => {
            let ok = kinds.iter().filter_map(Value::as_str).any(|k| kind_matches(k, value));
            if ok { Ok(()) } else { Err(BridgeError::bad_params(format!("{at} has the wrong type: {value}"))) }
        }
        _ => Ok(()),
    }
}

/// A command's members against its schema entry: every `required` member is there, every member
/// there is one the schema names, and each one has the right type and enum. `params` may carry the
/// command's `name` member, which is the envelope's business and is skipped here.
pub fn check_params(spec: &Value, params: &Value) -> Result<(), BridgeError> {
    let Some(object) = params.as_object() else {
        return Err(BridgeError::bad_params("a command's members must be an object"));
    };
    let properties = spec.get("properties").and_then(Value::as_object);
    for required in spec.get("required").and_then(Value::as_array).into_iter().flatten() {
        let Some(key) = required.as_str() else { continue };
        if !object.contains_key(key) {
            return Err(BridgeError::bad_params(format!("'{key}' is required")));
        }
    }
    for (key, value) in object {
        if key == "name" {
            continue;
        }
        let Some(prop) = properties.and_then(|p| p.get(key)) else {
            return Err(BridgeError::bad_params(format!("'{key}' is not part of this command")));
        };
        check_value(prop, value, key)?;
    }
    // A nested object can name its own required members (`camera` needs `center` and `res`).
    for (key, prop) in properties.into_iter().flatten() {
        let (Some(value), Some(inner)) = (object.get(key), prop.get("properties")) else { continue };
        for required in prop.get("required").and_then(Value::as_array).into_iter().flatten() {
            let Some(name) = required.as_str() else { continue };
            if value.get(name).is_none() {
                return Err(BridgeError::bad_params(format!("{key}.{name} is required")));
            }
            if let Some(spec) = inner.get(name) {
                check_value(spec, &value[name], &format!("{key}.{name}"))?;
            }
        }
    }
    Ok(())
}

/// How a dataset is named to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenKind {
    /// An address the view reads in place.
    Url,
    /// A file on this machine.
    Path,
}

impl OpenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Path => "path",
        }
    }
}

/// What kind of catchment was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchmentMode {
    /// Every feature within `radiusM` of the point.
    Radius,
    /// Road-network travel time: not in this fork yet, refused by name.
    DriveTime,
}

/// One engine command, parsed and validated.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineCommand {
    DatasetOpen { target: String, kind: OpenKind, name: Option<String>, zoom: bool },
    DatasetQuery { dataset: String, filter: Option<String>, limit: usize, offset: usize },
    Catchment { dataset: String, mode: CatchmentMode, lon: f64, lat: f64, radius_m: f64 },
    Command { id: String, params: Value },
    Mcp { message: String },
}

impl EngineCommand {
    /// The schema name this command is.
    pub fn name(&self) -> &'static str {
        match self {
            Self::DatasetOpen { .. } => "dataset.open",
            Self::DatasetQuery { .. } => "dataset.query",
            Self::Catchment { .. } => "catchment",
            Self::Command { .. } => "command",
            Self::Mcp { .. } => "mcp",
        }
    }

    /// Reads one command from the schema's shape: `{"name": …, …members}`. The name must be one the
    /// schema lists, and its members must pass [`check_params`]; anything else is refused with the
    /// reason, never coerced.
    pub fn parse(command: &Value) -> Result<Self, BridgeError> {
        let name = command.get("name").and_then(Value::as_str).ok_or_else(|| BridgeError::bad_params("a bridge command needs a 'name'"))?;
        let spec = engine_command_spec(name).ok_or_else(|| BridgeError::unknown_command(name))?;
        check_params(spec, command)?;
        let text = |key: &str| command.get(key).and_then(Value::as_str).map(str::to_string);
        let number = |key: &str| command.get(key).and_then(Value::as_f64);
        Ok(match name {
            "dataset.open" => {
                let target = text("target").ok_or_else(|| BridgeError::bad_params("'target' is required"))?;
                Self::DatasetOpen {
                    target,
                    kind: match text("kind").as_deref() {
                        Some("path") => OpenKind::Path,
                        _ => OpenKind::Url,
                    },
                    name: text("layerName"),
                    zoom: command.get("zoom").and_then(Value::as_bool).unwrap_or(true),
                }
            }
            "dataset.query" => Self::DatasetQuery {
                dataset: text("dataset").ok_or_else(|| BridgeError::bad_params("'dataset' is required"))?,
                filter: text("where").filter(|w| !w.trim().is_empty()),
                limit: number("limit").map(|n| n.clamp(0.0, MAX_QUERY_LIMIT as f64) as usize).unwrap_or(20),
                offset: number("offset").map(|n| n.max(0.0) as usize).unwrap_or(0),
            },
            "catchment" => {
                let mode = match text("mode").as_deref() {
                    Some("drive-time") => CatchmentMode::DriveTime,
                    _ => CatchmentMode::Radius,
                };
                Self::Catchment {
                    dataset: text("dataset").ok_or_else(|| BridgeError::bad_params("'dataset' is required"))?,
                    mode,
                    lon: number("lon").ok_or_else(|| BridgeError::bad_params("'lon' is required"))?,
                    lat: number("lat").ok_or_else(|| BridgeError::bad_params("'lat' is required"))?,
                    radius_m: number("radiusM").unwrap_or(5_000.0).clamp(1.0, 500_000.0),
                }
            }
            "command" => Self::Command {
                id: text("id").ok_or_else(|| BridgeError::bad_params("'id' is required"))?,
                params: command.get("params").cloned().filter(|p| p.is_object()).unwrap_or_else(|| json!({})),
            },
            "mcp" => Self::Mcp { message: text("message").ok_or_else(|| BridgeError::bad_params("'message' is required"))? },
            other => return Err(BridgeError::unknown_command(other)),
        })
    }
}

/// The engine's state, in the schema's shape: what the client shows after every command.
pub fn state(session: &Session) -> Value {
    let layers = session.project.map().layers.iter().map(|l| Value::String(l.name.clone())).collect::<Vec<_>>();
    let features: usize = session.project.map().layers.iter().map(|l| l.dataset.features.len()).sum();
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "layers": session.project.map().layers.len(),
        "features": features,
        "datasets": layers,
        "revision": session.revision,
        // The fork's own history, so a client can tell an edit from a read (#67). `undoDepth` is how
        // many steps undo would take back, and `undoLabel` is the top of them: the client records a
        // step of its own only when the depth grew, and rolls its cursor back when it did not move.
        "undoDepth": session.undo_depth(),
        "undoLabel": session.undo_label(),
        "redoDepth": session.redo_depth(),
    })
}

fn layer_id(session: &Session, name: &str) -> Result<u32, BridgeError> {
    session.layer_id(&json!({ "layer": name })).map_err(BridgeError::engine)
}

/// Runs one command against the fork and answers with its value. State comes after, from
/// [`state`], so a value never has to carry the whole project.
pub fn run(session: &mut Session, command: &EngineCommand) -> Result<Value, BridgeError> {
    match command {
        EngineCommand::DatasetOpen { target, kind, name, zoom } => {
            let mut params = json!({ "zoom": zoom });
            if let Some(name) = name {
                params["name"] = json!(name);
            }
            match kind {
                OpenKind::Url => {
                    params["url"] = json!(target);
                    session.run("layer.addUrl", &params).map_err(BridgeError::engine)
                }
                OpenKind::Path => {
                    params["path"] = json!(target);
                    session.run("layer.addData", &params).map_err(BridgeError::engine)
                }
            }
        }
        EngineCommand::DatasetQuery { dataset, filter, limit, offset } => {
            let id = layer_id(session, dataset)?;
            let layer = session.map().layer(id).ok_or_else(|| BridgeError::engine(format!("no layer '{dataset}'")))?;
            if layer.is_raster() {
                return Err(BridgeError::unsupported(format!("'{dataset}' is a raster layer; it has no rows")));
            }
            let rows: Vec<usize> = match filter {
                Some(w) => rustgis_engine::data::query::select(w, &layer.dataset).map_err(BridgeError::engine)?,
                None => (0..layer.dataset.features.len()).collect(),
            };
            let out: Vec<Value> = rows
                .iter()
                .skip(*offset)
                .take((*limit).min(MAX_QUERY_LIMIT))
                .filter_map(|&r| {
                    let f = layer.dataset.features.get(r)?;
                    let mut row = serde_json::Map::new();
                    row.insert("_row".into(), json!(r));
                    for (i, field) in layer.dataset.fields.iter().enumerate() {
                        let value = match f.attrs.get(i) {
                            Some(rustgis_engine::data::Value::Int(n)) => json!(n),
                            Some(rustgis_engine::data::Value::Float(x)) => json!(x),
                            Some(rustgis_engine::data::Value::Bool(b)) => json!(b),
                            Some(rustgis_engine::data::Value::Text(t)) => json!(t),
                            _ => Value::Null,
                        };
                        row.insert(field.name.clone(), value);
                    }
                    Some(Value::Object(row))
                })
                .collect();
            Ok(json!({
                "dataset": layer.name,
                "total": layer.dataset.features.len(),
                "matched": rows.len(),
                "offset": offset,
                "rows": out,
            }))
        }
        EngineCommand::Catchment { dataset, mode, lon, lat, radius_m } => {
            let id = layer_id(session, dataset)?;
            match mode {
                CatchmentMode::DriveTime => Err(BridgeError::unsupported(
                    "a drive-time catchment needs road-network routing, which this fork does not have yet; ask for mode 'radius'",
                )),
                CatchmentMode::Radius => {
                    let (rows, count) = {
                        let layer = session.map().layer(id).ok_or_else(|| BridgeError::engine(format!("no layer '{dataset}'")))?;
                        let all: Vec<usize> = (0..layer.dataset.features.len()).collect();
                        let hits = rustgis_engine::analysis::network::within(
                            &layer.dataset,
                            &all,
                            rustgis_engine::geom::Coord::new(*lon, *lat),
                            *radius_m,
                        );
                        let rows: Vec<usize> = hits.iter().map(|(r, _)| *r).collect();
                        (rows, hits.len())
                    };
                    // The catchment becomes the selection, which is the agent's scope (ADR 0009).
                    session.run("select.rows", &json!({ "layer": dataset, "rows": rows })).map_err(BridgeError::engine)?;
                    Ok(json!({
                        "mode": "radius",
                        "dataset": dataset,
                        "center": {"lon": lon, "lat": lat},
                        "radiusM": radius_m,
                        "count": count,
                        "rows": rows,
                    }))
                }
            }
        }
        EngineCommand::Command { id, params } => session.run(id, params).map_err(BridgeError::engine),
        EngineCommand::Mcp { message } => {
            // This session *is* the project the person has open, as the desktop's relay is, so the
            // pass-through is a relay's: commands that replace the project stay theirs, and a tool
            // call that edits the project is an edit like any other (it grows the undo history the
            // client reads). Only the app's own renderer can reach this line, so the exposure is no
            // wider than `command`.
            let server = rustgis_assistant::mcp::Server { users_project: true, ..rustgis_assistant::mcp::Server::new(false) };
            match server.handle(session, message) {
                Some(reply) => serde_json::from_str(&reply).map_err(|e| BridgeError::engine(format!("the MCP reply was not JSON: {e}"))),
                // A notification has no reply; the bridge says so rather than inventing one.
                None => Ok(json!({ "reply": null, "notification": true })),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// The engine commands the ticket names, one by one: a removal has to be a deliberate edit here.
    const SPEC_COMMANDS: [&str; 5] = ["dataset.open", "dataset.query", "catchment", "command", "mcp"];
    /// The view commands the spec's bridge section names.
    const SPEC_VIEW_COMMANDS: [&str; 7] = ["layers", "style", "camera", "tool", "selection", "theme", "render-to-png"];

    #[test]
    fn the_schema_declares_the_version_the_code_speaks() {
        assert_eq!(schema_version(), Some(PROTOCOL_VERSION), "the schema and the code are one version");
        assert!(check_version(&json!({"protocolVersion": PROTOCOL_VERSION})).is_ok());
    }

    /// Every command the code can parse is one the schema names, and every command the schema names
    /// is one the code can parse and run: the two lists are the same list.
    #[test]
    fn the_code_and_the_schema_agree_on_every_engine_command() {
        let mut in_schema = engine_command_names();
        in_schema.sort();
        let mut in_code = SPEC_COMMANDS.to_vec();
        in_code.sort();
        assert_eq!(in_schema, in_code, "bridge/observatory-bridge.schema.json and EngineCommand");

        for name in SPEC_COMMANDS {
            let spec = engine_command_spec(name).unwrap_or_else(|| panic!("the schema has {name}"));
            // A minimal instance of each, from the schema's own required members, parses back to the
            // same command name: the code's name and the schema's key cannot drift apart.
            let mut minimal = serde_json::Map::new();
            minimal.insert("name".into(), json!(name));
            for required in spec.get("required").and_then(Value::as_array).into_iter().flatten() {
                let key = required.as_str().unwrap_or("");
                minimal.insert(key.into(), placeholder(spec, key));
            }
            let parsed = EngineCommand::parse(&Value::Object(minimal.clone()))
                .unwrap_or_else(|e| panic!("{name} parses from its own required members: {e:?}"));
            assert_eq!(parsed.name(), name, "{minimal:?}");
        }
    }

    /// A value a required member will accept: its `enum`'s first entry, or its `type`'s zero value.
    fn placeholder(spec: &Value, key: &str) -> Value {
        let prop = spec.get("properties").and_then(|p| p.get(key)).cloned().unwrap_or(Value::Null);
        if let Some(first) = prop.get("enum").and_then(Value::as_array).and_then(|a| a.first()) {
            return first.clone();
        }
        match prop.get("type") {
            Some(Value::String(kind)) => match kind.as_str() {
                "number" => json!(0),
                "boolean" => json!(false),
                "array" => json!([]),
                "object" => json!({}),
                _ => json!(""),
            },
            _ => json!(""),
        }
    }

    #[test]
    fn the_view_half_is_the_specs_seven_commands_and_they_all_validate() {
        let mut names = view_command_names();
        names.sort();
        let mut spec = SPEC_VIEW_COMMANDS.map(str::to_string).to_vec();
        spec.sort();
        assert_eq!(names, spec, "the view's command list");
        // The state and event halves the React surface reads are declared too.
        let view = &schema()["view"];
        for key in ["layers", "active", "camera", "tool", "selection", "theme"] {
            assert!(view["state"]["required"].as_array().unwrap().contains(&json!(key)), "{key} in the view state");
        }
        for event in ["camera-changed", "selection-changed", "feature-clicked", "tool-result", "frame-stats"] {
            assert!(view["events"].get(event).is_some(), "{event} is a view event");
        }
        for event in ["progress", "result", "error"] {
            assert!(engine_event_spec(event).is_some(), "{event} is an engine event");
        }
    }

    #[test]
    fn members_the_schema_does_not_name_or_know_are_refused() {
        let query = engine_command_spec("dataset.query").unwrap();
        assert!(check_params(query, &json!({"dataset": "Countries"})).is_ok());
        // Not in this command.
        let err = check_params(query, &json!({"dataset": "Countries", "dropTable": true})).unwrap_err();
        assert_eq!(err.code, "bad-params");
        assert!(err.message.contains("dropTable"), "{err:?}");
        // Required and missing.
        let err = check_params(query, &json!({"limit": 5})).unwrap_err();
        assert!(err.message.contains("dataset"), "{err:?}");
        // The wrong type.
        let err = check_params(query, &json!({"dataset": 5})).unwrap_err();
        assert!(err.message.contains("string"), "{err:?}");
        // An enum outside its list.
        let catchment = engine_command_spec("catchment").unwrap();
        let err = check_params(catchment, &json!({"dataset": "Cities", "lon": 1.0, "lat": 2.0, "mode": "isochrone"})).unwrap_err();
        assert!(err.message.contains("radius"), "{err:?}");
        // A nested required member.
        let camera = &schema()["view"]["commands"]["camera"];
        assert!(check_params(camera, &json!({"camera": {"center": [0, 0], "res": 10}})).is_ok());
        let err = check_params(camera, &json!({"camera": {"center": [0, 0]}})).unwrap_err();
        assert!(err.message.contains("camera.res"), "{err:?}");
    }

    /// The refusal codes the code produces are the ones the schema's `error` event declares: the
    /// client switches on them, so a new code that the schema does not carry is a drift.
    #[test]
    fn every_error_code_the_code_uses_is_in_the_schema() {
        let declared: Vec<String> = engine_event_spec("error").unwrap()["properties"]["code"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        for code in [
            BridgeError::version_mismatch(Some(2)).code,
            BridgeError::version_mismatch(None).code,
            BridgeError::no_handshake().code,
            BridgeError::unknown_command("x").code,
            BridgeError::bad_params("x").code,
            BridgeError::unsupported("x").code,
            BridgeError::engine("x").code,
        ] {
            assert!(declared.iter().any(|d| d == code), "'{code}' is in the schema: {declared:?}");
        }
    }

    #[test]
    fn a_version_mismatch_names_both_versions_and_is_an_error_line() {
        let err = check_version(&json!({"protocolVersion": 7})).unwrap_err();
        assert_eq!(err.code, "version-mismatch");
        assert!(err.message.contains('7') && err.message.contains(&PROTOCOL_VERSION.to_string()));
        let line = err.line(&json!(4), Some("dataset.query"));
        assert_eq!(line["event"], "bridge.error");
        assert_eq!(line["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(line["payload"]["id"], 4);
        assert_eq!(line["payload"]["name"], "dataset.query");
    }
}
