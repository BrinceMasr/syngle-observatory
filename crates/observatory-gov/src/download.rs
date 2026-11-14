//! A file downloaded to disk: resumable, checked, and never written in place. The bytes arrive in `<name>.<tag>.part`, where the
//! tag names the remote file (its address, size and date), so a part left by a killed run is continued with a `Range` request
//! and a part of another release is dropped. Only a complete, verified file is renamed to its own name, so a name that exists
//! is a file that is whole.

use crate::error::{GovError, Res};
use crate::net::Net;
use md5::{Digest, Md5};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const ATTEMPTS: usize = 4;
/// A download is given time for this many bytes a second, and no less than a minute.
const SLOWEST: u64 = 128 << 10;
pub const MAX_FILE: u64 = 8 << 30;

pub struct Want<'a> {
    pub url: &'a str,
    /// A plain file name: letters, digits, dots, dashes and underscores (the caller checks).
    pub name: &'a str,
    /// The size the remote record states, or 0 when it does not say.
    pub size: u64,
    /// The hash the remote record states (MD5, 32 hex digits), when it gives one.
    pub md5: Option<&'a str>,
    pub tag: &'a str,
}

fn content_range_start(resp: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    let text = resp.headers().get("content-range")?.to_str().ok()?;
    text.strip_prefix("bytes ")?.split('-').next()?.parse().ok()
}

fn content_length(resp: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    resp.headers().get("content-length")?.to_str().ok()?.parse().ok()
}

/// One try: continue the part where it stops. `Ok` when the stream ended with every byte; the part stays on disk for the next try.
fn attempt(net: &Net, want: &Want, part: &Path, size: &mut u64) -> Result<(), String> {
    let have = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if *size > 0 && have > *size {
        let _ = fs::remove_file(part);
        return Err("the partial file was longer than the file".into());
    }
    if *size > 0 && have == *size {
        return Ok(());
    }
    let mut headers = vec![("Accept-Encoding", "identity".to_string())];
    if have > 0 {
        headers.push(("Range", format!("bytes={have}-")));
    }
    let left = if *size > have { *size - have } else { MAX_FILE };
    let timeout = Duration::from_secs(60 + left.min(MAX_FILE) / SLOWEST);
    let mut resp = net.send(want.url, false, &headers, timeout).map_err(|e| e.message)?;
    let from = match resp.status().as_u16() {
        206 if content_range_start(&resp) == Some(have) => have,
        206 => {
            let _ = fs::remove_file(part);
            return Err("the server continued from another place".into());
        }
        200 => 0,
        416 => {
            let _ = fs::remove_file(part);
            return Err("the server did not accept the range, so the file starts again".into());
        }
        status => return Err(format!("HTTP {status}")),
    };
    let total = content_length(&resp).map(|l| l + from);
    if *size == 0 {
        *size = total.unwrap_or(0);
    }
    if let Some(t) = total.filter(|t| *size > 0 && t != size) {
        return Err(format!("the server says the file is {t} bytes, not {size}"));
    }
    if *size > MAX_FILE {
        return Err("the file is larger than a data file can be".into());
    }
    let mut file = if from == 0 { File::create(part) } else { OpenOptions::new().append(true).open(part) }.map_err(|e| e.to_string())?;
    let mut reader = resp.body_mut().with_config().limit(MAX_FILE).reader();
    let mut buf = vec![0u8; 256 << 10];
    let mut got = from;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("the connection broke after {got} bytes ({e})"))?;
        if n == 0 {
            break;
        }
        file.write_all(buf.get(..n).unwrap_or_default()).map_err(|e| e.to_string())?;
        got += n as u64;
    }
    file.flush().map_err(|e| e.to_string())?;
    if *size > 0 && got != *size {
        return Err(format!("the download stopped at {got} of {size} bytes"));
    }
    Ok(())
}

/// Reading every entry to its end checks the CRC-32 the archive holds for it: a damaged archive is refused here, not half way
/// into an index build.
pub fn verify_zip(path: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?).map_err(|e| format!("not a ZIP archive ({e})"))?;
    if zip.is_empty() {
        return Err("the archive holds no files".into());
    }
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| format!("a damaged entry ({e})"))?;
        io::copy(&mut entry, &mut io::sink()).map_err(|e| format!("a damaged entry {} ({e})", entry.name()))?;
    }
    Ok(())
}

fn md5_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let (mut h, mut buf) = (Md5::new(), vec![0u8; 1 << 20]);
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(buf.get(..n).unwrap_or_default());
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn verify(part: &Path, want: &Want, size: u64) -> Result<(), String> {
    let len = fs::metadata(part).map_err(|e| e.to_string())?.len();
    if size > 0 && len != size {
        return Err(format!("{len} bytes, not the {size} the record states"));
    }
    if let Some(expected) = want.md5.filter(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit())) {
        let got = md5_hex(part).map_err(|e| e.to_string())?;
        if !got.eq_ignore_ascii_case(expected) {
            return Err("the hash is not the one the record states".into());
        }
    }
    if want.name.to_ascii_lowercase().ends_with(".zip") {
        verify_zip(part)?;
    }
    Ok(())
}

/// Brings `dir/name` into being, whole and checked, and returns its path (and, as the second value, its size). A file that is
/// already there (of the stated size, when one is stated) is the file.
pub fn fetch(net: &Net, want: &Want, dir: &Path) -> Res<(PathBuf, u64)> {
    fs::create_dir_all(dir).map_err(GovError::refresh)?;
    let done = dir.join(want.name);
    // A name that exists is a file that was renamed after its checks, so it is whole; the size, when the record states one, must agree.
    if let Ok(meta) = fs::metadata(&done)
        && meta.is_file()
        && (want.size == 0 || meta.len() == want.size)
    {
        return Ok((done, meta.len()));
    }
    let part = dir.join(format!("{}.{}.part", want.name, want.tag));
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(&format!("{}.", want.name)) && n.ends_with(".part") && e.path() != part {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let (mut size, mut failure) = (want.size, String::new());
    let mut ok = false;
    for _ in 0..ATTEMPTS {
        match attempt(net, want, &part, &mut size) {
            Ok(()) => {
                ok = true;
                break;
            }
            Err(e) => failure = e,
        }
    }
    if !ok {
        return Err(GovError::refresh(format!("{}: {failure}", want.name)));
    }
    if let Err(why) = verify(&part, want, size) {
        let _ = fs::remove_file(&part);
        return Err(GovError::refresh(format!("{} failed its check: {why}", want.name)));
    }
    let len = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    fs::rename(&part, &done).map_err(GovError::refresh)?;
    Ok((done, len))
}
