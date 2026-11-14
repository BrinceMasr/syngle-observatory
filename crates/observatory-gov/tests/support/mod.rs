//! A small HTTP server for the network tests: it answers from a handler, supports `Range`, can cut a connection short, and logs
//! every request. It stands in for CKAN, the file hosts, the ABS gateway and ABN Lookup (nothing here touches the internet).

#![allow(dead_code)]

use observatory_gov::{Env, Net, Online, run_with};
use serde_json::Value;
use std::io::{BufRead, BufReader, Cursor, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct Req {
    pub method: String,
    pub target: String,
    pub range: Option<u64>,
    /// `http://127.0.0.1:<port>`: where this server is, for the addresses a record names.
    pub base: String,
}

impl Req {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }
}

pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Write only this many bytes of the body and drop the connection (the length it states is the whole body's).
    pub cut: Option<usize>,
}

impl Resp {
    pub fn status(status: u16) -> Resp {
        Resp { status, headers: Vec::new(), body: Vec::new(), cut: None }
    }

    pub fn body(body: Vec<u8>) -> Resp {
        Resp { status: 200, headers: Vec::new(), body, cut: None }
    }

    pub fn json(v: &Value) -> Resp {
        Resp::body(v.to_string().into_bytes())
    }

    pub fn header(mut self, k: &str, v: &str) -> Resp {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }
}

/// A file as a server with ranges serves it.
pub fn file(req: &Req, bytes: &[u8]) -> Resp {
    match req.range {
        Some(from) if from as usize >= bytes.len() => Resp::status(416),
        Some(from) => {
            let rest = bytes.get(from as usize..).unwrap_or_default().to_vec();
            let range = format!("bytes {from}-{}/{}", bytes.len() - 1, bytes.len());
            Resp { status: 206, headers: vec![("Content-Range".into(), range)], body: rest, cut: None }
        }
        None => Resp::body(bytes.to_vec()),
    }
}

pub struct Server {
    pub base: String,
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    pub fn start(handler: impl Fn(&Req) -> Resp + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let log = Arc::new(Mutex::new(Vec::new()));
        let (handler, seen, at) = (Arc::new(handler), log.clone(), base.clone());
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (h, l, b) = (handler.clone(), seen.clone(), at.clone());
                thread::spawn(move || serve(stream, &*h, &l, &b));
            }
        });
        Server { base, log }
    }

    /// How many requests so far had a line holding `needle` (`GET /files/a.zip`, `range=160`).
    pub fn hits(&self, needle: &str) -> usize {
        self.log.lock().unwrap().iter().filter(|l| l.contains(needle)).count()
    }

    pub fn lines(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    /// A `Net` that fetches from this server only.
    pub fn net(&self) -> Net {
        Net {
            allow: vec!["127.0.0.1".into()],
            https_only: false,
            ckan: format!("{}/ckan?id=", self.base),
            abs_api: format!("{}/abs", self.base),
            abn_json: format!("{}/json/", self.base),
            asgs: Vec::new(),
        }
    }
}

fn serve(mut stream: TcpStream, handler: &dyn Fn(&Req) -> Resp, log: &Mutex<Vec<String>>, base: &str) {
    let Ok(clone) = stream.try_clone() else { return };
    let mut reader = BufReader::new(clone);
    let mut first = String::new();
    if reader.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string());
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
            range = v.trim().trim_end_matches('-').parse().ok();
        }
    }
    let req = Req { method: method.clone(), target: target.clone(), range, base: base.to_string() };
    log.lock().unwrap().push(format!("{method} {target}{}", range.map(|r| format!(" range={r}")).unwrap_or_default()));
    let resp = handler(&req);
    let mut head = format!("HTTP/1.1 {} X\r\n", resp.status);
    for (k, v) in &resp.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", resp.body.len()));
    let _ = stream.write_all(head.as_bytes());
    if method != "HEAD" {
        let n = resp.cut.map_or(resp.body.len(), |c| c.min(resp.body.len()));
        let _ = stream.write_all(resp.body.get(..n).unwrap_or_default());
    }
    let _ = stream.flush();
}

// ---- helpers the tests share ------------------------------------------------------------------------------------------

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/gov-data/fixtures/v1")
}

pub fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obs-gov-net-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn zipped(entry: &str, bytes: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(entry, zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(bytes).unwrap();
    zip.finish().unwrap().into_inner()
}

/// One CKAN `package_show` answer.
pub fn package(resources: &[(&str, &str, String, u64, &str)]) -> Value {
    let list: Vec<Value> = resources
        .iter()
        .map(|(name, format, url, size, modified)| serde_json::json!({"name": name, "format": format, "url": url, "size": size, "last_modified": modified, "hash": ""}))
        .collect();
    serde_json::json!({"success": true, "result": {"resources": list}})
}

/// Runs `gov <args>` against `data` with `net`; the answer parsed, as text, and the exit code.
pub fn go(data: &Path, net: &Net, guid: Option<&str>, args: &[&str]) -> (Value, String, u8) {
    let env = Env { gov_dir: Some(data.to_path_buf()), local_data: None, sort_run_bytes: 1 << 20 };
    let online = Online { abn_guid: guid.map(String::from), net: Some(net.clone()) };
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let out = run_with(&args, &env, &online);
    (serde_json::from_str(&out.stdout).unwrap_or(Value::Null), out.stdout, out.exit)
}
