//! Fetching the rows of a project opened in parts (Syngle-Ecosystem #471, ADR 0022): the document opened from a web address
//! (`ui.openUrl`, the embedded view's `?project=`) names its datasets' rows, and each is downloaded from the same gateway
//! as `<address of the document>/../dataset/<id>?revision=<n>`, once per id and revision. What arrives is put in every
//! dataset that shares it (`rustgis_engine::rows_book`).

use crate::{RustGisApp, Slot};
use rustgis_engine::data::RowsRef;
use std::time::Duration;

/// The downloads under way and the address they are read from.
#[derive(Default)]
pub struct Rows {
    /// Where datasets are served: the project's address up to its last `/`, once a project was opened from one.
    base: Option<String>,
    downloads: Vec<(RowsRef, String, Slot<Result<Vec<u8>, String>>)>,
}

/// The folder a document's address is in: `http://host/t/token/project.json?map=active` is `http://host/t/token/`.
pub(crate) fn base_of(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let (folder, last) = path.rsplit_once('/')?;
    last.eq_ignore_ascii_case("project.json").then(|| format!("{folder}/"))
}

impl Rows {
    /// Rows are read from the folder this document's address is in.
    pub(crate) fn opened_from(&mut self, url: &str) {
        self.base = base_of(url);
    }

    pub(crate) fn busy(&self) -> bool {
        !self.downloads.is_empty()
    }
}

impl RustGisApp {
    /// Per frame: starts the downloads the open project asks for and takes the rows that have landed.
    pub(crate) fn poll_rows(&mut self, ctx: &egui::Context) {
        for want in self.session.rows_book.take_wanted() {
            let Some(base) = self.ui.rows.base.clone() else {
                self.session.rows_book.failed(want);
                self.session.notify(crate::i18n::fmt("This project names its rows but not where they are kept", &[]), true);
                continue;
            };
            let url = format!("{base}dataset/{}?revision={}", want.id, want.revision);
            let slot: Slot<Result<Vec<u8>, String>> = Slot::default();
            let land = slot.clone();
            rustgis_engine::render::rustgis_tiles::cache::download(
                &url,
                Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)),
            );
            self.ui.rows.downloads.push((want, url, slot));
        }
        let mut landed = Vec::new();
        self.ui.rows.downloads.retain(|(want, url, slot)| {
            let r = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
            match r {
                Some(r) => {
                    landed.push((*want, url.clone(), r));
                    false
                }
                None => true,
            }
        });
        for (want, url, r) in landed {
            match r.and_then(|bytes| self.session.rows_book.give(&mut self.session.project, want, &bytes)) {
                Ok(_) => {
                    self.map.invalidate();
                    ctx.request_repaint();
                }
                Err(e) => {
                    self.session.rows_book.failed(want);
                    self.session.notify(format!("{url}: {e}"), true);
                }
            }
        }
        // A frame may not come until the user moves: look again while rows are on their way.
        if self.ui.rows.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::base_of;

    #[test]
    fn rows_are_read_from_the_folder_the_document_is_in() {
        assert_eq!(base_of("http://127.0.0.1:7391/t/5f2a/project.json").as_deref(), Some("http://127.0.0.1:7391/t/5f2a/"));
        assert_eq!(base_of("http://127.0.0.1:7391/project.json?map=active#x").as_deref(), Some("http://127.0.0.1:7391/"));
        assert_eq!(base_of("http://example.com/maps/7/project?v=2"), None);
        assert_eq!(base_of("http://example.com/a.rgis"), None);
    }
}
