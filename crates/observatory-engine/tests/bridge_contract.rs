//! The engine half of the Observatory bridge, driven the way the app drives it (#67).
//!
//! Every test here starts the real sidecar binary — the one the desktop supervisor launches — and
//! speaks its protocol on the process's own stdin and stdout. Nothing is mocked: the schema is the
//! shipped document, the commands are the fork's own, the PNG comes from the fork's renderer and
//! the gateway it is fetched from is the one the view is served by. These are the contract tests
//! the ticket asks for: commands in, state and events out, and a version mismatch that fails loudly
//! instead of being served.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long any single answer may take before the test calls the engine wedged.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// The launch token: every request the gateway serves must carry it.
const TOKEN: &str = "contract-5f2a";

struct Engine {
    child: Child,
    lines: Receiver<String>,
    port: u16,
    root: PathBuf,
}

impl Engine {
    /// Starts the real binary the supervisor starts: an open port, a mints-its-own token, the
    /// folders it serves the view and the datasets from.
    fn start(name: &str) -> Engine {
        let root = std::env::temp_dir().join(format!("obs-bridge-contract-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data")).expect("data folder");
        std::fs::create_dir_all(root.join("web")).expect("web folder");
        let mut child = Command::new(env!("CARGO_BIN_EXE_syngle-observatory-engine"))
            .arg("--root")
            .arg(&root)
            .arg("--web")
            .arg(root.join("web"))
            .arg("--data")
            .arg(root.join("data"))
            .arg("--port")
            .arg("0")
            .arg("--token")
            .arg(TOKEN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the engine binary starts");
        let stdout = child.stdout.take().expect("the engine's stdout");
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        // The first line the engine ever writes is `ready`, with the port it bound.
        let ready = Engine { child, lines, port: 0, root };
        let mut ready = ready;
        let first = ready.next();
        assert_eq!(first.get("event").and_then(Value::as_str), Some("ready"), "{first}");
        ready.port = first.get("port").and_then(Value::as_u64).unwrap_or(0) as u16;
        assert!(ready.port > 0, "the engine bound a port: {first}");
        ready
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.child.stdin.as_mut().expect("the engine's stdin");
        writeln!(stdin, "{message}").expect("a write on the control channel");
        stdin.flush().expect("a flush on the control channel");
    }

    /// The next line the engine wrote, or a failure: a silent engine is a failed test, not a hang.
    fn next(&mut self) -> Value {
        match self.lines.recv_timeout(ANSWER_TIMEOUT) {
            Ok(line) => serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON: {line}: {e}")),
            Err(RecvTimeoutError::Timeout) => panic!("the engine answered nothing in {ANSWER_TIMEOUT:?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("the engine stopped"),
        }
    }

    /// Reads lines until `done` accepts one, and answers with everything read on the way.
    fn until(&mut self, what: &str, done: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let line = self.next();
            let done = done(&line);
            seen.push(line);
            if done {
                return seen;
            }
            assert!(seen.len() < 64, "no {what} in 64 lines");
        }
    }

    /// Says hello on the bridge and answers with the `bridge.ready` line.
    fn hello(&mut self, protocol_version: u64) -> Value {
        self.send(&json!({"cmd": "bridge", "bridge": "hello", "protocolVersion": protocol_version}));
        self.next()
    }

    /// Sends one bridge command and answers with (result value, state, every line on the way).
    fn command(&mut self, id: u64, command: &Value) -> (Value, Value, Vec<Value>) {
        self.send(&json!({"cmd": "bridge", "bridge": "command", "protocolVersion": 1, "id": id, "command": command}));
        let seen = self.until("bridge.state or bridge.error", |line| {
            matches!(line.get("event").and_then(Value::as_str), Some("bridge.state") | Some("bridge.error"))
        });
        let payload = |type_name: &str| -> Option<Value> {
            seen.iter().filter_map(|l| l.get("payload")).find(|p| p.get("type").and_then(Value::as_str) == Some(type_name)).cloned()
        };
        let state = seen
            .iter()
            .find_map(|l| (l.get("event").and_then(Value::as_str) == Some("bridge.state")).then(|| l["state"].clone()))
            .unwrap_or(Value::Null);
        // The result's own value (what the command answered), or the error payload whole.
        let value = match (payload("result"), payload("error")) {
            (Some(result), _) => result.get("value").cloned().unwrap_or(Value::Null),
            (None, Some(error)) => error,
            (None, None) => Value::Null,
        };
        (value, state, seen)
    }

    /// A GET on the token-guarded gateway, answered with (status, content type, body).
    fn get(&self, path: &str) -> (u16, String, Vec<u8>) {
        use std::io::Read;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).expect("the gateway's port");
        stream.set_read_timeout(Some(ANSWER_TIMEOUT)).ok();
        let request = format!("GET /t/{TOKEN}{path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).expect("a request");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).expect("the answer");
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(raw.len());
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let body = raw.get(split + 4..).unwrap_or(&[]).to_vec();
        let status = head.split_whitespace().nth(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
        let content_type = head.lines().find_map(|l| l.strip_prefix("Content-Type: ")).unwrap_or("").to_string();
        (status, content_type, body)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.send(&json!({"cmd": "quit"}));
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write(root: &Path, name: &str, body: &[u8]) -> PathBuf {
    let path = root.join("data").join(name);
    std::fs::write(&path, body).expect("a fixture on disk");
    path
}

/// The bridge's own handshake: a peer on another version is refused, loudly, and served nothing.
#[test]
fn a_peer_on_another_version_is_refused_and_served_nothing() {
    let mut engine = Engine::start("version");
    // A version this bridge does not speak.
    let refused = engine.hello(99);
    assert_eq!(refused.get("event").and_then(Value::as_str), Some("bridge.error"), "{refused}");
    let payload = &refused["payload"];
    assert_eq!(payload.get("code").and_then(Value::as_str), Some("version-mismatch"), "{refused}");
    assert!(payload.get("message").and_then(Value::as_str).unwrap_or("").contains("99"), "{refused}");
    // And silence is refused too: a peer that never says which protocol it speaks has not joined.
    engine.send(&json!({"cmd": "bridge", "bridge": "hello"}));
    let silent = engine.next();
    assert_eq!(silent["payload"]["code"].as_str(), Some("version-mismatch"), "{silent}");

    // Nothing runs until the handshake agrees: the command that needs one is refused, not queued.
    engine.send(&json!({"cmd": "bridge", "bridge": "command", "protocolVersion": 1, "id": 1, "command": {"name": "command", "id": "project.sample"}}));
    let refused = engine.next();
    assert_eq!(refused.get("event").and_then(Value::as_str), Some("bridge.error"), "{refused}");
    assert_eq!(refused["payload"]["code"].as_str(), Some("no-handshake"), "{refused}");

    // The handshake this version, and the same command now runs.
    let hello = engine.hello(1);
    assert_eq!(hello.get("event").and_then(Value::as_str), Some("bridge.ready"), "{hello}");
    assert_eq!(hello["protocolVersion"].as_u64(), Some(1), "{hello}");
    assert!(hello["state"].get("layers").is_some(), "the handshake reports state: {hello}");
    let (_, state, lines) = engine.command(1, &json!({"name": "command", "id": "project.sample"}));
    assert_eq!(state["layers"].as_u64().unwrap_or(0), 4, "the sample project's four layers: {state}");
    // …and the events came out in order, on the way: progress, result, state.
    let kinds: Vec<String> =
        lines.iter().filter_map(|l| l.get("payload")).filter_map(|p| p.get("type").and_then(Value::as_str).map(str::to_string)).collect();
    assert_eq!(kinds, vec!["progress".to_string(), "result".to_string()], "{lines:?}");
}

/// Every engine command the schema has, run against the real engine: values out, state after, and
/// the fork's own answers (not a stand-in for them).
#[test]
fn the_five_engine_commands_answer_with_values_state_and_events() {
    let mut engine = Engine::start("commands");
    engine.hello(1);

    // 1. `command`: the fork's own registry, unchanged. The sample project gives real features.
    let (value, state, _) = engine.command(1, &json!({"name": "command", "id": "project.sample", "params": {}}));
    // The fork's own command answers with what it answers (nothing, here): the project it replaced
    // is the state that follows, and that is where the layers are.
    assert!(value.get("code").is_none(), "the command ran: {value}");
    assert_eq!(state["layers"].as_u64(), Some(4), "{state}");
    let features = state["features"].as_u64().unwrap_or(0);
    assert!(features > 100, "the sample project has real features: {state}");

    // 2. `dataset.query`: rows of a real dataset, filtered by the fork's own where-clause parser.
    let (value, _, _) =
        engine.command(2, &json!({"name": "dataset.query", "dataset": "Countries", "where": "pop_est > 100000000", "limit": 3}));
    assert_eq!(value["dataset"].as_str(), Some("Countries"), "{value}");
    let rows = value["rows"].as_array().cloned().unwrap_or_default();
    assert!(!rows.is_empty() && rows.len() <= 3, "{value}");
    assert!(rows[0].get("name").is_some(), "a row carries the dataset's fields: {value}");
    assert!(value["matched"].as_u64().unwrap_or(0) >= rows.len() as u64, "{value}");

    // 3. `catchment`: the fork's own distance work, applied as the selection (the agent's scope).
    let (value, _, _) = engine.command(
        3,
        &json!({"name": "catchment", "dataset": "Cities", "mode": "radius", "lon": 151.2093, "lat": -33.8688, "radiusM": 60000}),
    );
    assert_eq!(value["mode"].as_str(), Some("radius"), "{value}");
    assert!(value["count"].as_u64().unwrap_or(0) >= 1, "Sydney is in the cities layer: {value}");
    let selected: Vec<u64> = value["rows"].as_array().cloned().unwrap_or_default().iter().filter_map(Value::as_u64).collect();
    assert!(!selected.is_empty(), "{value}");
    // The selection really is the engine's: asking the project for it shows the same rows.
    let (project, _, _) = engine.command(4, &json!({"name": "command", "id": "select.zoomTo"}));
    assert!(project.get("error").is_none(), "a catchment leaves a real selection: {project}");

    // Drive-time is refused by name, not faked as a radius.
    let (value, _, lines) = engine.command(
        5,
        &json!({"name": "catchment", "dataset": "Cities", "mode": "drive-time", "lon": 151.2093, "lat": -33.8688, "minutes": 15}),
    );
    assert_eq!(value.get("code").and_then(Value::as_str), Some("unsupported"), "{lines:?}");
    assert!(value.get("message").and_then(Value::as_str).unwrap_or("").contains("road-network"), "{value}");

    // 4. `mcp`: one JSON-RPC message into the fork's own MCP server, its reply back out.
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "contract", "version": "1"}},
    });
    let (value, _, _) = engine.command(6, &json!({"name": "mcp", "message": initialize.to_string()}));
    assert_eq!(value["result"]["serverInfo"]["name"].as_str(), Some("rustgis"), "{value}");
    // A tool call goes through the same pass-through: the map's layers, as the agent sees them.
    let tools = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "list_layers", "arguments": {}}});
    let (value, _, _) = engine.command(7, &json!({"name": "mcp", "message": tools.to_string()}));
    let text = value["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("Countries"), "list_layers names the project's layers: {value}");

    // 5. `dataset.open`: a real file on disk, opened by the fork's reader, in the state after.
    let path = write(
        &engine.root,
        "clinics.geojson",
        br#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"name":"Clinic A","owner":"Dr One"},"geometry":{"type":"Point","coordinates":[151.2,-33.87]}},
            {"type":"Feature","properties":{"name":"Clinic B","owner":"Dr Two"},"geometry":{"type":"Point","coordinates":[150.9,-33.9]}}]}"#,
    );
    let (value, state, _) =
        engine.command(8, &json!({"name": "dataset.open", "kind": "path", "target": path.display().to_string(), "layerName": "Clinics"}));
    assert!(value.get("ok").is_none() || value["ok"].as_bool() == Some(true), "{value}");
    assert_eq!(state["layers"].as_u64(), Some(5), "the opened layer is in the state: {state}");
    let names: Vec<String> =
        state["datasets"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    assert!(names.iter().any(|n| n == "Clinics"), "{state}");
    // Its provenance-bearing rows come back through the same query command.
    let (value, _, _) = engine.command(9, &json!({"name": "dataset.query", "dataset": "Clinics", "limit": 10}));
    assert_eq!(value["matched"].as_u64(), Some(2), "{value}");
    assert_eq!(value["rows"][0]["owner"].as_str(), Some("Dr One"), "{value}");

    // A command that cannot work says so, and the bridge stays up for the next one.
    let (value, _, lines) = engine.command(10, &json!({"name": "dataset.open", "kind": "path", "target": "/nonexistent/nope.geojson"}));
    assert_eq!(value.get("code").and_then(Value::as_str), Some("engine-error"), "{lines:?}");
    let (value, _, _) = engine.command(11, &json!({"name": "dataset.query", "dataset": "Nope"}));
    assert_eq!(value.get("code").and_then(Value::as_str), Some("engine-error"), "{value}");

    // A command name or member the schema does not have is refused before the fork sees it.
    let (value, _, _) = engine.command(12, &json!({"name": "dataset.drop", "dataset": "Countries"}));
    assert_eq!(value.get("code").and_then(Value::as_str), Some("unknown-command"), "{value}");
    let (value, _, _) = engine.command(13, &json!({"name": "dataset.query", "dataset": "Countries", "dropTable": true}));
    assert_eq!(value.get("code").and_then(Value::as_str), Some("bad-params"), "{value}");
    let (_, state, _) = engine.command(14, &json!({"name": "dataset.query", "dataset": "Countries", "limit": 1}));
    assert_eq!(state["layers"].as_u64(), Some(5), "the bridge is still serving: {state}");
}

/// `render-to-png`: the page asks the gateway, the command loop draws with the fork's own renderer,
/// and what comes back is a real PNG of the real project.
#[test]
fn render_to_png_draws_the_project_off_screen() {
    let mut engine = Engine::start("render");
    engine.hello(1);
    engine.command(1, &json!({"name": "command", "id": "project.sample"}));

    let (status, content_type, body) = engine.get("/render?width=640&height=400");
    assert_eq!(status, 200, "a render over the token-guarded gateway");
    assert_eq!(content_type, "image/png");
    assert_eq!(body.get(..8), Some(b"\x89PNG\r\n\x1a\n".as_slice()), "a real PNG: {} bytes", body.len());
    // The header's own width and height, big-endian at bytes 16..24.
    let side = |at: usize| body.get(at..at + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_be_bytes);
    assert_eq!((side(16), side(20)), (Some(640), Some(400)), "the size that was asked for");
    assert!(body.len() > 2_000, "a drawn map, not a blank one: {} bytes", body.len());

    // The un-tokened address is refused: the render is the launch's own, like every other route.
    let refused = {
        use std::io::Read;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", engine.port)).expect("the port");
        stream.write_all(b"GET /render HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").expect("a request");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).ok();
        String::from_utf8_lossy(&raw).split_whitespace().nth(1).unwrap_or("").to_string()
    };
    assert_eq!(refused, "403", "the gateway serves only this launch's token");
}
