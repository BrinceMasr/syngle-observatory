//! `syngle-map gov <verb> ...` as a function: arguments in, one line of JSON and an exit code out. The binary prints the
//! line; tests call this directly. Exit 0 for an answer, 2 for a refusal (`ok:false` and a fixed code), never a panic.

use crate::envelope::{self, Answer};
use crate::error::{GovError, Res};
use crate::json::{key, quote};
use crate::query;
use crate::refresh;
use crate::snap::Snap;
use crate::store::Store;
use std::path::PathBuf;

pub struct Env {
    /// `SYNGLE_GOV_DIR`.
    pub gov_dir: Option<PathBuf>,
    /// The operating system's local data folder; the data lives in `syngle-map/gov` under it.
    pub local_data: Option<PathBuf>,
    /// How much a sort holds in memory before it writes a run to disk.
    pub sort_run_bytes: usize,
}

impl Env {
    pub fn from_process() -> Env {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let local_data = if cfg!(windows) {
            var("LOCALAPPDATA")
        } else if cfg!(target_os = "macos") {
            var("HOME").map(|h| h.join("Library").join("Application Support"))
        } else {
            var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("share")))
        };
        Env { gov_dir: var("SYNGLE_GOV_DIR"), local_data, sort_run_bytes: 256 << 20 }
    }
}

pub struct Outcome {
    pub stdout: String,
    pub exit: u8,
}

const VALUE_FLAGS: [&str; 12] =
    ["--data-dir", "--data-version", "--from", "--source", "--state", "--limit", "--lat", "--lon", "--filter", "--id", "--last", "--to"];

struct Parsed {
    positional: Vec<String>,
    options: Vec<(String, Option<String>)>,
}

impl Parsed {
    fn new(args: &[String]) -> Parsed {
        let (mut positional, mut options) = (Vec::new(), Vec::new());
        let mut i = 0;
        while let Some(a) = args.get(i) {
            i += 1;
            if a.len() > 2 && a.starts_with("--") {
                let value = if VALUE_FLAGS.contains(&a.as_str()) {
                    let v = args.get(i).cloned();
                    i += usize::from(v.is_some());
                    v
                } else {
                    None
                };
                options.push((a.clone(), value));
            } else {
                positional.push(a.clone());
            }
        }
        Parsed { positional, options }
    }

    fn present(&self, name: &str) -> bool {
        self.options.iter().any(|(k, _)| k == name)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.options.iter().find(|(k, _)| k == name).and_then(|(_, v)| v.as_deref())
    }
}

fn limit(p: &Parsed) -> Res<usize> {
    if !p.present("--limit") {
        return Ok(5);
    }
    let n = p.value("--limit").and_then(|v| v.trim().parse::<f64>().ok()).filter(|n| n.is_finite() && *n >= 1.0);
    n.map(|n| n.floor().min(50.0) as usize).ok_or_else(|| GovError::new("invalid-limit", "--limit must be a positive number"))
}

fn store(p: &Parsed, env: &Env) -> Res<Store> {
    let root = env
        .gov_dir
        .clone()
        .or_else(|| p.value("--data-dir").filter(|d| !d.is_empty()).map(PathBuf::from))
        .or_else(|| env.local_data.as_ref().map(|d| d.join("syngle-map").join("gov")));
    root.map(Store::new).ok_or_else(|| GovError::new("no-data", "there is no data folder: set SYNGLE_GOV_DIR or pass --data-dir"))
}

/// The snapshot a query reads: the one named by `--data-version`, else the current one.
fn open(p: &Parsed, store: &Store) -> Res<Snap> {
    let id = match p.value("--data-version") {
        Some(v) if store.has(v) => v.to_string(),
        Some(v) => return Err(GovError::new("unknown-data-version", format!("no retained snapshot {}", crate::json::echo(v)))),
        None => store.current_id().filter(|id| store.has(id)).ok_or_else(GovError::no_data)?,
    };
    Snap::open(store.snap_dir(&id))
}

fn refresh_verb(p: &Parsed, env: &Env) -> Res<String> {
    let store = store(p, env)?;
    let Some(from) = p.value("--from") else {
        return Err(GovError::refresh("refresh needs --from <folder with manifest.json> in this build"));
    };
    let done = refresh::from_dir(&store, std::path::Path::new(from), env.sort_run_bytes)?;
    let mut out = String::from("{\"ok\":true,\"dataVersion\":");
    quote(&mut out, &done.id);
    out.push_str(&format!(",\"built\":{},\"sources\":[", done.built));
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
    Ok(out)
}

fn query_verb(verb: &str, p: &Parsed, env: &Env) -> Res<String> {
    let store = store(p, env)?;
    let snap = open(p, &store)?;
    let rest = p.positional.get(1..).unwrap_or_default();
    let answer: Answer = match verb {
        "abn" => query::abn(&snap, rest)?,
        "company" => query::company(&snap, rest)?,
        "business-names" => query::business_names(&snap, rest)?,
        "name" => {
            let state = match (p.present("--state"), p.value("--state")) {
                (true, None) => return Err(GovError::new("invalid-filter", "--state needs a value, such as NSW")),
                (_, v) => v,
            };
            query::name(&snap, &rest.join(" "), state, p.present("--active"), limit(p)?)?
        }
        other => return Err(GovError::new("no-data", format!("gov {other} needs a source this snapshot does not hold"))),
    };
    Ok(envelope::success(&snap.meta, &answer))
}

fn dispatch(args: &[String], env: &Env) -> Res<String> {
    let p = Parsed::new(args);
    match p.positional.first().map(String::as_str) {
        Some("refresh") => refresh_verb(&p, env),
        Some("status") => {
            let store = store(&p, env)?;
            let snap = open(&p, &store)?;
            Ok(envelope::status(&snap.meta, &store.retained()))
        }
        Some(verb @ ("abn" | "company" | "business-names" | "name" | "address" | "stats" | "area")) => query_verb(verb, &p, env),
        Some(other) => Err(GovError::new(
            "unknown-verb",
            format!("no verb {}; refresh, status, abn, name, company, business-names, address, stats, area", crate::json::echo(other)),
        )),
        None => {
            Err(GovError::new("unknown-verb", "give a verb: refresh, status, abn, name, company, business-names, address, stats, area"))
        }
    }
}

pub fn run(args: &[String], env: &Env) -> Outcome {
    match dispatch(args, env) {
        Ok(line) => Outcome { stdout: format!("{line}\n"), exit: 0 },
        Err(e) => Outcome { stdout: format!("{}\n", envelope::refusal(&e)), exit: 2 },
    }
}
