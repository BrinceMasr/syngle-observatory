//! The project in parts (Syngle-Ecosystem #471, ADR 0022): what the gateway serves instead of one document that holds
//! every map and every row.
//!
//! - `GET /project.json?shell`: the project's own document with no map's layers and no rows (name, maps with their ids and
//!   views, styles, ramps, layouts, tables' schemas).
//! - `GET /project.json?map=<id>`: the shell with that map's layers, still without rows. Each layer's dataset names its
//!   rows (`rows: {id, revision, count}`).
//! - `GET /dataset/<id>`: those rows, as FlatGeobuf in row order (`fgb_rows`), by byte range, with an `ETag` of the
//!   dataset's revision. A dataset's id is the identity of its allocation of rows, so maps that share rows share an id and
//!   the page fetches them once.
//! - `GET /project.json` (no query): the whole project, as before, for a page that does not know the parts. It goes when
//!   the page takes the parts (#471, phase 2).
//!
//! The command loop owns the session, so after a command changes the project it publishes a [`Published`]: a shallow copy
//! (rows are shared, not copied) from which the gateway's threads serve on demand. Nothing is serialised until a page asks,
//! and a style edit never touches rows. Each map and the shell have a revision that moves only when their document does;
//! a dataset's is its rows' own. The bridge state names them, so a page takes only what changed.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::sync::{Arc, Mutex, PoisonError};

use rustgis_engine::Project;
use rustgis_engine::data::source::fgb_rows;
use rustgis_engine::data::{Dataset, Features, Field, GeometryType};
use serde_json::{Value, json};

/// One dataset's rows, as the gateway can build them.
struct Part {
    id: u64,
    revision: u64,
    name: String,
    geometry_type: GeometryType,
    fields: Vec<Field>,
    features: Features,
}

impl Part {
    fn of(dataset: &Dataset) -> Part {
        Part {
            id: dataset.features.id(),
            revision: dataset.features.revision(),
            name: dataset.name.clone(),
            geometry_type: dataset.geometry_type,
            fields: dataset.fields.clone(),
            features: dataset.features.clone(),
        }
    }
}

/// What the project was when its last change finished.
pub struct Published {
    /// The whole project, for the route that has not gone yet.
    project: Project,
    /// The project's document with no rows: every map's layers in place.
    document: Value,
    datasets: Vec<Part>,
    shell_revision: u64,
    maps: Vec<(u32, u64)>,
}

/// A hash of a document, to tell whether it changed.
fn fingerprint(value: &Value) -> u64 {
    struct Hashing(DefaultHasher);
    impl std::io::Write for Hashing {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.write(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hashing = Hashing(DefaultHasher::new());
    let _ = serde_json::to_writer(&mut hashing, value);
    hashing.0.finish()
}

/// The revisions the command loop keeps between publications: a document's revision moves only when its text does.
#[derive(Default)]
struct Revisions {
    shell: (u64, u64),
    maps: HashMap<u32, (u64, u64)>,
}

impl Revisions {
    fn bump(slot: &mut (u64, u64), hash: u64) -> u64 {
        if slot.1 == 0 || slot.0 != hash {
            *slot = (hash, slot.1 + 1);
        }
        slot.1
    }
}

fn rows_of(dataset: &Dataset) -> Value {
    json!({ "id": dataset.features.id(), "revision": dataset.features.revision(), "count": dataset.features.len() })
}

impl Published {
    fn build(project: &Project, revisions: &mut Revisions) -> Published {
        let mut document = rustgis_engine::data::without_rows(|| serde_json::to_value(project)).unwrap_or(Value::Null);
        let mut datasets: Vec<Part> = Vec::new();
        let mut seen = |dataset: &Dataset| {
            if !datasets.iter().any(|p| p.id == dataset.features.id()) {
                datasets.push(Part::of(dataset));
            }
        };
        // The datasets name their rows in the document: the same walk as the project's own, in order.
        if let Some(maps) = document.get_mut("maps").and_then(Value::as_array_mut) {
            for (map, doc) in project.maps.iter().zip(maps) {
                let Some(layers) = doc.get_mut("layers").and_then(Value::as_array_mut) else { continue };
                for (layer, doc) in map.layers.iter().zip(layers) {
                    seen(&layer.dataset);
                    if let Some(dataset) = doc.get_mut("dataset").and_then(Value::as_object_mut) {
                        dataset.insert("rows".into(), rows_of(&layer.dataset));
                    }
                }
            }
        }
        if let Some(tables) = document.get_mut("tables").and_then(Value::as_array_mut) {
            for (table, doc) in project.tables.iter().zip(tables) {
                seen(&table.dataset);
                if let Some(dataset) = doc.as_object_mut() {
                    dataset.insert("rows".into(), rows_of(&table.dataset));
                }
            }
        }
        // Each map's revision follows its own document; the shell's follows everything else.
        let mut maps = Vec::new();
        let mut live = Vec::new();
        let mut shell = document.clone();
        if let (Some(docs), Some(shell_maps)) =
            (document.get("maps").and_then(Value::as_array), shell.get_mut("maps").and_then(Value::as_array_mut))
        {
            for ((map, doc), shell_map) in project.maps.iter().zip(docs).zip(shell_maps) {
                let slot = revisions.maps.entry(map.id).or_default();
                maps.push((map.id, Revisions::bump(slot, fingerprint(doc))));
                live.push(map.id);
                if let Some(layers) = shell_map.get_mut("layers") {
                    *layers = json!([]);
                }
            }
        }
        revisions.maps.retain(|id, _| live.contains(id));
        let shell_revision = Revisions::bump(&mut revisions.shell, fingerprint(&shell));
        Published { project: project.clone(), document, datasets, shell_revision, maps }
    }

    /// The names a page needs to know what changed: the shell's revision, each map's `[id, revision]` and each dataset's.
    pub fn revisions(&self) -> Value {
        json!({
            "shellRevision": self.shell_revision,
            "mapRevisions": self.maps.iter().map(|(id, revision)| json!([id, revision])).collect::<Vec<_>>(),
            "datasetRevisions": self.datasets.iter().map(|p| json!([p.id, p.revision])).collect::<Vec<_>>(),
        })
    }

    /// The whole project, as one document.
    pub fn whole(&self) -> Vec<u8> {
        serde_json::to_vec(&self.project).unwrap_or_default()
    }

    /// `?shell` (no map's layers) or `?map=<id>` (that map's layers): `None` for a map the project does not hold.
    pub fn document(&self, map: Option<u32>) -> Option<Vec<u8>> {
        let mut document = self.document.clone();
        let maps = document.get_mut("maps").and_then(Value::as_array_mut)?;
        if let Some(wanted) = map
            && !self.maps.iter().any(|(id, _)| *id == wanted)
        {
            return None;
        }
        for ((id, _), doc) in self.maps.iter().zip(maps) {
            if Some(*id) != map
                && let Some(layers) = doc.get_mut("layers")
            {
                *layers = json!([]);
            }
        }
        serde_json::to_vec(&document).ok()
    }

    /// The rows of the dataset `part`, built now (the gateway keeps the last few, see [`Parts::dataset`]).
    fn rows(part: &Part) -> Result<Vec<u8>, String> {
        fgb_rows::write_rows(&part.name, part.geometry_type, &part.fields, &part.features).map_err(|e| e.to_string())
    }
}

/// The most bytes of built rows the gateway keeps: the rows of the datasets a page is reading, not of every map. Built
/// again after a revision, and not at all while no page asks, so it does not grow with the maps.
const KEPT_BYTES: usize = 64 << 20;

/// What the gateway serves, and what the command loop publishes into.
pub struct Parts {
    current: Mutex<Arc<Published>>,
    revisions: Mutex<Revisions>,
    built: Mutex<Vec<(String, Arc<Vec<u8>>)>>,
}

impl Default for Parts {
    fn default() -> Parts {
        Parts::of(&Project::default())
    }
}

impl Parts {
    pub fn of(project: &Project) -> Parts {
        let mut revisions = Revisions::default();
        let current = Mutex::new(Arc::new(Published::build(project, &mut revisions)));
        Parts { current, revisions: Mutex::new(revisions), built: Mutex::new(Vec::new()) }
    }

    /// Called by the command loop after a command changed the project.
    pub fn publish(&self, project: &Project) {
        let mut revisions = self.revisions.lock().unwrap_or_else(PoisonError::into_inner);
        let next = Arc::new(Published::build(project, &mut revisions));
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = next;
    }

    pub fn current(&self) -> Arc<Published> {
        Arc::clone(&self.current.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Dataset `id`'s rows with their `ETag`: `None` when the project holds no such dataset, `Some(Err)` when they could
    /// not be written.
    pub fn dataset(&self, id: u64) -> Option<(String, Result<Arc<Vec<u8>>, String>)> {
        let published = self.current();
        let part = published.datasets.iter().find(|p| p.id == id)?;
        let tag = format!("\"{}-{}\"", part.id, part.revision);
        {
            let built = self.built.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((_, bytes)) = built.iter().find(|(t, _)| *t == tag) {
                return Some((tag, Ok(Arc::clone(bytes))));
            }
        }
        let bytes = Published::rows(part).map(Arc::new);
        if let Ok(bytes) = &bytes {
            let mut built = self.built.lock().unwrap_or_else(PoisonError::into_inner);
            // An older revision of the same rows is of no use to anyone.
            let same = format!("\"{id}-");
            built.retain(|(t, _)| !t.starts_with(&same));
            built.push((tag.clone(), Arc::clone(bytes)));
            let mut total: usize = built.iter().map(|(_, b)| b.len()).sum();
            while total > KEPT_BYTES && built.len() > 1 {
                total -= built.remove(0).1.len();
            }
        }
        Some((tag, bytes))
    }
}

/// What a `/project.json` request asks for.
#[derive(Debug, PartialEq)]
pub enum Wanted {
    /// No part named: the whole project.
    Whole,
    /// `?shell` (`None`) or `?map=<id>` (`Some`).
    Part(Option<u32>),
    /// A `map` that is not a map id.
    Bad,
}

/// Reads `?shell` or `?map=<id>` off a request target.
pub fn wanted_part(target: &str) -> Wanted {
    let Some((_, query)) = target.split_once('?') else { return Wanted::Whole };
    let mut wanted = Wanted::Whole;
    for pair in query.split('&') {
        match pair.split_once('=') {
            None if pair == "shell" => wanted = Wanted::Part(None),
            Some(("map", id)) => match id.parse::<u32>() {
                Ok(id) => wanted = Wanted::Part(Some(id)),
                Err(_) => return Wanted::Bad,
            },
            _ => {}
        }
    }
    wanted
}
