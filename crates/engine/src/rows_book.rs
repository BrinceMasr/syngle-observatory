//! The rows of a project opened in parts (Syngle-Ecosystem #471, ADR 0022): a map's document names its datasets' rows
//! (`features` is a reference, see `rustgis_data::RowsRef`) and the front end fetches them by id. This book is what the
//! session keeps between documents, so a document that names rows already here costs nothing and one that names a newer
//! revision fetches only that.
//!
//! - [`RowsBook::settle`] fills every dataset that is only a reference from what is held (a revision behind is shown until the
//!   new one lands, never an empty layer) and queues what is missing.
//! - The front end takes the queue ([`RowsBook::take_wanted`]), fetches each, and hands the bytes to [`RowsBook::give`].
//! - Rows the session holds itself are remembered ([`RowsBook::remember`]) so a document the page posts back, naming them,
//!   resolves to the very same rows.

use crate::Project;
use rustgis_data::source::fgb_rows;
use rustgis_data::{Dataset, Features, RowsRef};
use std::collections::HashMap;

/// Most reference-only documents' worth of rows a book keeps beyond what the project holds: none. Held rows are the project's.
#[derive(Default)]
pub struct RowsBook {
    held: HashMap<u64, Features>,
    /// Asked for and not yet given (or failed).
    asked: Vec<RowsRef>,
    /// Not yet handed to the front end.
    wanted: Vec<RowsRef>,
}

/// Every dataset of the project: each map's layers, then its tables.
fn datasets_mut(project: &mut Project) -> impl Iterator<Item = &mut Dataset> {
    let layers = project.maps.iter_mut().flat_map(|m| m.layers.iter_mut().map(|l| &mut l.dataset));
    layers.chain(project.tables.iter_mut().map(|t| &mut t.dataset))
}

impl RowsBook {
    /// Fills what a freshly opened project only names, from the rows held, and queues the rest to be fetched.
    pub fn settle(&mut self, project: &mut Project) {
        for ds in datasets_mut(project) {
            let Some(want) = ds.features.pending() else { continue };
            match self.held.get(&want.id) {
                Some(held) if held.revision() == want.revision => ds.features = held.clone(),
                Some(held) => {
                    // Behind: shown until the newer rows arrive.
                    ds.features = held.clone();
                    self.queue(want);
                }
                None => self.queue(want),
            }
        }
        self.remember(project);
    }

    fn queue(&mut self, want: RowsRef) {
        if !self.asked.contains(&want) {
            self.asked.push(want);
            self.wanted.push(want);
        }
    }

    /// Keeps the rows the project holds, by id, and lets go of the rest.
    pub fn remember(&mut self, project: &mut Project) {
        let mut held = HashMap::new();
        for ds in datasets_mut(project) {
            if ds.features.pending().is_none() {
                held.entry(ds.features.id()).or_insert_with(|| ds.features.clone());
            }
        }
        self.held = held;
    }

    /// The rows to fetch, once.
    pub fn take_wanted(&mut self) -> Vec<RowsRef> {
        std::mem::take(&mut self.wanted)
    }

    /// Whether rows are still being fetched.
    pub fn waiting(&self) -> bool {
        !self.asked.is_empty()
    }

    /// A fetch that did not end in rows: forgotten, so a later document asks again.
    pub fn failed(&mut self, want: RowsRef) {
        self.asked.retain(|a| *a != want);
        self.wanted.retain(|a| *a != want);
    }

    /// The rows that were asked for, as the engine served them: put in every dataset that shares them.
    pub fn give(&mut self, project: &mut Project, want: RowsRef, bytes: &[u8]) -> Result<usize, String> {
        self.asked.retain(|a| *a != want);
        let Some(fields) = datasets_mut(project).find(|d| d.features.id() == want.id).map(|d| d.fields.clone()) else {
            // The project moved on: nothing to fill.
            return Ok(0);
        };
        let rows = Features::with_identity(want.id, want.revision, fgb_rows::read_rows(bytes, &fields).map_err(|e| e.to_string())?);
        if rows.len() != want.count {
            return Err(format!("the rows came to {} where {} were named", rows.len(), want.count));
        }
        let mut filled = 0;
        for ds in datasets_mut(project) {
            if ds.features.id() == want.id {
                ds.features = rows.clone();
                filled += 1;
            }
        }
        self.held.insert(want.id, rows);
        Ok(filled)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rustgis_data::{Feature, Field, FieldType, GeometryType, Value};

    fn project_with_rows(n: usize) -> Project {
        let mut p = Project::new("p");
        let mut layer_ds = Dataset::new("pts", GeometryType::None, vec![Field::new("n", FieldType::Integer)]);
        layer_ds.features = (0..n).map(|i| Feature { id: i as u64 + 1, geometry: None, attrs: vec![Value::Int(i as i64)] }).collect();
        let mut layer = p.map().layers.first().cloned().unwrap_or_else(|| rustgis_map::Layer::new(1, layer_ds.clone()));
        layer.dataset = layer_ds;
        p.maps[0].layers = vec![layer];
        p
    }

    fn in_parts(p: &Project) -> Project {
        let text = rustgis_data::without_rows(|| serde_json::to_string(p)).unwrap();
        Project::from_json(&text).unwrap()
    }

    #[test]
    fn a_document_naming_rows_is_filled_from_the_wire_once_and_from_the_book_after() {
        let source = project_with_rows(40);
        let want = source.maps[0].layers[0].dataset.features.reference();
        let bytes = {
            let d = &source.maps[0].layers[0].dataset;
            fgb_rows::write_rows(&d.name, d.geometry_type, &d.fields, &d.features).unwrap()
        };
        let mut book = RowsBook::default();
        let mut page = in_parts(&source);
        book.settle(&mut page);
        assert_eq!(page.maps[0].layers[0].dataset.features.len(), 0, "nothing yet");
        assert_eq!(book.take_wanted(), vec![want]);
        assert!(book.take_wanted().is_empty(), "asked once");
        assert_eq!(book.give(&mut page, want, &bytes).unwrap(), 1);
        assert_eq!(page.maps[0].layers[0].dataset.features.len(), 40);
        assert_eq!(page.maps[0].layers[0].dataset.features.reference(), want, "the identity it was served under");

        // The echo, posted back in parts, resolves to the same rows with nothing fetched.
        let mut again = in_parts(&page);
        book.settle(&mut again);
        assert_eq!(again.maps[0].layers[0].dataset.features.len(), 40);
        assert!(book.take_wanted().is_empty());
        assert!(again.maps[0].layers[0].dataset.features.shares(&page.maps[0].layers[0].dataset.features));
    }

    #[test]
    fn a_newer_revision_shows_the_older_rows_and_fetches_the_new() {
        let mut source = project_with_rows(10);
        let old = source.maps[0].layers[0].dataset.features.reference();
        let old_bytes = {
            let d = &source.maps[0].layers[0].dataset;
            fgb_rows::write_rows(&d.name, d.geometry_type, &d.fields, &d.features).unwrap()
        };
        let mut book = RowsBook::default();
        let mut page = in_parts(&source);
        book.settle(&mut page);
        book.give(&mut page, old, &old_bytes).unwrap();
        let _ = book.take_wanted();
        // The engine changed the rows in place: the same id, a later revision.
        source.maps[0].layers[0].dataset.features.push(Feature { id: 99, geometry: None, attrs: vec![Value::Int(99)] });
        let new = source.maps[0].layers[0].dataset.features.reference();
        assert_eq!((new.id, new.revision), (old.id, old.revision + 1));
        let mut next = in_parts(&source);
        book.settle(&mut next);
        assert_eq!(next.maps[0].layers[0].dataset.features.len(), 10, "the layer does not go empty while the new rows come");
        assert_eq!(book.take_wanted(), vec![new]);
        let d = &source.maps[0].layers[0].dataset;
        let new_bytes = fgb_rows::write_rows(&d.name, d.geometry_type, &d.fields, &d.features).unwrap();
        book.give(&mut next, new, &new_bytes).unwrap();
        assert_eq!(next.maps[0].layers[0].dataset.features.len(), 11);
    }

    #[test]
    fn rows_that_do_not_read_or_do_not_match_are_an_error_and_ask_again() {
        let source = project_with_rows(3);
        let want = source.maps[0].layers[0].dataset.features.reference();
        let mut book = RowsBook::default();
        let mut page = in_parts(&source);
        book.settle(&mut page);
        assert_eq!(book.take_wanted(), vec![want]);
        assert!(book.give(&mut page, want, b"not rows").is_err());
        assert_eq!(page.maps[0].layers[0].dataset.features.len(), 0);
        book.failed(want);
        book.settle(&mut page);
        assert_eq!(book.take_wanted(), vec![want], "a later document asks again");
        // Rows for a project that has moved on fill nothing.
        let mut other = Project::new("other");
        assert_eq!(book.give(&mut other, want, b"anything").unwrap(), 0);
    }
}
