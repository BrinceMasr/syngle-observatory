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
//! - `GET /project.json` (no query), or `?map=active`: the map in view, as `?map=<id>`. There is no document with every row any
//!   more: a page reads the rows by dataset (the view does, itself, from the references).
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
    /// The map in view, by id.
    active: Option<u32>,
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

impl Published {
    fn build(project: &Project, revisions: &mut Revisions) -> Published {
        // The rows are named in the document by the dataset's own `features` (a reference), so nothing is patched in.
        let document = rustgis_engine::data::without_rows(|| serde_json::to_value(project)).unwrap_or(Value::Null);
        let mut datasets: Vec<Part> = Vec::new();
        let mut seen = |dataset: &Dataset| {
            if !datasets.iter().any(|p| p.id == dataset.features.id()) {
                datasets.push(Part::of(dataset));
            }
        };
        for map in &project.maps {
            for layer in &map.layers {
                seen(&layer.dataset);
            }
        }
        for table in &project.tables {
            seen(&table.dataset);
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
        Published { active: project.maps.get(project.active_map).map(|m| m.id), document, datasets, shell_revision, maps }
    }

    /// The names a page needs to know what changed: the shell's revision, each map's `[id, revision]` and each dataset's.
    pub fn revisions(&self) -> Value {
        json!({
            "shellRevision": self.shell_revision,
            "mapRevisions": self.maps.iter().map(|(id, revision)| json!([id, revision])).collect::<Vec<_>>(),
            "datasetRevisions": self.datasets.iter().map(|p| json!([p.id, p.revision])).collect::<Vec<_>>(),
        })
    }

    /// The document for what was asked: the shell, one map's layers, or the map in view.
    pub fn answer(&self, wanted: &Wanted) -> Option<Vec<u8>> {
        match wanted {
            Wanted::Shell => self.document(None),
            Wanted::Map(id) => self.document(Some(*id)),
            Wanted::Active => self.document(Some(self.active?)),
            Wanted::Bad => None,
        }
    }

    /// No map's layers (`None`) or one map's, without rows: `None` for a map the project does not hold.
    fn document(&self, map: Option<u32>) -> Option<Vec<u8>> {
        let mut document = self.document.clone();
        let maps = document.get_mut("maps").and_then(Value::as_array_mut)?;
        if let Some(wanted) = map
            && !self.maps.iter().any(|(id, _)| *id == wanted)
        {
            return None;
        }
        // The map asked for is the one in view of this document, so a page showing it needs nothing else.
        let shown = map.and_then(|wanted| self.maps.iter().position(|(id, _)| *id == wanted));
        for ((id, _), doc) in self.maps.iter().zip(maps.iter_mut()) {
            if Some(*id) != map
                && let Some(layers) = doc.get_mut("layers")
            {
                *layers = json!([]);
            }
        }
        if let (Some(shown), Some(object)) = (shown, document.as_object_mut()) {
            object.insert("active_map".into(), json!(shown));
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
    /// No part named: the map in view, which is the project a page that knows nothing of parts can read.
    Active,
    /// `?shell`: no map's layers.
    Shell,
    /// `?map=<id>`: that map's layers.
    Map(u32),
    /// A `map` that is not a map id.
    Bad,
}

/// Reads `?shell` or `?map=<id>` off a request target.
pub fn wanted_part(target: &str) -> Wanted {
    let Some((_, query)) = target.split_once('?') else { return Wanted::Active };
    let mut wanted = Wanted::Active;
    for pair in query.split('&') {
        match pair.split_once('=') {
            None if pair == "shell" => wanted = Wanted::Shell,
            Some(("map", "active")) => wanted = Wanted::Active,
            Some(("map", id)) => match id.parse::<u32>() {
                Ok(id) => wanted = Wanted::Map(id),
                Err(_) => return Wanted::Bad,
            },
            _ => {}
        }
    }
    wanted
}
