//! The Syngle Observatory engine sidecar (fork crate; from spike A, #43).
//!
//! What it is, and the boundary it draws (ADR 0005):
//!
//! * **Data authority.** It owns the datasets: a `Session` (the fork's own engine) loads them with
//!   the fork's readers, so the same code the desktop app runs parses the same bytes.
//! * **Gateway.** It serves one localhost HTTP surface, with byte ranges: the canvas-only WASM
//!   view (the fork's `dist/canvas` or `dist/web` build), the embedding page and the dataset files.
//!   Byte ranges matter: a GeoParquet or FlatGeobuf "read in place" fetches its footer and its row
//!   groups by range.
//! * **Control channel.** It speaks JSON lines over stdin/stdout with the host process (SyngleCode's
//!   desktop process; the spike host before it). What crosses it: commands in, and the *project*
//!   (never the pixels or the feature buffers) out.
//! * **One launch, one port, one token.** The host passes `--token <secret>`; every request must
//!   carry it, as a `/t/<token>/…` path, `?token=<token>` or the `X-Observatory-Token` header, and
//!   the gateway binds the port the host chose or the one the OS picked. Without `--token` the
//!   gateway is open, which is what spike A's (#43) bench runs against.
//! * **The canvas page.** `/canvas/` is the engine's own canvas-only host page (ADR 0010): the page
//!   SyngleCode frames. It is compiled into this binary, so a deployment needs the wasm dist beside
//!   it and nothing else. It is the view's parent: it relays the fork's `rustgis` messages to its
//!   own host as `observatory` messages, hands the view a project, and reports the view's own
//!   per-frame samples (`?spike`) so a measurement is the view's own, not a proxy for it.
//!
//! Deliberately *not* in the engine: the map view. The engine never renders; the view never reads
//! the disk. See `agents/docs/research/observatory-engine-spike.md` in the Syngle-Ecosystem repo.
//!
//! Usage: `syngle-observatory-engine --root <repo> --data <dir> [--token <secret>] [--preload <url|path>]`
//!
//! stdout: one JSON object a line, `{"event": …}` — the protocol.
//! stderr: human/log lines — HTTP requests and their byte counts, notices, timings.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rustgis_engine::{Project, Session};
use serde_json::{Value, json};

/// Timestamps are the machine's wall clock: the host and this process compare them directly.
fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

struct Cfg {
    /// `<repo>/dist/web`: the trunk build of the WASM view.
    web: PathBuf,
    /// `<repo>/syngle-spike/host/view`: the page that embeds the view.
    host: PathBuf,
    /// Where the generated datasets live.
    data: PathBuf,
    /// Send cacheable headers for the view's hashed assets (`--cache`).
    cache: bool,
    /// The token this launch minted, when the host asked for one (`--token`): every request must
    /// carry it, as a `/t/<token>/…` path, `?token=<token>` or an `X-Observatory-Token` header.
    /// Without one the gateway serves openly, which is what the #43 bench runs against.
    token: Option<String>,
}

/// What the gateway served, for the report: how many requests, how many bytes, how many of them
/// were ranges (a "read in place" source is a long tail of small ranges — that is a cost of the
/// engine/view boundary, so it is measured rather than guessed).
#[derive(Default)]
struct Counters {
    requests: std::sync::atomic::AtomicU64,
    ranges: std::sync::atomic::AtomicU64,
    bytes: std::sync::atomic::AtomicU64,
}

/// Runs the sidecar: binds the loopback gateway, speaks the JSON-lines protocol on stdin/stdout
/// and returns the process exit code. `args` are the command line's arguments after the program
/// name.
pub fn run(args: &[String]) -> i32 {
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let root = PathBuf::from(opt("--root").unwrap_or_else(|| ".".to_string()));
    // `--cache` makes the view's content-hashed assets cacheable: the second launch reuses the
    // fetched (and code-cached) WebAssembly instead of re-downloading 32 MB. Off by default, so the
    // as-shipped cold-open numbers stay the pessimistic ones.
    let cfg = Arc::new(Cfg {
        web: opt("--web").map(PathBuf::from).unwrap_or_else(|| root.join("dist/web")),
        host: opt("--host").map(PathBuf::from).unwrap_or_else(|| root.join("syngle-spike/host/view")),
        data: opt("--data").map(PathBuf::from).unwrap_or_else(|| root.join("syngle-spike/data")),
        cache: args.iter().any(|a| a == "--cache"),
        token: opt("--token").filter(|t| !t.is_empty()),
    });
    let port_wanted: u16 = opt("--port").and_then(|p| p.parse().ok()).unwrap_or(0);
    let listener = match TcpListener::bind(("127.0.0.1", port_wanted)) {
        Ok(l) => l,
        Err(e) => {
            emit(&json!({"event": "error", "error": format!("could not listen: {e}")}));
            return 2;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let serve_cfg = Arc::clone(&cfg);
    let counters = Arc::new(Counters::default());
    let serve_counters = Arc::clone(&counters);
    // The project the view should open is also served as a document, so a host can point the view at
    // `?project=<url>` and skip the postMessage handshake entirely.
    let project = Arc::new(std::sync::Mutex::new(String::from("{}")));
    let serve_project = Arc::clone(&project);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let cfg = Arc::clone(&serve_cfg);
                    let counters = Arc::clone(&serve_counters);
                    let project = Arc::clone(&serve_project);
                    // A thread a connection: the view opens several at once, and the engine's own
                    // native load keeps fetching from this same server while it runs.
                    std::thread::spawn(move || {
                        let _ = handle(s, &cfg, &counters, &project);
                    });
                }
                Err(e) => log(&format!("accept: {e}")),
            }
        }
    });
    let pid = std::process::id();
    let base = format!("http://127.0.0.1:{port}");
    // `auth` tells the host the gateway is token-guarded; the token itself is the host's (it
    // minted it) and never travels back over this channel.
    emit(&json!({"event":"ready","port":port,"pid":pid,"base":base,"auth":cfg.token.is_some(),"t":now_ms()}));

    let mut session = Session::new(Project::default());
    if let Some(preload) = opt("--preload") {
        let kind = if preload.starts_with("http") { "url" } else { "path" };
        let out = open(&mut session, &preload, kind, None, true);
        emit(&json!({"event":"preloaded","t":now_ms(),"result":out}));
    }

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            emit(&json!({"event":"error","error":"not JSON","line":line}));
            continue;
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        match Command::parse(&msg) {
            Command::Ping => emit(&json!({"event":"pong","id":id,"t":now_ms()})),
            Command::Open { which, name, zoom } => {
                let Some((kind, target)) = which else {
                    emit(&json!({"event":"error","id":id,"error":"open needs url or path"}));
                    continue;
                };
                // Progress, not an answer: an id here would resolve the caller's promise early.
                emit(&json!({"event":"opening","kind":kind,"target":target,"t":now_ms()}));
                let out = open(&mut session, &target, kind, name.as_deref(), zoom);
                if let Some(text) = out.get("project").and_then(Value::as_str)
                    && let Ok(mut shared) = project.lock()
                {
                    *shared = text.to_string();
                }
                emit(&json!({"event":"opened","id":id,"t":now_ms(),"result":out}));
            }
            Command::Project => {
                let t = Instant::now();
                let text = serde_json::to_string(&session.project).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"));
                emit(
                    &json!({"event":"project","id":id,"t":now_ms(),"serializeMs":t.elapsed().as_secs_f64() * 1000.0,"bytes":text.len(),"text":text}),
                );
            }
            Command::Stats => {
                use std::sync::atomic::Ordering;
                let features: usize = session.project.map().layers.iter().map(|l| l.dataset.features.len()).sum();
                emit(&json!({
                    "event":"stats",
                    "id":id,
                    "t":now_ms(),
                    "layers":session.project.map().layers.len(),
                    "features":features,
                    "requests":counters.requests.load(Ordering::Relaxed),
                    "ranges":counters.ranges.load(Ordering::Relaxed),
                    "bytes":counters.bytes.load(Ordering::Relaxed),
                }))
            }
            Command::Quit => break,
            Command::Unknown(other) => emit(&json!({"event":"error","id":id,"error":format!("unknown command {other}")})),
        }
    }
    0
}

/// One line of the control protocol: what the host asks the engine to do.
#[derive(Debug, PartialEq)]
enum Command {
    Ping,
    /// Open a dataset. `which` is (kind, target) with kind `"url"` or `"path"`, or `None` when the
    /// line carried neither.
    Open {
        which: Option<(&'static str, String)>,
        name: Option<String>,
        zoom: bool,
    },
    /// The current project, serialised, for the view.
    Project,
    /// The load counters and what is open.
    Stats,
    Quit,
    Unknown(String),
}

impl Command {
    /// Reads one protocol line. A line without a `cmd` member is [`Command::Unknown`], like one
    /// naming a command this engine does not know.
    fn parse(msg: &Value) -> Command {
        match msg.get("cmd").and_then(Value::as_str).unwrap_or("") {
            "ping" => Command::Ping,
            "open" => {
                let which = msg
                    .get("url")
                    .and_then(Value::as_str)
                    .map(|u| ("url", u.to_string()))
                    .or_else(|| msg.get("path").and_then(Value::as_str).map(|p| ("path", p.to_string())));
                Command::Open {
                    which,
                    name: msg.get("name").and_then(Value::as_str).map(str::to_string),
                    zoom: msg.get("zoom").and_then(Value::as_bool).unwrap_or(true),
                }
            }
            "project" => Command::Project,
            "stats" => Command::Stats,
            "quit" => Command::Quit,
            other => Command::Unknown(other.to_string()),
        }
    }
}

/// Loads a dataset with the fork's own engine, the way the desktop app does. Answers with the
/// timings and the stats the report needs, plus the project the view should open.
fn open(session: &mut Session, target: &str, kind: &str, name: Option<&str>, zoom: bool) -> Value {
    let mut params = json!({"zoom": zoom});
    if let Some(name) = name {
        params["name"] = json!(name);
    }
    params[kind] = json!(target);
    // Two commands, one dataset: `layer.addUrl` for an address (the view will read it in place),
    // `layer.addData` for a file on disk.
    let command = if kind == "url" { "layer.addUrl" } else { "layer.addData" };
    let t = Instant::now();
    let result = session.run(command, &params);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    match result {
        Err(e) => json!({"ok": false, "loadMs": ms, "error": e}),
        Ok(value) => {
            let t2 = Instant::now();
            let text = serde_json::to_string(&session.project).unwrap_or_default();
            let serialize_ms = t2.elapsed().as_secs_f64() * 1000.0;
            json!({
                "ok": true,
                "loadMs": ms,
                "serializeMs": serialize_ms,
                "projectBytes": text.len(),
                "result": value,
                "project": text,
            })
        }
    }
}

// The working set is the host's to read (`Get-Process -Id <pid>`): the workspace forbids `unsafe`,
// and `K32GetProcessMemoryInfo` needs it, so the engine keeps no memory code at all. The `stats`
// reply carries what it does own: the layer count and the dataset's feature count.

fn emit(v: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn log(line: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "engine: {line}");
}

// --- the localhost gateway -------------------------------------------------------------------

/// The instrument the served view page carries: frame pacing and GPU submissions, nothing else.
/// Injected into `index.html` at serve time, so no fork file is changed for the measurement.
const INSTRUMENT: &str = include_str!("instrument.js");

/// A `Range` request, in the two shapes a reader uses: `bytes=a-b` / `bytes=a-` (a start), and
/// `bytes=-n` (the last n bytes — how a Parquet reader fetches the footer, and the reason a naive
/// "first n bytes" server answers `not a Parquet file`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum ByteRange {
    From { start: u64, end: Option<u64> },
    Last(u64),
}

struct Request {
    method: String,
    target: String,
    range: Option<ByteRange>,
    /// The `X-Observatory-Token` header, when the caller sent one (the app's own fetches do).
    token: Option<String>,
}

/// Whether a request may read the gateway. `None` cfg token: the gateway is open (the #43 bench).
/// The token may travel in the path (`/t/<token>/…`, how the view's own asset and range requests
/// carry it), in the query (`?token=…`) or in the header.
fn authorized(req: &Request, token: Option<&String>) -> bool {
    let Some(token) = token else { return true };
    if req.token.as_deref() == Some(token.as_str()) {
        return true;
    }
    if let Some((_, rest)) = req.target.split_once('?')
        && rest.split('&').any(|kv| kv == format!("token={token}"))
    {
        return true;
    }
    path_token(&req.target).is_some_and(|(t, _)| t == token)
}

/// The token in a `/t/<token>/…` path, and the path with that prefix removed. `None` when the path
/// carries no token prefix.
fn path_token(target: &str) -> Option<(&str, String)> {
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, format!("?{q}")),
        None => (target, String::new()),
    };
    let rest = path.strip_prefix("/t/")?;
    let (token, tail) = rest.split_once('/')?;
    if token.is_empty() {
        return None;
    }
    Some((token, format!("/{tail}{query}")))
}

/// A request target with the launch token's path prefix removed, when it carries this launch's.
fn route_target(target: &str, token: Option<&String>) -> String {
    match (token, path_token(target)) {
        (Some(token), Some((t, stripped))) if t == token => stripped,
        _ => target.to_string(),
    }
}

/// `/t/<token>` — the prefix the gateway's own URLs carry for a launch with a token.
fn token_prefix(token: Option<&String>) -> String {
    token.map(|t| format!("/t/{t}")).unwrap_or_default()
}

/// The address the canvas-only view is framed at: the launch token's path, then the flags the
/// requesting page asked for. `?project=1` makes the view fetch the project document itself
/// (`?project=<url>` is the fork's own flag); `?backend=webgl2` picks the WebGL2 backend;
/// `?spike` turns on the view's own per-frame samples, which is what the measurements read.
fn view_embed(target: &str, token: Option<&String>) -> String {
    let t = token_prefix(token);
    let mut embed = format!("{t}/web/?embed");
    if target.contains("backend=webgl2") {
        embed.push_str("&webgl");
    }
    if target.contains("project=1") {
        embed.push_str(&format!("&project={t}/project.json"));
    }
    if target.contains("spike") {
        embed.push_str("&spike");
    }
    embed
}

/// The engine's canvas-only host page (ADR 0010): the page SyngleCode frames. It is compiled into
/// this binary, so a deployment needs the wasm dist beside it and nothing else. The view's iframe
/// address is rewritten per request by [`view_embed`], like the spike host page's is.
const CANVAS_HTML: &str = include_str!("../host/index.html");
/// Its script: the parent half of the view's channel (see `host/canvas.js`).
const CANVAS_JS: &str = include_str!("../host/canvas.js");

fn handle(mut stream: TcpStream, cfg: &Cfg, counters: &Counters, project: &std::sync::Mutex<String>) -> std::io::Result<()> {
    use std::sync::atomic::Ordering;
    stream.set_nodelay(true).ok();
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head_bytes = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        head_bytes.extend_from_slice(&line);
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        if head_bytes.len() > 64 * 1024 {
            return Ok(());
        }
    }
    let text = String::from_utf8_lossy(&head_bytes).to_string();
    let mut lines = text.lines();
    let Some(first) = lines.next() else { return Ok(()) };
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut length = 0usize;
    let mut range = None;
    let mut token = None;
    for h in lines {
        let Some((k, v)) = h.split_once(':') else { continue };
        match k.trim().to_ascii_lowercase().as_str() {
            "range" => range = parse_range(v.trim()),
            "content-length" => length = v.trim().parse().unwrap_or(0),
            "x-observatory-token" => token = Some(v.trim().to_string()),
            _ => {}
        }
    }
    if length > 0 {
        let mut body = vec![0u8; length.min(8 << 20)];
        let _ = reader.read_exact(&mut body);
    }
    counters.requests.fetch_add(1, Ordering::Relaxed);
    if range.is_some() {
        counters.ranges.fetch_add(1, Ordering::Relaxed);
    }
    // A token-guarded gateway answers nothing else: without the launch token the request is
    // refused before any file is touched (the token may sit in the path, the query or the header).
    let mut req = Request { method, target, range, token };
    if !authorized(&req, cfg.token.as_ref()) {
        counters.requests.fetch_add(1, Ordering::Relaxed);
        log(&format!("{} {} 403", req.method, req.target));
        let h = head(403, "text/plain", 0, None, false);
        stream.write_all(h.as_bytes())?;
        stream.flush().ok();
        return Ok(());
    }
    // The `/t/<token>/` prefix the view's own requests carry is stripped for routing, which then
    // sees the same paths with or without a token.
    req.target = route_target(&req.target, cfg.token.as_ref());
    // What the APP asks for during a cold open, in order, is the evidence for the boundary: the
    // view fetches its own assets and the dataset, by range, from here.
    let t = Instant::now();
    let (status, headers, body) = route(&req, cfg, project);
    let is_404 = status == 404;
    if is_404 {
        log(&format!("{} {} 404", req.method, req.target));
    } else {
        log(&format!(
            "{} {} {} bytes range={:?} in {:.1} ms",
            req.method,
            req.target,
            headers.1,
            req.range,
            t.elapsed().as_secs_f64() * 1000.0
        ));
    }
    counters.bytes.fetch_add(headers.1 + headers.0.len() as u64, Ordering::Relaxed);
    stream.write_all(headers.0.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush().ok();
    Ok(())
}

/// Builds the whole response: (status, (head, content length), body).
fn route(req: &Request, cfg: &Cfg, project: &std::sync::Mutex<String>) -> (u16, (String, u64), Vec<u8>) {
    let (path, forced_type) = match req.target.split('?').next().unwrap_or("/") {
        "/" | "/host/" | "/host/index.html" => ("host/index.html".to_string(), Some("text/html; charset=utf-8")),
        // The engine's own canvas-only host page (ADR 0010): what SyngleCode frames. It is part of
        // this binary, so a deployment needs the wasm dist and nothing else beside it.
        "/canvas" | "/canvas/" | "/canvas/index.html" => {
            // The page's own addresses are relative, so the launch token's prefix comes from the
            // URL the host framed it with; its one view address is rewritten the same way the
            // spike host page's is.
            let bytes = CANVAS_HTML.replace("/web/?embed", &view_embed(&req.target, cfg.token.as_ref())).into_bytes();
            return (200, (head(200, "text/html; charset=utf-8", bytes.len() as u64, None, false), bytes.len() as u64), bytes);
        }
        "/canvas/canvas.js" => {
            let bytes = CANVAS_JS.as_bytes().to_vec();
            return (200, (head(200, "text/javascript", bytes.len() as u64, None, false), bytes.len() as u64), bytes);
        }
        "/health" => return (200, (head(200, "text/plain", 2, None, false), 2), b"ok".to_vec()),
        // The project the view opens, as a document: `?project=<this>` needs no postMessage round trip.
        "/project.json" => {
            let text = project.lock().map(|p| p.clone()).unwrap_or_default();
            let bytes = text.into_bytes();
            return (200, (head(200, "application/json", bytes.len() as u64, None, false), bytes.len() as u64), bytes);
        }
        "/spike/instrument.js" => {
            return (
                200,
                (head(200, "text/javascript", INSTRUMENT.len() as u64, None, false), INSTRUMENT.len() as u64),
                INSTRUMENT.as_bytes().to_vec(),
            );
        }
        other => {
            let decoded = percent_decode(other);
            if decoded.contains("..") {
                return (404, (head(404, "text/plain", 0, None, false), 0), Vec::new());
            }
            let trimmed = decoded.trim_start_matches('/').to_string();
            let (name, ty) = match trimmed.split_once('/') {
                Some(("web", rest)) => (format!("web/{rest}"), None),
                Some(("host", rest)) => (format!("host/{rest}"), None),
                Some(("data", rest)) => (format!("data/{rest}"), None),
                _ => (trimmed, None),
            };
            (name, ty)
        }
    };
    let (file, ty) = match path.split_once('/') {
        Some(("web", rest)) => {
            let rest = if rest.is_empty() { "index.html" } else { rest };
            (cfg.web.join(rest), forced_type.or_else(|| content_type(rest)))
        }
        Some(("host", rest)) => {
            let rest = if rest.is_empty() { "index.html" } else { rest };
            (cfg.host.join(rest), forced_type.or_else(|| content_type(rest)))
        }
        Some(("data", rest)) => (cfg.data.join(rest), forced_type.or_else(|| content_type(rest))),
        _ => (cfg.host.join(&path), forced_type),
    };
    let ty = ty.unwrap_or("application/octet-stream");
    let mut handle = match std::fs::File::open(&file) {
        Ok(f) => f,
        Err(e) => {
            log(&format!("{}: {e}", file.display()));
            return (404, (head(404, "text/plain", 0, None, false), 0), Vec::new());
        }
    };
    // The view's page, served with the instrument (nothing in the fork changes for the measurement)
    // and, on request, the WebGL2 backend instead of WebGPU: `?backend=webgl2` rewrites the iframe
    // address, so the choice costs the measured run nothing.
    let index = file.file_name().is_some_and(|n| n == "index.html");
    if index && (req.target.starts_with("/web") || req.target.starts_with("/host")) {
        let mut html = String::new();
        if handle.read_to_string(&mut html).is_ok() {
            let mut injected = html;
            if req.target.starts_with("/web") {
                injected = injected.replace("</head>", "<script src=\"/spike/instrument.js\"></script></head>");
            }
            // One replacement, so the flags compose: the host page's own query decides what
            // the view's iframe address carries. Every address the page hands the view carries
            // the launch token's path, so the view's own asset and range requests are served.
            let embed = view_embed(&req.target, cfg.token.as_ref());
            injected = injected.replace("/web/?embed", &embed);
            let bytes = injected.into_bytes();
            return (200, (head(200, ty, bytes.len() as u64, None, false), bytes.len() as u64), bytes);
        }
    }
    let size = handle.metadata().map(|m| m.len()).unwrap_or(0);
    let last = size.saturating_sub(1);
    let wanted = req.range.map(|r| match r {
        ByteRange::From { start, end } => (start.min(last), end.unwrap_or(last).min(last)),
        ByteRange::Last(n) => (size.saturating_sub(n), last),
    });
    if let Some((start, end)) = wanted {
        let body = read_slice(&mut handle, start, end);
        let n = body.len() as u64;
        let cr = format!("bytes {start}-{end}/{size}");
        let cacheable = cfg.cache && req.target.starts_with("/web/");
        return (206, (head(206, ty, n, Some(&cr), cacheable), n), body);
    }
    let body = read_slice(&mut handle, 0, size.saturating_sub(1));
    let n = body.len() as u64;
    let cacheable = cfg.cache && req.target.starts_with("/web/");
    (200, (head(200, ty, n, None, cacheable), n), body)
}

fn read_slice(file: &mut std::fs::File, start: u64, end: u64) -> Vec<u8> {
    if end < start {
        return Vec::new();
    }
    let want = end - start + 1;
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = vec![0u8; want as usize];
    let mut got = 0;
    while got < buf.len() {
        match file.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    buf.truncate(got);
    buf
}

fn head(status: u16, ty: &str, length: u64, content_range: Option<&str>, cache: bool) -> String {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        _ => "OK",
    };
    let mut h = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ty}\r\nContent-Length: {length}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Range, Content-Type\r\nAccess-Control-Expose-Headers: Content-Range, Accept-Ranges, Content-Length\r\nCache-Control: {}\r\nConnection: close\r\n",
        if cache { "public, max-age=31536000, immutable" } else { "no-store" }
    );
    if let Some(cr) = content_range {
        h.push_str(&format!("Content-Range: {cr}\r\n"));
    }
    h.push_str("\r\n");
    h
}

/// `Range: bytes=a-b`, `bytes=a-`, `bytes=-n` — a single range, which is all a reader asks for.
fn parse_range(value: &str) -> Option<ByteRange> {
    let rest = value.trim().strip_prefix("bytes=")?;
    let (a, b) = rest.split_once('-')?;
    if a.is_empty() {
        // `bytes=-n`: the *last* n bytes. The Parquet reader's footer request; answering it with the
        // first n bytes is what "not a Parquet file" means.
        let n: u64 = b.trim().parse().ok()?;
        return Some(ByteRange::Last(n));
    }
    let start: u64 = a.trim().parse().ok()?;
    let end = if b.trim().is_empty() { None } else { Some(b.trim().parse().ok()?) };
    Some(ByteRange::From { start, end })
}

fn content_type(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" | "webmanifest" => "application/json",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "fgb" => "application/octet-stream",
        "geoparquet" => "application/octet-stream",
        _ => return None,
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = hex {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// A request against `cfg`, with a real file behind `/data` so range answers are the bytes on
    /// disk, not a mock.
    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("syngle-obs-engine-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("data")).ok();
        dir
    }

    fn cfg(root: &Path) -> Cfg {
        Cfg { web: root.join("web"), host: root.join("host"), data: root.join("data"), cache: false, token: None }
    }

    /// The same roots, with the launch token a SyngleCode launch passes.
    fn cfg_with_token(root: &Path, token: &str) -> Cfg {
        Cfg { token: Some(token.to_string()), ..cfg(root) }
    }

    fn the_project() -> std::sync::Mutex<String> {
        std::sync::Mutex::new(String::from("{\"name\":\"test\"}"))
    }

    #[test]
    fn ranges_are_read_in_the_shapes_a_reader_asks_for() {
        assert_eq!(parse_range("bytes=10-20"), Some(ByteRange::From { start: 10, end: Some(20) }));
        assert_eq!(parse_range("bytes=10-"), Some(ByteRange::From { start: 10, end: None }));
        // `bytes=-n` is the *last* n bytes: the Parquet footer request.
        assert_eq!(parse_range("bytes=-100"), Some(ByteRange::Last(100)));
        for bad in ["", "items=1-2", "bytes=abc", "bytes=10", "bytes=a-b"] {
            assert_eq!(parse_range(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_last_n_range_returns_the_tail_of_the_file() {
        let root = tmp_dir("tail");
        std::fs::write(root.join("data/blob.bin"), b"0123456789").ok();
        let req = Request { method: "GET".into(), target: "/data/blob.bin".into(), range: parse_range("bytes=-4"), token: None };
        let (status, (head, len), body) = route(&req, &cfg(&root), &the_project());
        assert_eq!(status, 206);
        assert!(head.contains("Content-Range: bytes 6-9/10"));
        assert_eq!(len, 4);
        assert_eq!(body, b"6789");
    }

    #[test]
    fn a_whole_file_answer_is_200_and_carries_the_reader_headers() {
        let root = tmp_dir("whole");
        std::fs::write(root.join("data/notes.json"), b"hi").ok();
        let req = Request { method: "GET".into(), target: "/data/notes.json".into(), range: None, token: None };
        let (status, (head, len), body) = route(&req, &cfg(&root), &the_project());
        assert_eq!(status, 200);
        assert_eq!(len, 2);
        assert_eq!(body, b"hi");
        assert!(head.contains("Accept-Ranges: bytes"));
        assert!(head.contains("Access-Control-Allow-Origin: *"));
        assert!(head.contains("Content-Type: application/json"));
    }

    #[test]
    fn traversal_out_of_a_served_root_is_refused() {
        let root = tmp_dir("traversal");
        std::fs::write(root.join("secret.txt"), b"secret").ok();
        let req = Request { method: "GET".into(), target: "/data/%2e%2e/secret.txt".into(), range: None, token: None };
        let (status, _, body) = route(&req, &cfg(&root), &the_project());
        assert_eq!(status, 404);
        assert!(body.is_empty());
    }

    #[test]
    fn health_and_project_documents_answer_without_touching_the_disk() {
        let root = tmp_dir("meta");
        let project = the_project();
        let health = route(&Request { method: "GET".into(), target: "/health".into(), range: None, token: None }, &cfg(&root), &project);
        assert_eq!((health.0, health.2), (200, b"ok".to_vec()));
        let doc = route(&Request { method: "GET".into(), target: "/project.json".into(), range: None, token: None }, &cfg(&root), &project);
        assert_eq!(doc.0, 200);
        assert_eq!(doc.2, b"{\"name\":\"test\"}");
        assert!((doc.1).0.contains("Content-Type: application/json"));
    }

    #[test]
    fn the_protocol_reads_the_commands_the_host_sends() {
        assert_eq!(Command::parse(&json!({"cmd": "ping"})), Command::Ping);
        assert_eq!(Command::parse(&json!({"cmd": "project"})), Command::Project);
        assert_eq!(Command::parse(&json!({"cmd": "stats"})), Command::Stats);
        assert_eq!(Command::parse(&json!({"cmd": "quit"})), Command::Quit);
        assert_eq!(
            Command::parse(&json!({"cmd": "open", "url": "http://x/a.parquet", "zoom": false})),
            Command::Open { which: Some(("url", "http://x/a.parquet".into())), name: None, zoom: false }
        );
        assert_eq!(
            Command::parse(&json!({"cmd": "open", "path": "C:/d/a.fgb", "name": "roads"})),
            Command::Open { which: Some(("path", "C:/d/a.fgb".into())), name: Some("roads".into()), zoom: true }
        );
        // `url` wins when a line carries both, and a missing target stays missing for the caller
        // to answer with its own error.
        assert_eq!(
            Command::parse(&json!({"cmd": "open", "url": "u", "path": "p"})),
            Command::Open { which: Some(("url", "u".into())), name: None, zoom: true }
        );
        assert_eq!(Command::parse(&json!({"cmd": "open"})), Command::Open { which: None, name: None, zoom: true });
        assert_eq!(Command::parse(&json!({"cmd": "nope"})), Command::Unknown("nope".into()));
        assert_eq!(Command::parse(&json!({"id": 1})), Command::Unknown(String::new()));
    }

    #[test]
    fn content_types_cover_the_view_and_the_datasets() {
        assert_eq!(content_type("index.html"), Some("text/html; charset=utf-8"));
        assert_eq!(content_type("app_bg.WASM"), Some("application/wasm"));
        assert_eq!(content_type("points.fgb"), Some("application/octet-stream"));
        assert_eq!(content_type("no-extension"), None);
    }

    #[test]
    fn percent_decoding_leaves_bad_escapes_alone() {
        assert_eq!(percent_decode("/a%20b"), "/a b");
        assert_eq!(percent_decode("/a%2"), "/a%2");
        assert_eq!(percent_decode("/a%zz"), "/a%zz");
    }

    #[test]
    fn a_slice_reads_exactly_the_asked_range() {
        let root = tmp_dir("slice");
        let path = root.join("data/blob.bin");
        std::fs::write(&path, b"abcdef").ok();
        let mut f = std::fs::File::open(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read_slice(&mut f, 2, 4), b"cde");
        assert_eq!(read_slice(&mut f, 4, 2), b"");
        assert_eq!(read_slice(&mut f, 5, 99), b"f");
    }

    /// A request as the app's renderer sends it: no token in the path (the app puts it there
    /// itself when it opens a dataset), just what the gateway sees.
    fn req(target: &str) -> Request {
        Request { method: "GET".into(), target: target.into(), range: None, token: None }
    }

    #[test]
    fn an_open_gateway_serves_and_a_token_guarded_one_does_not() {
        let token = String::from("5f2a");
        // Without a token every request is served — the #43 bench runs this way.
        assert!(authorized(&req("/data/x.parquet"), None));
        assert!(!authorized(&req("/data/x.parquet"), Some(&token)));
        // The three shapes a request may carry the token in.
        assert!(authorized(&req("/t/5f2a/data/x.parquet"), Some(&token)));
        assert!(authorized(&req("/data/x.parquet?token=5f2a"), Some(&token)));
        assert!(authorized(&Request { token: Some("5f2a".into()), ..req("/data/x.parquet") }, Some(&token)));
        // Another launch's token, a prefix with nothing after it, a token inside a path: refused.
        for bad in ["/t/other/data/x.parquet", "/t//data/x.parquet", "/data/t/5f2a/x.parquet", "/data/x.parquet?x=5f2a"] {
            assert!(!authorized(&req(bad), Some(&token)), "{bad}");
        }
    }

    #[test]
    fn a_token_prefix_is_stripped_before_routing_and_another_launchs_is_not() {
        let token = String::from("5f2a");
        assert_eq!(route_target("/t/5f2a/data/x.parquet", Some(&token)), "/data/x.parquet");
        assert_eq!(route_target("/t/5f2a/web/index.html?x=1", Some(&token)), "/web/index.html?x=1");
        // Not this launch's: left alone (and refused by `authorized` before routing).
        assert_eq!(route_target("/t/other/x", Some(&token)), "/t/other/x");
        // No token configured: every path is a path.
        assert_eq!(route_target("/t/5f2a/x", None), "/t/5f2a/x");
    }

    #[test]
    fn the_views_address_carries_the_token_and_the_flags_that_were_asked_for() {
        let token = String::from("5f2a");
        assert_eq!(view_embed("/host/", None), "/web/?embed");
        assert_eq!(view_embed("/host/?project=1&spike=1", None), "/web/?embed&project=/project.json&spike");
        assert_eq!(view_embed("/canvas/?project=1&spike=1", Some(&token)), "/t/5f2a/web/?embed&project=/t/5f2a/project.json&spike");
        assert_eq!(view_embed("/canvas/?backend=webgl2", Some(&token)), "/t/5f2a/web/?embed&webgl");
    }

    #[test]
    fn the_canvas_page_is_the_engines_own_and_frames_the_token_guarded_view() {
        let root = tmp_dir("canvas");
        let (status, (_, len), body) = route(&req("/canvas/"), &cfg_with_token(&root, "5f2a"), &the_project());
        assert_eq!(status, 200);
        assert_eq!(len, body.len() as u64);
        let html = String::from_utf8_lossy(&body).to_string();
        assert!(html.contains("src=\"/t/5f2a/web/?embed\""), "{html}");
        // Its script is served from beside it, not from disk.
        let (js_status, (_, js_len), js) = route(&req("/canvas/canvas.js"), &cfg_with_token(&root, "5f2a"), &the_project());
        assert_eq!((js_status, js_len), (200, CANVAS_JS.len() as u64));
        assert_eq!(js, CANVAS_JS.as_bytes());
    }

    #[test]
    fn the_spike_host_page_still_gets_the_token_and_its_own_flags() {
        let root = tmp_dir("host-inject");
        std::fs::write(root.join("host/index.html"), "<iframe src=\"/web/?embed\"></iframe><head></head>").ok();
        let (status, _, body) = route(&req("/host/?project=1&spike=1"), &cfg_with_token(&root, "5f2a"), &the_project());
        assert_eq!(status, 200);
        let html = String::from_utf8_lossy(&body).to_string();
        assert!(html.contains("src=\"/t/5f2a/web/?embed&project=/t/5f2a/project.json&spike\""), "{html}");
    }
}
