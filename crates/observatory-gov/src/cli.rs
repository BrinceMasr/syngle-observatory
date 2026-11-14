//! `syngle-map gov <verb> ...` as a function: arguments in, one line of JSON and an exit code out. The binary prints the
//! line; tests call this directly. Exit 0 for an answer, 2 for a refusal (`ok:false` and a fixed code), never a panic.

use crate::abnlive;
use crate::envelope::{self, Answer};
use crate::error::{GovError, Res};
use crate::net::Net;
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

/// What the network verbs use besides the data folder: the ABN Lookup GUID, for `gov abn --live` only (never printed, stored or put
/// in an answer), and the hosts to fetch from (the official ones when `None`; tests point it at a local server). It is apart from
/// `Env` so that code building an `Env` by its fields keeps compiling.
#[derive(Default)]
pub struct Online {
    pub abn_guid: Option<String>,
    pub net: Option<Net>,
}

impl Online {
    pub fn from_process() -> Online {
        Online { abn_guid: std::env::var("SYNGLE_ABN_LOOKUP_GUID").ok(), net: None }
    }

    fn net(&self) -> std::borrow::Cow<'_, Net> {
        match &self.net {
            Some(n) => std::borrow::Cow::Borrowed(n),
            None => std::borrow::Cow::Owned(Net::official()),
        }
    }
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

const VALUE_FLAGS: [&str; 13] = [
    "--data-dir",
    "--data-version",
    "--from",
    "--source",
    "--flow",
    "--state",
    "--limit",
    "--lat",
    "--lon",
    "--filter",
    "--id",
    "--last",
    "--to",
];

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

    /// Every value of a flag given more than once (a flag with no value counts as an empty one).
    fn values(&self, name: &str) -> Vec<&str> {
        self.options.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_deref().unwrap_or("")).collect()
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

/// The names a flag lists (`--source asic,gnaf`), without repeats; a flag with no names is refused.
fn names(p: &Parsed, flag: &str) -> Res<Vec<String>> {
    if !p.present(flag) {
        return Ok(Vec::new());
    }
    let mut out: Vec<String> = Vec::new();
    for n in p.value(flag).unwrap_or_default().split(',').map(str::trim).filter(|n| !n.is_empty()) {
        if !out.iter().any(|o| o == n) {
            out.push(n.to_string());
        }
    }
    if out.is_empty() {
        return Err(GovError::bad_source(format!("{flag} needs at least one name")));
    }
    Ok(out)
}

/// `gov refresh --from <folder>` builds from files you hold; `--source`, `--flow` and `--check` (or nothing at all, for what is
/// already turned on) fetch from the official hosts. Exit 10 is `--check` finding something newer.
fn refresh_verb(p: &Parsed, env: &Env, online: &Online) -> Res<(String, u8)> {
    let store = store(p, env)?;
    let network = p.present("--source") || p.present("--check") || p.present("--flow");
    if let Some(from) = p.value("--from") {
        if network {
            return Err(GovError::bad_source(
                "--from builds from files you hold; --source, --flow and --check use the network: give one or the other",
            ));
        }
        let done = refresh::from_dir(&store, std::path::Path::new(from), env.sort_run_bytes)?;
        return Ok((refresh::report_json(&done, ""), 0));
    }
    if p.present("--from") {
        return Err(GovError::refresh("--from needs a folder with a manifest.json"));
    }
    let net = online.net();
    let opts = crate::online::Opts { sources: names(p, "--source")?, flows: names(p, "--flow")?, check: p.present("--check") };
    if opts.check { crate::online::check(&store, &net, &opts) } else { crate::online::refresh(&store, &net, &opts, env.sort_run_bytes) }
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
        "address" => {
            let (id, text) = (p.value("--id"), rest.join(" "));
            if p.present("--id") && id.is_none() {
                return Err(GovError::new("empty-query", "--id needs a G-NAF address PID")
                    .with_query("{\"verb\":\"address\",\"id\":\"\"}".to_string()));
            }
            crate::address::address(&snap, &text, id, limit(p)?)?
        }
        "stats" => match rest.first().map(String::as_str) {
            Some("find") => crate::stats::find(&snap, &rest.get(1..).unwrap_or_default().join(" "))?,
            Some("get") => crate::stats::get(
                &snap,
                &crate::stats::Get {
                    flow: rest.get(1).map_or("", String::as_str),
                    filters: p.values("--filter"),
                    from: p.value("--from"),
                    to: p.value("--to"),
                    last: p.present("--last").then(|| p.value("--last").unwrap_or("")),
                },
            )?,
            _ => {
                return Err(GovError::new(
                    "unknown-verb",
                    "stats find <words> | stats get <dataflow> [--filter D=V] [--from P] [--to P] [--last N]",
                )
                .with_query("{\"verb\":\"stats\"}".to_string()));
            }
        },
        "area" => crate::area::area(&snap, p.value("--lat"), p.value("--lon"))?,
        other => return Err(GovError::new("no-data", format!("gov {other} needs a source this snapshot does not hold"))),
    };
    Ok(envelope::success(&snap.meta, &answer))
}

fn dispatch(args: &[String], env: &Env, online: &Online) -> Res<(String, u8)> {
    let p = Parsed::new(args);
    match p.positional.first().map(String::as_str) {
        Some("refresh") => refresh_verb(&p, env, online),
        Some("status") => {
            let store = store(&p, env)?;
            let snap = open(&p, &store)?;
            Ok((envelope::status(&snap.meta, &store.retained()), 0))
        }
        Some("abn") if p.present("--live") => {
            abnlive::lookup(&online.net(), online.abn_guid.as_deref(), p.positional.get(1..).unwrap_or_default()).map(|line| (line, 0))
        }
        Some(verb @ ("abn" | "company" | "business-names" | "name" | "address" | "stats" | "area")) => {
            query_verb(verb, &p, env).map(|line| (line, 0))
        }
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
    run_with(args, env, &Online::from_process())
}

/// `run` with the GUID and hosts given instead of read from the process (the tests' way in).
pub fn run_with(args: &[String], env: &Env, online: &Online) -> Outcome {
    match dispatch(args, env, online) {
        Ok((line, exit)) => Outcome { stdout: format!("{line}\n"), exit },
        Err(e) => Outcome { stdout: format!("{}\n", envelope::refusal(&e)), exit: 2 },
    }
}
