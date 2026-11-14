//! The ABS observations: the dataflow catalogue and, for each flow in the configured list, its SDMX-CSV, asked for with
//! `Accept: application/vnd.sdmx.data+csv`. The gateway answers at most 10 MB in 30 seconds and a key of 5,000 characters: a
//! flow it refuses whole is fetched again in slices of five years, whose rows are joined under one header.

use crate::dates::this_year;
use crate::error::{GovError, Res};
use crate::net::{Net, read_limited};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::Duration;

const CSV: &str = "application/vnd.sdmx.data+csv";
const STRUCTURE: &str = "application/vnd.sdmx.structure+json";
/// The flows fetched when none were named: the ones people ask about first.
pub const DEFAULT_FLOWS: [&str; 5] = ["CPI", "LF", "WPI", "ERP_Q", "RT"];
const MAX_CSV: u64 = 256 << 20;
const SLICE_YEARS: i64 = 5;
const FIRST_YEAR: i64 = 1950;
const WAIT: Duration = Duration::from_secs(120);

pub fn valid_flow(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// One GET of a data query: `None` when the gateway has no observations for it (404 `NoRecordsFound`), `Err(None)` when it
/// refused the size of it, so the caller slices.
fn data(net: &Net, url: &str) -> Result<Option<Vec<u8>>, Option<GovError>> {
    let mut resp = net.send(url, false, &[("Accept", CSV.to_string())], WAIT).map_err(Some)?;
    match resp.status().as_u16() {
        200 => read_limited(&mut resp, MAX_CSV).map(Some).map_err(|_| None),
        404 => Ok(None),
        413 | 414 | 500 | 502 | 503 | 504 => Err(None),
        s => Err(Some(GovError::refresh(format!("{} answered HTTP {s}", crate::net::host_of(url))))),
    }
}

fn flow(net: &Net, key: &str) -> Res<Option<Vec<u8>>> {
    let base = format!("{}/data/{key}/all", net.abs_api);
    match data(net, &base) {
        Ok(rows) => return Ok(rows),
        Err(Some(e)) => return Err(e),
        Err(None) => {}
    }
    let mut joined: Vec<u8> = Vec::new();
    let mut start = FIRST_YEAR;
    while start <= this_year() {
        match data(net, &format!("{base}?startPeriod={start}&endPeriod={}", start + SLICE_YEARS - 1)) {
            Ok(Some(rows)) if joined.is_empty() => joined = rows,
            Ok(Some(rows)) => {
                let body = rows.iter().position(|b| *b == b'\n').map_or(0, |i| i + 1);
                joined.extend_from_slice(rows.get(body..).unwrap_or_default());
            }
            Ok(None) => {}
            Err(Some(e)) => return Err(e),
            Err(None) => return Err(GovError::refresh(format!("{key}: the gateway refused even a slice of five years"))),
        }
        if joined.len() as u64 > MAX_CSV {
            return Err(GovError::refresh(format!("{key} is larger than {MAX_CSV} bytes")));
        }
        start += SLICE_YEARS;
    }
    Ok((!joined.is_empty()).then_some(joined))
}

/// Fills `dir` (made beside it and renamed whole): `dataflows.json` and `data/<ID>.csv` for each flow that has observations.
pub fn fetch(net: &Net, dir: &Path, flows: &[String]) -> Res<()> {
    let tmp = dir.with_extension("tmp");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("data")).map_err(GovError::refresh)?;
    let catalogue = net.bytes(&format!("{}/dataflow/all", net.abs_api), &[("Accept", STRUCTURE.to_string())], WAIT, 64 << 20)?;
    let json: Value =
        serde_json::from_slice(&catalogue).map_err(|e| GovError::refresh(format!("the ABS dataflow list is not JSON ({e})")))?;
    let listed = json.pointer("/data/dataflows").and_then(Value::as_array).cloned().unwrap_or_default();
    fs::write(tmp.join("dataflows.json"), &catalogue).map_err(GovError::refresh)?;
    for id in flows {
        let text = |f: &Value, k: &str| f.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let Some(f) = listed.iter().find(|f| f.get("id").and_then(Value::as_str) == Some(id)) else {
            let _ = fs::remove_dir_all(&tmp);
            return Err(GovError::bad_source(format!("the ABS has no dataflow {}", crate::json::echo(id))));
        };
        let key = format!("{},{id},{}", text(f, "agencyID"), text(f, "version"));
        if let Some(rows) = flow(net, &key).inspect_err(|_| {
            let _ = fs::remove_dir_all(&tmp);
        })? {
            fs::write(tmp.join("data").join(format!("{id}.csv")), rows).map_err(GovError::refresh)?;
        }
    }
    let _ = fs::remove_dir_all(dir);
    fs::rename(&tmp, dir).map_err(GovError::refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_ids_are_plain() {
        assert!(valid_flow("ERP_Q") && valid_flow("CPI") && valid_flow("A-b.1"));
        assert!(!valid_flow("") && !valid_flow("a/b") && !valid_flow("a b") && !valid_flow("../x") && !valid_flow(&"a".repeat(65)));
        assert!(DEFAULT_FLOWS.iter().all(|f| valid_flow(f)));
    }
}
