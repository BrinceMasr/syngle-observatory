//! The layers a command run on this machine made, written where the app can read them by range
//! (Syngle-Ecosystem #419).
//!
//! The app holds a map's project on the computer the person is at and keeps its data on the machine the
//! chat runs on. A scan runs here, against the map's saved copy, in a process that ends with the command.
//! A command that makes a layer (a filter into a new layer, a join, an opened file) would lose it at the
//! end. So each layer a command added is written as a FlatGeobuf file in a folder the caller names: the
//! format has a spatial index in front, so the app's engine reads the part it draws by byte range and the
//! dataset never crosses. The answer names the files; the app opens them as it opens any file of this
//! machine.
//!
//! Only vector layers are written. A raster or graphics layer is not feature data and is left out (the app
//! opens a raster by its address as before). A layer whose source already is a file the app reads in
//! place is named, not copied.

use std::collections::HashSet;
use std::path::Path;

use rustgis_engine::Session;
use rustgis_engine::data::io;
use serde_json::{Value, json};

/// The extensions of files the app's engine reads in place, a range at a time.
const RANGE_READABLE: [&str; 3] = ["fgb", "parquet", "geoparquet"];

/// The longest a written file's stem is, so a long layer name never makes a path the system refuses.
const STEM_LIMIT: usize = 60;

/// The ids of the layers the session holds now: what a command's new layers are told apart from.
pub fn layer_ids(session: &Session) -> HashSet<u32> {
    session.map().layers.iter().map(|l| l.id).collect()
}

/// A layer's name as a file stem: letters, digits, `_` and `-`, whatever else becomes `_`.
fn stem_of(name: &str) -> String {
    let stem: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(STEM_LIMIT).collect();
    if stem.trim_matches('_').is_empty() { "layer".to_string() } else { stem }
}

/// FNV-1a of `text`, as eight hex digits: the same command on the same layer names the same file, so a repeat
/// replaces it and the folder does not grow.
fn short_key(text: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// The path of a layer's source when it is a file the app reads in place.
fn range_readable_source(layer: &rustgis_engine::map::Layer) -> Option<String> {
    let path = layer.source.as_ref()?.path.as_deref()?;
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    RANGE_READABLE.contains(&ext.as_str()).then(|| path.to_string())
}

/// A layer's every feature, and what reading them said. A streamed layer reads its source whole (up to the engine's
/// read limit, which it reports); any other layer already holds its features.
fn whole(layer: &rustgis_engine::map::Layer) -> Result<(rustgis_engine::data::Dataset, Vec<String>), String> {
    let Some(source) = layer.source.as_ref() else { return Ok((layer.dataset.clone(), Vec::new())) };
    let mut copy = source.clone();
    if !copy.is_open() {
        copy.open()?;
    }
    let (dataset, report) = copy.read_all()?;
    Ok((dataset, report.messages))
}

/// Writes every vector layer the session gained since `before` into `dir` and says where each went:
/// `{"name", "path", "features", "skipped"?}`. `key` is the command's text, which makes the file names.
/// The folder is made when it is missing; each file is written through a temporary file and renamed in
/// place, so a reader never sees half of one.
pub fn emit_new(session: &Session, before: &HashSet<u32>, dir: &Path, key: &str) -> Result<Vec<Value>, String> {
    let mut written = Vec::new();
    for layer in session.map().layers.iter().filter(|l| !before.contains(&l.id)) {
        if layer.raster.is_some() || layer.graphics {
            continue;
        }
        if let Some(path) = range_readable_source(layer) {
            written.push(json!({"name": layer.name, "path": path, "features": Value::Null}));
            continue;
        }
        let (dataset, notes) = whole(layer)?;
        let crs = io::default_export_crs(&dataset, "fgb");
        let (bytes, skipped) = io::write_bytes_in(&dataset, "fgb", crs).map_err(|e| format!("{}: {e}", layer.name))?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let file = dir.join(format!("{}-{}.fgb", stem_of(&layer.name), short_key(&format!("{key}\n{}", layer.name))));
        io::write_atomic(&file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        let mut entry = json!({"name": layer.name, "path": file.display().to_string(), "features": dataset.features.len()});
        if skipped > 0 {
            entry["skipped"] = json!(skipped);
        }
        // A source too large to read whole says so (`read a sample of N of M features`): the app tells the person.
        if !notes.is_empty() {
            entry["notes"] = json!(notes);
        }
        written.push(entry);
    }
    Ok(written)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use rustgis_engine::Project;

    fn session_with_cities() -> Session {
        let mut s = Session::new(Project::default());
        let text = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"n":1},"geometry":{"type":"Point","coordinates":[1.0,2.0]}},
            {"type":"Feature","properties":{"n":2},"geometry":{"type":"Point","coordinates":[3.0,4.0]}}]}"#;
        s.run("layer.addDataText", &json!({"fileName": "cities.geojson", "text": text})).unwrap();
        s
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("syngle-emit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_layer_the_command_added_is_written_as_a_range_readable_file_and_one_that_was_there_is_not() {
        let mut s = session_with_cities();
        let before = layer_ids(&s);
        s.run("layer.addDataText", &json!({"fileName": "more.geojson", "text": r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[5.0,6.0]}}]}"#})).unwrap();
        let dir = scratch("added");
        let written = emit_new(&s, &before, &dir, "{\"name\":\"command\"}").unwrap();
        assert_eq!(written.len(), 1, "{written:?}");
        let path = written[0]["path"].as_str().unwrap();
        assert!(path.ends_with(".fgb") && path.contains("more"), "{path}");
        assert_eq!(written[0]["features"], json!(1));
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..3], b"fgb", "a FlatGeobuf file starts with its magic bytes");
        // The same command again names the same file: a repeat replaces it, the folder does not grow.
        let again = emit_new(&s, &before, &dir, "{\"name\":\"command\"}").unwrap();
        assert_eq!(again[0]["path"], written[0]["path"]);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_new_writes_nothing_and_makes_no_folder() {
        let s = session_with_cities();
        let before = layer_ids(&s);
        let dir = scratch("none");
        assert!(emit_new(&s, &before, &dir, "x").unwrap().is_empty());
        assert!(!dir.exists());
    }

    #[test]
    fn a_file_stem_is_safe_and_never_empty() {
        assert_eq!(stem_of("Cities / 2024"), "Cities___2024");
        assert_eq!(stem_of("../../etc"), "______etc");
        assert_eq!(stem_of("???"), "layer");
        assert_eq!(stem_of(&"a".repeat(200)).len(), STEM_LIMIT);
    }
}
