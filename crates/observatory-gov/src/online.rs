//! `gov refresh [--source a,b] [--flow ID,..] [--check]`: the sources kept up to date from their official hosts.
//!
//! What is turned on, and the release of each, is `downloads/state.json`; a release's files live in `downloads/<source>/<id>/`,
//! where the id names the remote files (address, size, date). Nothing is ever written over a release: a new one gets a folder of
//! its own, the snapshot is built from the folders the state points at and switched, and only then does the state move and the
//! old folders go. So a killed or failed run leaves the snapshot answering as it was, the state as it was, and the files that
//! were already whole in their folder, which the next run reuses (a half-written file resumes by range).

use crate::absfetch::{self, DEFAULT_FLOWS};
use crate::ckan::{self, Latest};
use crate::download::{self, Want};
use crate::error::{GovError, Res};
use crate::json::{echo, key, quote};
use crate::net::Net;
use crate::refresh::{self, Manifest, ManifestSource};
use crate::snap::Snap;
use crate::sources::SOURCES;
use crate::store::{Store, write_atomic};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

/// A source with no size to count (the ASGS server does not say) is counted as this much.
const UNKNOWN_SIZE: u64 = 1 << 30;
/// What one ABS dataflow is counted as.
const ABS_ESTIMATE: u64 = 64 << 20;

pub struct Opts {
    pub sources: Vec<String>,
    pub flows: Vec<String>,
    pub check: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct HeldFile {
    name: String,
    url: String,
    size: u64,
    #[serde(rename = "lastModified")]
    last_modified: String,
}

/// The release of one source that is indexed.
#[derive(Serialize, Deserialize, Clone)]
struct Held {
    #[serde(rename = "asOf")]
    as_of: String,
    dir: String,
    #[serde(default)]
    files: Vec<HeldFile>,
    #[serde(default)]
    flows: Vec<String>,
    #[serde(default)]
    fetched: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    #[serde(default)]
    sources: BTreeMap<String, Held>,
}

fn downloads(store: &Store) -> std::path::PathBuf {
    store.root.join("downloads")
}

fn is_release_id(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The state file, with every entry that is not what this program writes dropped: it names folders that are joined to the data
/// folder and, for old releases, removed, so a damaged or hand-edited file must never lead outside `downloads/<source>/<id>`.
fn load(root: &Path) -> State {
    let mut state: State = fs::read(root.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    state.sources.retain(|name, h| {
        SOURCES.iter().any(|s| s.name == name)
            && is_release_id(&h.dir)
            && h.files.iter().all(|f| crate::ckan::safe_name(&f.name))
            && h.flows.iter().all(|f| absfetch::valid_flow(f))
    });
    state
}

/// The names given, as the sources they are, in the order sources are listed in.
fn named(names: &[String]) -> Res<Vec<&'static str>> {
    let mut out: Vec<&'static str> = Vec::new();
    for n in names {
        let Some(info) = SOURCES.iter().find(|s| s.name == n) else {
            return Err(GovError::bad_source(format!("no source {}; the sources are abr, asic, asicBn, gnaf, abs, asgs", echo(n))));
        };
        if !out.contains(&info.name) {
            out.push(info.name);
        }
    }
    out.sort_by_key(|n| SOURCES.iter().position(|s| s.name == *n));
    Ok(out)
}

fn identity(l: &Latest, flows: &[String]) -> String {
    let mut h = Sha256::new();
    h.update(l.source.as_bytes());
    if l.fetched {
        h.update(l.as_of.as_bytes());
        for f in flows {
            h.update(format!("|{f}").as_bytes());
        }
    }
    for f in &l.files {
        h.update(format!("|{}|{}|{}", f.url, f.size, f.last_modified).as_bytes());
    }
    h.finalize().iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn same(h: &Held, l: &Latest) -> bool {
    h.files.len() == l.files.len()
        && h.files
            .iter()
            .zip(&l.files)
            .all(|(a, b)| a.url == b.url && a.last_modified == b.last_modified && (b.size == 0 || a.size == b.size))
}

fn stamp<'a>(files: impl Iterator<Item = (&'a str, u64)>) -> (Option<String>, u64) {
    let (mut last, mut total) = (None::<&str>, 0);
    for (modified, size) in files {
        last = last.max(Some(modified));
        total += size;
    }
    (last.map(str::to_string), total)
}

fn stamp_json(out: &mut String, as_of: &str, last_modified: Option<&str>, size: u64, files: usize) {
    out.push('{');
    let mut f = true;
    key(out, &mut f, "asOf");
    quote(out, as_of);
    key(out, &mut f, "lastModified");
    match last_modified {
        Some(m) => quote(out, m),
        None => out.push_str("null"),
    }
    key(out, &mut f, "size");
    out.push_str(&size.to_string());
    key(out, &mut f, "files");
    out.push_str(&files.to_string());
    out.push('}');
}

/// `--check`: what is newer than what is indexed. Downloads nothing; exit 0 when nothing is, 10 when something is.
pub fn check(store: &Store, net: &Net, opts: &Opts) -> Res<(String, u8)> {
    let state = load(&downloads(store));
    let explicit = !opts.sources.is_empty();
    let scope: Vec<&'static str> = if explicit {
        named(&opts.sources)?
    } else if state.sources.is_empty() {
        SOURCES.iter().map(|s| s.name).collect()
    } else {
        SOURCES.iter().map(|s| s.name).filter(|n| state.sources.contains_key(*n)).collect()
    };
    let (mut rows, mut updates) = (String::new(), Vec::new());
    for (i, source) in scope.iter().enumerate() {
        let latest = ckan::latest(net, source)?;
        let held = state.sources.get(*source);
        let status = match held {
            None => "not-installed",
            Some(_) if latest.fetched => "unknown",
            Some(h) if same(h, &latest) => "current",
            Some(_) => "update",
        };
        if status == "update" || (status == "not-installed" && explicit) {
            updates.push(*source);
        }
        rows.push_str(if i == 0 { "{" } else { ",{" });
        let mut f = true;
        key(&mut rows, &mut f, "source");
        quote(&mut rows, source);
        key(&mut rows, &mut f, "status");
        quote(&mut rows, status);
        key(&mut rows, &mut f, "installed");
        match held {
            Some(h) => {
                let (modified, size) = stamp(h.files.iter().map(|f| (f.last_modified.as_str(), f.size)));
                stamp_json(&mut rows, &h.as_of, modified.as_deref(), size, h.files.len());
            }
            None => rows.push_str("null"),
        }
        key(&mut rows, &mut f, "latest");
        let (modified, size) = stamp(latest.files.iter().map(|f| (f.last_modified.as_str(), f.size)));
        stamp_json(&mut rows, &latest.as_of, modified.as_deref(), size, latest.files.len());
        rows.push('}');
    }
    let mut out = format!("{{\"ok\":true,\"check\":true,\"upToDate\":{},\"updates\":[", updates.is_empty());
    for (i, u) in updates.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        quote(&mut out, u);
    }
    out.push_str("],\"sources\":[");
    out.push_str(&rows);
    out.push_str("]}");
    Ok((out, if updates.is_empty() { 0 } else { 10 }))
}

fn lock(root: &Path) -> Res<File> {
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(root.join(".lock")).map_err(GovError::refresh)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(GovError::refresh("another refresh is running on this data folder")),
        Err(fs::TryLockError::Error(e)) => Err(GovError::refresh(e)),
    }
}

fn need(l: &Latest, dir: &Path, flows: &[String]) -> u64 {
    if l.fetched {
        return flows.len() as u64 * ABS_ESTIMATE;
    }
    let left = |f: &crate::ckan::Remote| {
        let have = fs::metadata(dir.join(&f.name)).map(|m| m.len()).unwrap_or(0);
        (if f.size == 0 { UNKNOWN_SIZE } else { f.size }).saturating_sub(have)
    };
    l.files.iter().map(left).sum()
}

/// Brings one source's release into its folder, whole and checked.
fn get(net: &Net, root: &Path, l: &Latest, flows: &[String]) -> Res<Held> {
    let id = identity(l, flows);
    let dir = root.join(l.source).join(&id);
    let mut files = Vec::new();
    if l.fetched {
        if !dir.is_dir() {
            absfetch::fetch(net, &dir, flows)?;
        }
    } else {
        for f in &l.files {
            let want = Want { url: &f.url, name: &f.name, size: f.size, md5: f.md5.as_deref(), tag: &id };
            let (_, size) = download::fetch(net, &want, &dir)?;
            files.push(HeldFile { name: f.name.clone(), url: f.url.clone(), size, last_modified: f.last_modified.clone() });
        }
    }
    Ok(Held { as_of: l.as_of.clone(), dir: id, files, flows: if l.fetched { flows.to_vec() } else { Vec::new() }, fetched: l.fetched })
}

fn manifest(root: &Path, held: &BTreeMap<String, Held>) -> Manifest {
    let sources = held
        .iter()
        .map(|(name, h)| {
            let files = if h.fetched {
                vec!["dataflows.json".to_string(), "data".to_string()]
            } else {
                h.files.iter().map(|f| f.name.clone()).collect()
            };
            let ms = ManifestSource {
                as_of: h.as_of.clone(),
                files,
                licence: None,
                publisher: None,
                as_of_kind: h.fetched.then(|| "fetched".to_string()),
                dir: Some(root.join(name).join(&h.dir)),
            };
            (name.clone(), ms)
        })
        .collect();
    Manifest { sources }
}

/// Removes the folders of releases the state no longer points at.
fn tidy(root: &Path, state: &State) {
    for (source, held) in &state.sources {
        let Ok(entries) = fs::read_dir(root.join(source)) else { continue };
        for e in entries.flatten() {
            if e.file_name().to_string_lossy() != held.dir {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

/// The flows to fetch: the ones named, else the ones already held, else the default list.
fn flows_for(opts: &Opts, state: &State) -> Res<Vec<String>> {
    let held = state.sources.get("abs").map(|h| h.flows.clone()).filter(|f| !f.is_empty());
    let flows = if opts.flows.is_empty() { held.unwrap_or_else(|| DEFAULT_FLOWS.map(String::from).to_vec()) } else { opts.flows.clone() };
    match flows.iter().find(|f| !absfetch::valid_flow(f)) {
        Some(bad) => Err(GovError::bad_source(format!("{} is not an ABS dataflow id", echo(bad)))),
        None => Ok(flows),
    }
}

pub fn refresh(store: &Store, net: &Net, opts: &Opts, run_bytes: usize) -> Res<(String, u8)> {
    let root = downloads(store);
    fs::create_dir_all(&root).map_err(GovError::refresh)?;
    let _lock = lock(&root)?;
    let mut state = load(&root);
    let scope = if opts.sources.is_empty() {
        SOURCES.iter().map(|s| s.name).filter(|n| state.sources.contains_key(*n)).collect()
    } else {
        named(&opts.sources)?
    };
    if scope.is_empty() {
        return Err(GovError::new(
            "no-data",
            "no source is turned on: name one, for example gov refresh --source asic (the sources are abr, asic, asicBn, gnaf, abs, asgs)",
        ));
    }
    if !opts.flows.is_empty() && !scope.contains(&"abs") {
        return Err(GovError::bad_source("--flow names ABS dataflows: add --source abs"));
    }
    // An index built with `--from` holds sources this refresh would leave out, since it builds from what it downloaded.
    let current = store.current_id().filter(|id| store.has(id)).and_then(|id| Snap::open(store.snap_dir(&id)).ok());
    if let Some(snap) = &current {
        let dropped: Vec<&str> =
            snap.meta.sources.keys().map(String::as_str).filter(|k| !state.sources.contains_key(*k) && !scope.contains(k)).collect();
        if !dropped.is_empty() {
            return Err(GovError::refresh(format!(
                "the index holds {} from files given with --from, which this refresh would leave out: name them too, for example --source {},{}",
                dropped.join(", "),
                dropped.join(","),
                scope.join(",")
            )));
        }
    }
    let flows = flows_for(opts, &state)?;
    let latest = scope.iter().map(|s| ckan::latest(net, s)).collect::<Res<Vec<_>>>()?;
    let todo: Vec<&Latest> = latest.iter().filter(|l| l.fetched || !state.sources.get(l.source).is_some_and(|h| same(h, l))).collect();
    if todo.is_empty()
        && let Some(snap) = &current
    {
        return Ok((refresh::report_json(&refresh::report(&snap.meta, false), ",\"upToDate\":true"), 0));
    }
    let wanted: u64 = todo.iter().map(|l| need(l, &root.join(l.source).join(identity(l, &flows)), &flows)).sum();
    let free = fs4::available_space(&root).map_err(GovError::refresh)?;
    if wanted.saturating_mul(5) / 2 > free {
        return Err(GovError::refresh(format!(
            "not enough free disk space: {} MB to download needs {} MB free (2.5 times), and {} MB is free",
            wanted >> 20,
            (wanted.saturating_mul(5) / 2) >> 20,
            free >> 20
        )));
    }
    let mut next = state.sources.clone();
    for l in &todo {
        next.insert(l.source.to_string(), get(net, &root, l, &flows)?);
    }
    let done = refresh::apply(store, &root, &manifest(&root, &next), run_bytes)?;
    state.sources = next;
    let json = serde_json::to_vec_pretty(&state).map_err(GovError::refresh)?;
    write_atomic(&root.join("state.json"), &json).map_err(GovError::refresh)?;
    tidy(&root, &state);
    let names: Vec<String> = todo.iter().map(|l| format!("\"{}\"", l.source)).collect();
    Ok((refresh::report_json(&done, &format!(",\"downloaded\":[{}]", names.join(","))), 0))
}
