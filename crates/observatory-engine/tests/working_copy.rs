//! The working copy, driven the way the app drives the engine (Syngle-Ecosystem #671).
//!
//! The real sidecar binary is started with `--autosave <file>` and spoken to on its own stdin and stdout. What is asked
//! is what a person feels: make something, close the app, open it again, and it is there. A kill stands for a crash.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

struct Engine {
    child: Child,
    lines: Receiver<String>,
    ready: Value,
    hello: bool,
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obs-working-copy-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("data")).expect("data folder");
    std::fs::create_dir_all(dir.join("web")).expect("web folder");
    dir
}

impl Engine {
    fn start(root: &Path, working_copy: &Path) -> Engine {
        let mut child = Command::new(env!("CARGO_BIN_EXE_syngle-observatory-engine"))
            .arg("--root")
            .arg(root)
            .arg("--web")
            .arg(root.join("web"))
            .arg("--data")
            .arg(root.join("data"))
            .args(["--port", "0", "--autosave"])
            .arg(working_copy)
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
        let mut engine = Engine { child, lines, ready: Value::Null, hello: false };
        engine.ready = engine.next();
        assert_eq!(engine.ready.get("event").and_then(Value::as_str), Some("ready"), "{}", engine.ready);
        engine
    }

    fn restored(&self) -> bool {
        self.ready.get("restored").and_then(Value::as_bool).unwrap_or(false)
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.child.stdin.as_mut().expect("the engine's stdin");
        writeln!(stdin, "{message}").expect("a write on the control channel");
        stdin.flush().expect("a flush on the control channel");
    }

    fn next(&mut self) -> Value {
        match self.lines.recv_timeout(ANSWER_TIMEOUT) {
            Ok(line) => serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON: {line}: {e}")),
            Err(RecvTimeoutError::Timeout) => panic!("the engine answered nothing in {ANSWER_TIMEOUT:?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("the engine stopped"),
        }
    }

    /// One registry command through the bridge (after its hello, said once); answers with the command's value once the
    /// engine has settled it.
    fn command(&mut self, id: &str, params: Value) -> Value {
        if !self.hello {
            self.send(&json!({"cmd": "bridge", "bridge": "hello", "protocolVersion": 1}));
            assert_eq!(self.next()["event"], "bridge.ready");
            self.hello = true;
        }
        self.send(&json!({
            "cmd": "bridge", "bridge": "command", "protocolVersion": 1, "id": 1,
            "command": {"name": "command", "id": id, "params": params},
        }));
        let mut value = Value::Null;
        loop {
            let line = self.next();
            match line["event"].as_str() {
                Some("bridge.state") => return value,
                Some("bridge.error") => panic!("{id} failed: {line}"),
                _ => {
                    if line["payload"]["type"] == "result" {
                        value = line["payload"]["value"].clone();
                    }
                }
            }
        }
    }

    /// What `info.project` says: the file the project was saved to and whether it has changes that file lacks.
    fn info(&mut self) -> (Option<String>, bool) {
        let info = self.command("info.project", json!({"includeFields": false}));
        (info["path"].as_str().map(str::to_string), info["unsaved"].as_bool().unwrap_or(false))
    }

    /// The project the engine holds now, as the host reads it.
    fn project(&mut self) -> Value {
        self.send(&json!({"cmd": "project", "id": 7}));
        loop {
            let line = self.next();
            if line["event"] == "project" {
                return serde_json::from_str(line["text"].as_str().expect("the project's text")).expect("project json");
            }
        }
    }

    /// The names of the maps the engine holds now.
    fn maps(&mut self) -> Vec<String> {
        names_of(&self.project())
    }

    /// A polite quit: the engine writes what is unwritten and exits.
    fn quit(mut self) {
        self.send(&json!({"cmd": "quit"}));
        self.wait_for_exit();
    }

    /// A crash: the process ends where it stands.
    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn wait_for_exit(&mut self) {
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(20) {
            if self.child.try_wait().expect("the engine's state").is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        panic!("the engine did not exit after quit");
    }
}

fn names_of(project: &Value) -> Vec<String> {
    project["maps"].as_array().expect("maps").iter().map(|m| m["name"].as_str().expect("a map's name").to_string()).collect()
}

fn names_in(file: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(file).expect("the working copy");
    names_of(&serde_json::from_str(&text).expect("the working copy is json"))
}

/// Waits for `file` to hold a map called `name`, or fails.
fn wait_for_map(file: &Path, name: &str, within: Duration) {
    let began = Instant::now();
    while began.elapsed() < within {
        if std::fs::read_to_string(file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|p| names_of(&p).iter().any(|n| n == name))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{} never held a map called {name}", file.display());
}

#[test]
fn a_first_start_has_nothing_to_restore_and_writes_nothing_until_there_is_a_change() {
    let root = folder("first");
    let file = root.join("maps").join("project.rgis");
    let mut engine = Engine::start(&root, &file);
    assert!(!engine.restored());
    assert_eq!(engine.maps(), ["Map"]);
    engine.quit();
    assert!(!file.exists(), "an engine nobody changed has no copy to keep");
}

#[test]
fn a_quit_keeps_the_project_and_the_next_start_brings_it_back() {
    let root = folder("quit");
    let file = root.join("maps").join("project.rgis");
    let mut first = Engine::start(&root, &file);
    first.command("map.new", json!({"name": "Sydney data centres"}));
    first.quit();
    assert_eq!(names_in(&file), ["Map", "Sydney data centres"]);

    let mut second = Engine::start(&root, &file);
    assert!(second.restored(), "{}", second.ready);
    assert_eq!(second.maps(), ["Map", "Sydney data centres"]);
    second.quit();
}

#[test]
fn a_change_is_on_disk_after_a_quiet_moment_so_a_crash_loses_almost_nothing() {
    let root = folder("crash");
    let file = root.join("maps").join("project.rgis");
    let mut first = Engine::start(&root, &file);
    first.command("map.new", json!({"name": "Second"}));
    wait_for_map(&file, "Second", Duration::from_secs(10));
    first.kill();

    let mut second = Engine::start(&root, &file);
    assert!(second.restored());
    assert_eq!(second.maps(), ["Map", "Second"]);
    second.quit();
}

#[test]
fn a_working_copy_that_is_not_a_project_is_set_aside_and_the_engine_starts_fresh() {
    let root = folder("broken");
    let maps = root.join("maps");
    std::fs::create_dir_all(&maps).expect("maps folder");
    let file = maps.join("project.rgis");
    std::fs::write(&file, "{ not a project").expect("a broken copy");
    let mut engine = Engine::start(&root, &file);
    assert!(!engine.restored());
    assert_eq!(engine.maps(), ["Map"]);
    engine.command("map.new", json!({"name": "After"}));
    engine.quit();
    assert_eq!(std::fs::read_to_string(maps.join("project.rgis.unreadable")).expect("kept aside"), "{ not a project");
    assert_eq!(names_in(&file), ["Map", "After"]);
}

#[test]
fn the_file_a_project_was_saved_to_and_its_unsaved_mark_survive_a_quit() {
    let root = folder("saved");
    let file = root.join("maps").join("project.rgis");
    let saved = root.join("Sydney.rgis");
    let mut first = Engine::start(&root, &file);
    first.command("map.new", json!({"name": "Sydney data centres"}));
    first.command("project.saveAs", json!({"path": saved.to_string_lossy()}));
    assert_eq!(first.info(), (Some(saved.display().to_string()), false));
    first.command("map.new", json!({"name": "After the save"}));
    assert!(first.info().1);
    first.quit();

    let mut second = Engine::start(&root, &file);
    assert!(second.restored());
    // "Save" still writes to the same file, and the map made after the save is still unsaved.
    assert_eq!(second.info(), (Some(saved.display().to_string()), true));
    assert_eq!(second.maps(), ["Map", "Sydney data centres", "After the save"]);
    second.quit();
}

/// What a person sees of a map, and so what has to come back unchanged: its name, basemap and view, and each layer's name,
/// style, visibility and rows.
fn what_a_person_sees(project: &Value) -> Value {
    let maps: Vec<Value> = project["maps"]
        .as_array()
        .expect("maps")
        .iter()
        .map(|map| {
            let layers: Vec<Value> = map["layers"]
                .as_array()
                .expect("layers")
                .iter()
                .map(|l| json!({"name": l["name"], "renderer": l["renderer"], "visible": l["visible"], "dataset": l["dataset"]}))
                .collect();
            json!({"id": map["id"], "name": map["name"], "basemap": map["basemap"], "camera": map["camera"], "layers": layers})
        })
        .collect();
    json!({"name": project["name"], "active_map": project["active_map"], "maps": maps})
}

const SITES: &str = r#"{"type":"FeatureCollection","features":[
{"type":"Feature","properties":{"name":"SYD1","operator":"AirTrunk"},"geometry":{"type":"Point","coordinates":[151.2093,-33.8688]}},
{"type":"Feature","properties":{"name":"SYD2","operator":"NEXTDC"},"geometry":{"type":"Point","coordinates":[151.1,-33.9]}},
{"type":"Feature","properties":{"name":"SYD3","operator":"Equinix"},"geometry":{"type":"Point","coordinates":[151.0,-33.8]}}]}"#;

#[test]
fn a_map_with_data_a_style_a_basemap_and_a_view_comes_back_exactly() {
    let root = folder("rich");
    let sites = root.join("data").join("sites.geojson");
    std::fs::write(&sites, SITES).expect("a dataset");
    let file = root.join("maps").join("project.rgis");

    let mut first = Engine::start(&root, &file);
    first.command("layer.addData", json!({"path": sites.to_string_lossy(), "name": "Sydney data centres"}));
    first.command("layer.symbology", json!({"layer": "Sydney data centres", "type": "unique", "field": "operator"}));
    first.command("map.basemap", json!({"name": "midnight"}));
    first.command("map.goToXY", json!({"lon": 151.0, "lat": -33.9, "scale": 50000.0, "bearing": 0, "pitch": 0}));
    first.command("map.new", json!({"name": "Second map"}));
    first.command("map.rename", json!({"name": "Sites, second look"}));
    let before = what_a_person_sees(&first.project());
    // The test means something only if there is something there to lose.
    assert_eq!(before["maps"].as_array().map(Vec::len), Some(2));
    assert_eq!(before["maps"][0]["layers"][0]["dataset"]["features"].as_array().map(Vec::len), Some(3));
    assert_eq!(before["maps"][0]["basemap"], "Midnight");
    assert!(before["maps"][0]["layers"][0]["renderer"].get("Unique").is_some(), "{}", before["maps"][0]["layers"][0]["renderer"]);
    first.quit();

    let mut second = Engine::start(&root, &file);
    assert!(second.restored(), "{}", second.ready);
    assert_eq!(what_a_person_sees(&second.project()), before);
    second.quit();

    // And again: a restored project that is quit untouched is still the same project, not a thinner copy.
    let mut third = Engine::start(&root, &file);
    assert_eq!(what_a_person_sees(&third.project()), before);
    third.quit();
}
