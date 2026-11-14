//! A snapshot: one folder per data version, `meta.json` and the table files. Written once, in a temporary folder, and
//! renamed; never changed after that, so a reader needs no lock.

use crate::codec::Dicts;
use crate::error::{GovError, OrDamaged, Res};
use crate::table::{Blob, Fixed};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The layout of the table files. Part of the data version, so a build with another layout never reads an old folder.
pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceMeta {
    pub version: String,
    #[serde(rename = "asOf")]
    pub as_of: String,
    #[serde(rename = "asOfKind", default, skip_serializing_if = "Option::is_none")]
    pub as_of_kind: Option<String>,
    pub licence: String,
    pub attribution: String,
    pub publisher: String,
    pub records: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    pub format: u32,
    pub id: String,
    pub sources: BTreeMap<String, SourceMeta>,
    #[serde(default)]
    pub dicts: Dicts,
}

pub struct Snap {
    pub dir: PathBuf,
    pub meta: Meta,
}

impl Snap {
    pub fn open(dir: PathBuf) -> Res<Snap> {
        let bytes = std::fs::read(dir.join("meta.json")).damaged()?;
        let meta: Meta = serde_json::from_slice(&bytes).map_err(GovError::damaged)?;
        if meta.format != FORMAT {
            return Err(GovError::new("no-data", "the index was built by another version of this program; run syngle-map gov refresh"));
        }
        Ok(Snap { dir, meta })
    }

    pub fn has(&self, source: &str) -> bool {
        self.meta.sources.contains_key(source)
    }

    /// A source the verb needs, or the refusal that says how to get it.
    pub fn need(&self, source: &str, what: &str) -> Res<()> {
        if self.has(source) {
            Ok(())
        } else {
            Err(GovError::new("no-data", format!("{what} needs the {source} source: run syngle-map gov refresh --source {source}")))
        }
    }

    pub fn fixed(&self, file: &str, width: u64) -> Res<Fixed> {
        Fixed::open(&self.dir.join(file), width).damaged()
    }

    pub fn blob(&self, file: &str) -> Res<Blob> {
        Blob::open(&self.dir.join(file)).damaged()
    }
}
