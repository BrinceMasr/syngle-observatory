//! Spike A (#43): the native engine sidecar.
//!
//! What it is, and the boundary it draws:
//!
//! * **Data authority.** It owns the datasets: a `Session` (the fork's own engine) loads them with
//!   the fork's readers, so the same code the desktop app runs parses the same bytes.
//! * **Gateway.** It serves one localhost HTTP surface, with byte ranges: the WASM view (the fork's
//!   `dist/web` build), the embedding page and the dataset files. Byte ranges matter: a GeoParquet
//!   or FlatGeobuf "read in place" fetches its footer and its row groups by range.
//! * **Control channel.** It speaks JSON lines over stdin/stdout with the Electron main process.
//!   What crosses it: commands in, and the *project* (never the pixels or the feature buffers) out.
//!
//! Deliberately *not* in the engine: the map view. The engine never renders; the view never reads
//! the disk. See the spike report for the rationale.
//!
//! Usage: `syngle-spike-engine --root <repo> --data <dir> [--preload <url|path>]`
//!
//! stdout: one JSON object a line, `{"event": …}` — the protocol.
//! stderr: human/log lines — HTTP requests and their byte counts, notices, timings.

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    });
    let port_wanted: u16 = opt("--port").and_then(|p| p.parse().ok()).unwrap_or(0);
    let listener = match TcpListener::bind(("127.0.0.1", port_wanted)) {
        Ok(l) => l,
        Err(e) => {
            emit(&json!({"event": "error", "error": format!("could not listen: {e}")}));
            std::process::exit(2);
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
    emit(&json!({"event":"ready","port":port,"pid":pid,"base":base,"t":now_ms()}));

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
        let cmd = msg.get("cmd").and_then(Value::as_str).unwrap_or("");
        match cmd {
            "ping" => emit(&json!({"event":"pong","id":id,"t":now_ms()})),
            "open" => {
                let which = msg
                    .get("url")
                    .and_then(Value::as_str)
                    .map(|u| (u, "url"))
                    .or_else(|| msg.get("path").and_then(Value::as_str).map(|p| (p, "path")));
                let Some((target, kind)) = which else {
                    emit(&json!({"event":"error","id":id,"error":"open needs url or path"}));
                    continue;
                };
                let name = msg.get("name").and_then(Value::as_str);
                let zoom = msg.get("zoom").and_then(Value::as_bool).unwrap_or(true);
                // Progress, not an answer: an id here would resolve the caller's promise early.
                emit(&json!({"event":"opening","kind":kind,"target":target,"t":now_ms()}));
                let out = open(&mut session, target, kind, name, zoom);
                if let Some(text) = out.get("project").and_then(Value::as_str)
                    && let Ok(mut shared) = project.lock()
                {
                    *shared = text.to_string();
                }
                emit(&json!({"event":"opened","id":id,"t":now_ms(),"result":out}));
            }
            "project" => {
                let t = Instant::now();
                let text = serde_json::to_string(&session.project).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"));
                emit(
                    &json!({"event":"project","id":id,"t":now_ms(),"serializeMs":t.elapsed().as_secs_f64() * 1000.0,"bytes":text.len(),"text":text}),
                );
            }
            "stats" => {
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
            "quit" => break,
            other => emit(&json!({"event":"error","id":id,"error":format!("unknown command {other}")})),
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

/// The working set is the host's to read (`Get-Process -Id <pid>`): the workspace forbids `unsafe`,
/// and `K32GetProcessMemoryInfo` needs it, so the engine keeps no memory code at all.
/// The `stats` reply carries what it does own: the layer count and the dataset's feature count.

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
#[derive(Clone, Copy, Debug)]
enum ByteRange {
    From { start: u64, end: Option<u64> },
    Last(u64),
}

struct Request {
    method: String,
    target: String,
    range: Option<ByteRange>,
}

fn handle(mut stream: TcpStream, cfg: &Cfg, counters: &Counters, project: &std::sync::Mutex<String>) -> std::io::Result<()> {
    use std::sync::atomic::Ordering;
    stream.set_nodelay(true).ok();
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        head.extend_from_slice(&line);
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        if head.len() > 64 * 1024 {
            return Ok(());
        }
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let mut lines = text.lines();
    let Some(first) = lines.next() else { return Ok(()) };
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut length = 0usize;
    let mut range = None;
    for h in lines {
        let Some((k, v)) = h.split_once(':') else { continue };
        match k.trim().to_ascii_lowercase().as_str() {
            "range" => range = parse_range(v.trim()),
            "content-length" => length = v.trim().parse().unwrap_or(0),
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
    let req = Request { method, target, range };
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
            // the view's iframe address carries.
            let mut embed = String::from("/web/?embed");
            if req.target.contains("backend=webgl2") {
                embed.push_str("&webgl");
            }
            // `?project=1` on the host page makes the view fetch the project itself
            // (`?project=<url>` is the fork's own flag), instead of the page posting it.
            if req.target.contains("project=1") {
                embed.push_str("&project=/project.json");
            }
            // `?spike` turns on the view's own per-frame samples (the measurement host reads
            // them); the stock page never asks for them.
            if req.target.contains("spike") {
                embed.push_str("&spike");
            }
            if embed.len() > "/web/?embed".len() {
                injected = injected.replace("/web/?embed", &embed);
            }
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
            match hex {
                Some(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                None => {}
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}
