//! Where the newest files of each source are. CKAN's `package_show` lists a dataset's resources with their address, size and
//! `last_modified` (the date of the data: `metadata_modified` only says the page was edited), which is what a refresh compares.
//! The ASGS boundary files have no CKAN record: their addresses are pinned in `Net`, and their date is the server's
//! `Last-Modified`. The ABS observations are fetched, so they have no date to compare: only the day they were fetched.

use crate::dates::{http_date, today};
use crate::error::{GovError, Res};
use crate::net::Net;
use serde_json::Value;
use std::time::Duration;

/// Where each CKAN source lives (the dataset ids of the ticket; G-NAF is addressed by its name).
const DATASETS: [(&str, &str); 4] = [
    ("abr", "5bd7fcab-e315-42cb-8daf-50b7efc2027e"),
    ("asic", "7b8656f9-606d-4337-af29-66b89b2eeefb"),
    ("asicBn", "bc515135-4bb6-4d50-957a-3713709a76d3"),
    ("gnaf", "geocoded-national-address-file-g-naf"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// The file's own name, which is also its name on disk.
    pub name: String,
    pub url: String,
    /// The size the record states (0 when it states none).
    pub size: u64,
    /// The date of the data, as the record writes it (`2026-11-09T13:53:06.104534`).
    pub last_modified: String,
    pub md5: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Latest {
    pub source: &'static str,
    /// `yyyy-mm-dd`: the newest `last_modified` of the files, or the day the data was fetched.
    pub as_of: String,
    pub files: Vec<Remote>,
    /// Fetched from an API on request, with no date to compare.
    pub fetched: bool,
}

struct Resource {
    name: String,
    format: String,
    remote: Remote,
}

/// A file name is letters, digits, dots, dashes and underscores: it is joined to a folder, so nothing else is let through.
pub(crate) fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn plain_name(url: &str) -> Option<String> {
    let last = url.split(['?', '#']).next()?.rsplit('/').next()?;
    safe_name(last).then(|| last.to_string())
}

fn valid_day(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 10 && b.iter().take(10).enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

fn resource_of(r: &Value) -> Option<Resource> {
    let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default();
    let url = text("url");
    let last_modified = [text("last_modified"), text("created")].into_iter().find(|d| valid_day(d))?.to_string();
    let size = r.get("size").and_then(|s| s.as_u64().or_else(|| s.as_str().and_then(|t| t.parse().ok()))).unwrap_or(0);
    let md5 = Some(text("hash").to_string()).filter(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    Some(Resource {
        name: text("name").to_string(),
        format: text("format").to_ascii_uppercase(),
        remote: Remote { name: plain_name(url)?, url: url.to_string(), size, last_modified, md5 },
    })
}

/// The files of `source` among a dataset's resources. The ABN extract is every `public_split` ZIP (it comes in parts); the others
/// are the newest ZIP of one name.
fn pick(source: &str, resources: Vec<Resource>) -> Vec<Remote> {
    let zips = resources.into_iter().filter(|r| r.format == "ZIP");
    if source == "abr" {
        let mut parts: Vec<Remote> =
            zips.filter(|r| r.remote.name.to_ascii_lowercase().starts_with("public_split")).map(|r| r.remote).collect();
        parts.sort_by(|a, b| a.name.cmp(&b.name));
        return parts;
    }
    let want = |r: &Resource| match source {
        "asic" => r.name.starts_with("Company Dataset"),
        "asicBn" => r.name.starts_with("Business Names Dataset"),
        _ => r.name.contains("GDA2020"),
    };
    let newest = zips.filter(want).max_by(|a, b| (&a.remote.last_modified, &a.remote.name).cmp(&(&b.remote.last_modified, &b.remote.name)));
    newest.map(|r| vec![r.remote]).unwrap_or_default()
}

fn package(net: &Net, source: &'static str) -> Res<Latest> {
    let Some((_, id)) = DATASETS.iter().find(|(s, _)| *s == source) else {
        return Err(GovError::bad_source(format!("{source} has no CKAN dataset")));
    };
    let body = net.bytes(&format!("{}{id}", net.ckan), &[("Accept", "application/json".to_string())], Duration::from_secs(30), 16 << 20)?;
    let json: Value = serde_json::from_slice(&body).map_err(|e| GovError::refresh(format!("CKAN did not answer with JSON ({e})")))?;
    if json.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(GovError::refresh(format!("CKAN has no dataset for {source}")));
    }
    let resources =
        json.pointer("/result/resources").and_then(Value::as_array).map(|a| a.iter().filter_map(resource_of).collect()).unwrap_or_default();
    finish(net, source, pick(source, resources))
}

/// The ASGS files: a HEAD of each pinned address says its date and, usually, its size.
fn pinned(net: &Net, source: &'static str) -> Res<Latest> {
    let mut files = Vec::new();
    for url in &net.asgs {
        let name = plain_name(url).ok_or_else(|| GovError::bad_source("a pinned ASGS address has no file name"))?;
        let resp = net.send(url, true, &[], Duration::from_secs(30))?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(GovError::refresh(format!("{name}: the ABS answered HTTP {status}")));
        }
        let header = |k: &str| resp.headers().get(k).and_then(|v| v.to_str().ok());
        let last_modified = header("last-modified")
            .and_then(http_date)
            .ok_or_else(|| GovError::refresh(format!("{name}: the ABS gave no Last-Modified date")))?;
        let size = header("content-length").and_then(|s| s.parse().ok()).unwrap_or(0);
        files.push(Remote { name, url: url.clone(), size, last_modified, md5: None });
    }
    finish(net, source, files)
}

fn finish(net: &Net, source: &'static str, files: Vec<Remote>) -> Res<Latest> {
    if files.is_empty() {
        return Err(GovError::refresh(format!("the record lists no {source} file this build knows how to read")));
    }
    if let Some(f) = files.iter().find(|f| !net.allowed(&f.url)) {
        return Err(GovError::refresh(format!(
            "{} is named on {}, which is not an official data host",
            f.name,
            crate::net::host_of(&f.url)
        )));
    }
    let as_of = files.iter().map(|f| f.last_modified.get(..10).unwrap_or_default()).max().unwrap_or_default().to_string();
    Ok(Latest { source, as_of, files, fetched: false })
}

pub fn latest(net: &Net, source: &'static str) -> Res<Latest> {
    match source {
        "abs" => Ok(Latest { source, as_of: today(), files: Vec::new(), fetched: true }),
        "asgs" => pinned(net, source),
        _ => package(net, source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn res(name: &str, format: &str, url: &str, size: u64, lm: &str) -> Value {
        json!({"name": name, "format": format, "url": url, "size": size, "last_modified": lm, "hash": ""})
    }

    fn list(source: &str, items: Vec<Value>) -> Vec<Remote> {
        pick(source, items.iter().filter_map(resource_of).collect())
    }

    #[test]
    fn picks_the_files_of_each_source() {
        let abn = list(
            "abr",
            vec![
                res("Readme", "PDF", "https://data.gov.au/x/readme.pdf", 1, "2025-07-02T01:59:12"),
                res("ABN Bulk Extract Part 2 ", "ZIP", "https://data.gov.au/x/public_split_11_20.zip", 5, "2026-11-10T21:36:11.055240"),
                res("ABN Bulk Extract Part 1", "ZIP", "https://data.gov.au/x/public_split_1_10.zip", 4, "2026-11-10T21:34:30.891697"),
            ],
        );
        assert_eq!(abn.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["public_split_11_20.zip", "public_split_1_10.zip"]);
        let asic = list(
            "asic",
            vec![
                res("Company Dataset - Current", "CSV", "https://data.gov.au/x/company_202611.csv", 400, "2026-11-09T13:52:53"),
                res("Company Dataset - Current", "ZIP", "https://data.gov.au/x/company_202611.zip", 79, "2026-11-09T13:53:06"),
                res("Company Dataset - Previous", "ZIP", "https://data.gov.au/x/company_202610.zip", 78, "2026-10-09T13:53:06"),
            ],
        );
        assert_eq!((asic.len(), asic[0].name.as_str(), asic[0].size), (1, "company_202611.zip", 79));
        let gnaf = list(
            "gnaf",
            vec![
                res("AUG 2026 - Geoscape G-NAF - GDA94", "ZIP", "https://data.gov.au/x/g-naf_aug26_gda94.zip", 1, "2026-08-17T03:20:13"),
                res(
                    "AUG 2026 - Geoscape G-NAF - GDA2020",
                    "ZIP",
                    "https://data.gov.au/x/g-naf_aug26_gda2020.zip",
                    2,
                    "2026-08-17T03:24:56",
                ),
                res(
                    "MAY 2026 - Geoscape G-NAF - GDA2020",
                    "ZIP",
                    "https://data.gov.au/x/g-naf_may26_gda2020.zip",
                    2,
                    "2026-05-18T03:24:56",
                ),
            ],
        );
        assert_eq!(gnaf.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["g-naf_aug26_gda2020.zip"]);
        assert!(
            list("asicBn", vec![res("Business Names Dataset - Help", "PDF", "https://data.gov.au/x/h.pdf", 1, "2026-01-01")]).is_empty()
        );
    }

    #[test]
    fn dates_sizes_hashes_and_names() {
        let r = resource_of(&json!({"name": "n", "format": "zip", "url": "https://data.gov.au/a/b.zip?x=1", "size": "42", "last_modified": null, "created": "2026-01-02T03:04:05", "hash": "4F5638FF363F438F95097599C8C33B82"})).unwrap();
        assert_eq!(
            (r.remote.name.as_str(), r.remote.size, r.remote.last_modified.as_str(), r.format.as_str()),
            ("b.zip", 42, "2026-01-02T03:04:05", "ZIP")
        );
        assert!(r.remote.md5.is_some());
        assert!(resource_of(&json!({"url": "https://data.gov.au/a/b.zip", "last_modified": "yesterday"})).is_none());
        for bad in [
            "https://data.gov.au/a/..",
            "https://data.gov.au/a/",
            "https://data.gov.au/a/.hidden",
            "https://data.gov.au/a/a b.zip",
            "https://data.gov.au/a/%2e%2e",
        ] {
            assert!(plain_name(bad).is_none(), "{bad}");
        }
    }
}
