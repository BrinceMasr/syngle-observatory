//! `gov refresh --from <folder>`: a snapshot built from official files and the `manifest.json` that names them. Tests and
//! the weekly job share this one path (the job's download writes the same folder and manifest). The version of every
//! source is derived from the bytes of its files, and the data version from the versions, never from the clock, so the
//! same files give the same version on any machine. A refresh that fails changes nothing.

use crate::build::{SourcePlan, build};
use crate::error::{GovError, Res};
use crate::snap::{FORMAT, Meta, SourceMeta};
use crate::sources::{SOURCES, info};
use crate::store::Store;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// The sources this build reads. Others in a manifest are left out of the snapshot.
const BUILT: [&str; 6] = ["abr", "asic", "asicBn", "gnaf", "abs", "asgs"];
const MAX_MANIFEST: u64 = 1 << 20;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub sources: BTreeMap<String, ManifestSource>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestSource {
    #[serde(rename = "asOf")]
    pub as_of: String,
    pub files: Vec<String>,
    pub licence: Option<String>,
    pub publisher: Option<String>,
    #[serde(rename = "asOfKind")]
    pub as_of_kind: Option<String>,
    /// Where this source's files are, when not in the manifest's own folder (the network refresh keeps each release in a folder
    /// of its own). Never read from a manifest file.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

pub struct RefreshedSource {
    pub source: String,
    pub version: String,
    pub as_of: String,
    pub records: u64,
}

pub struct Refreshed {
    pub id: String,
    pub built: bool,
    pub sources: Vec<RefreshedSource>,
}

pub(crate) fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// A file named by a manifest stays inside the folder the manifest is in.
fn inside(base: &Path, rel: &str) -> Res<PathBuf> {
    let p = Path::new(rel);
    if rel.is_empty() || p.is_absolute() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(GovError::bad_source(format!("the manifest names {rel}, which is not a path inside its folder")));
    }
    let full = base.join(p);
    if full.exists() { Ok(full) } else { Err(GovError::bad_source(format!("the manifest names {rel}, which is not there"))) }
}

/// Every file under `full` (or `full` itself), in name order, with the path as the manifest wrote it.
fn expand(rel: &str, full: &Path, depth: u8, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    if full.is_dir() {
        if depth > 6 {
            return Err(crate::rd::invalid("folders nested too deeply"));
        }
        let mut names: Vec<String> = std::fs::read_dir(full)?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for n in names {
            expand(&format!("{rel}/{n}"), &full.join(&n), depth + 1, out)?;
        }
    } else {
        out.push((rel.replace('\\', "/"), full.to_path_buf()));
    }
    Ok(())
}

fn hash_files(files: &[(String, PathBuf)]) -> io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    for (rel, path) in files {
        h.update(rel.as_bytes());
        let mut f = File::open(path)?;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(buf.get(..n).unwrap_or_default());
        }
    }
    Ok(h.finalize().iter().take(4).map(|b| format!("{b:02x}")).collect())
}

fn data_version(versions: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(format!("{FORMAT}").as_bytes());
    for v in versions {
        h.update(b"\n");
        h.update(v.as_bytes());
    }
    format!("gov-{}", h.finalize().iter().take(4).map(|b| format!("{b:02x}")).collect::<String>())
}

pub fn from_dir(store: &Store, from: &Path, run_bytes: usize) -> Res<Refreshed> {
    let path = from.join("manifest.json");
    let too_big = std::fs::metadata(&path).map(|m| m.len() > MAX_MANIFEST).unwrap_or(false);
    let bytes = if too_big { None } else { std::fs::read(&path).ok() };
    let Some(bytes) = bytes else { return Err(GovError::bad_source("refresh --from needs a folder that holds a manifest.json")) };
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| GovError::bad_source(format!("manifest.json is not a manifest: {e}")))?;
    apply(store, from, &manifest, run_bytes)
}

pub fn apply(store: &Store, base: &Path, manifest: &Manifest, run_bytes: usize) -> Res<Refreshed> {
    if let Some(unknown) = manifest.sources.keys().find(|k| info(k).is_none()) {
        return Err(GovError::bad_source(format!("the manifest names a source that does not exist: {unknown}")));
    }
    let (mut plan, mut metas) = (Vec::new(), BTreeMap::new());
    for source in &SOURCES {
        let Some(ms) = manifest.sources.get(source.name).filter(|_| BUILT.contains(&source.name)) else { continue };
        if !valid_date(&ms.as_of) || ms.files.is_empty() || ms.files.len() > 64 {
            return Err(GovError::bad_source(format!("{}: needs an asOf date (yyyy-mm-dd) and from 1 to 64 files", source.name)));
        }
        let mut hashed = Vec::new();
        for rel in &ms.files {
            let full = inside(ms.dir.as_deref().unwrap_or(base), rel)?;
            expand(rel, &full, 0, &mut hashed).map_err(GovError::bad_source)?;
        }
        // A folder in the manifest stands for the files in it.
        let files: Vec<PathBuf> = hashed.iter().map(|(_, path)| path.clone()).collect();
        let version = format!("{}-{}-{}", source.name, ms.as_of, hash_files(&hashed).map_err(GovError::bad_source)?);
        metas.insert(
            source.name.to_string(),
            SourceMeta {
                version,
                as_of: ms.as_of.clone(),
                as_of_kind: ms.as_of_kind.clone(),
                licence: ms.licence.clone().unwrap_or_else(|| source.licence.to_string()),
                attribution: source.attribution.to_string(),
                publisher: ms.publisher.clone().unwrap_or_else(|| source.publisher.to_string()),
                records: 0,
            },
        );
        plan.push(SourcePlan { name: source.name, files });
    }
    if plan.is_empty() {
        return Err(GovError::bad_source("the manifest names no source this build reads (abr, asic, asicBn, gnaf, abs, asgs)"));
    }
    let versions: Vec<&str> = metas.values().map(|m| m.version.as_str()).collect();
    let id = data_version(&versions);

    if store.has(&id) {
        store.activate(&id).map_err(GovError::refresh)?;
        let meta = crate::snap::Snap::open(store.snap_dir(&id))?.meta;
        return Ok(report(&meta, false));
    }
    std::fs::create_dir_all(store.snapshots()).map_err(GovError::refresh)?;
    let tmp = store.snapshots().join(format!("{id}.tmp{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(GovError::refresh)?;
    let mut finish = |tmp: &Path| -> Res<Meta> {
        let built = build(&plan, tmp, run_bytes).map_err(GovError::refresh)?;
        for (name, n) in &built.records {
            if let Some(m) = metas.get_mut(*name) {
                m.records = *n;
            }
        }
        let meta = Meta { format: FORMAT, id: id.clone(), sources: metas.clone(), dicts: built.dicts };
        let json = serde_json::to_vec(&meta).map_err(GovError::refresh)?;
        std::fs::write(tmp.join("meta.json"), json).map_err(GovError::refresh)?;
        Ok(meta)
    };
    let meta = match finish(&tmp) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
    };
    let target = store.snap_dir(&id);
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_dir_all(&tmp);
        if !store.has(&id) {
            return Err(GovError::refresh(e));
        }
    }
    store.activate(&id).map_err(GovError::refresh)?;
    Ok(report(&meta, true))
}

pub(crate) fn report(meta: &Meta, built: bool) -> Refreshed {
    let sources = SOURCES
        .iter()
        .filter_map(|i| {
            meta.sources.get(i.name).map(|s| RefreshedSource {
                source: i.name.to_string(),
                version: s.version.clone(),
                as_of: s.as_of.clone(),
                records: s.records,
            })
        })
        .collect();
    Refreshed { id: meta.id.clone(), built, sources }
}

/// The answer of a refresh: the data version, whether an index was built, `extra` (a JSON fragment such as
/// `,"downloaded":["asic"]`, or nothing) and each source's version, as-of date and record count.
pub fn report_json(done: &Refreshed, extra: &str) -> String {
    use crate::json::{key, quote};
    let mut out = String::from("{\"ok\":true,\"dataVersion\":");
    quote(&mut out, &done.id);
    out.push_str(&format!(",\"built\":{}{extra},\"sources\":[", done.built));
    for (i, s) in done.sources.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('{');
        let mut f = true;
        key(&mut out, &mut f, "source");
        quote(&mut out, &s.source);
        key(&mut out, &mut f, "dataVersion");
        quote(&mut out, &s.version);
        key(&mut out, &mut f, "asOf");
        quote(&mut out, &s.as_of);
        key(&mut out, &mut f, "records");
        out.push_str(&s.records.to_string());
        out.push('}');
    }
    out.push_str("]}");
    out
}
