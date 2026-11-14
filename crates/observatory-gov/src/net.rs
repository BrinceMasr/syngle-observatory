//! The network: the only module that opens a connection. A request, and every hop of a redirect, goes to a host on the list
//! (data.gov.au, abs.gov.au and its data gateway, abr.business.gov.au) over https; any other host is refused, whoever named
//! it (a CKAN record included). Nothing here prints a URL in an error, because the ABN Lookup address carries a secret.

use crate::error::{GovError, Res};
use std::io::Read;
use std::time::Duration;
use ureq::http::Response;

const AGENT: &str = concat!("syngle-map-gov/", env!("CARGO_PKG_VERSION"));
const MAX_REDIRECTS: usize = 5;

/// The official hosts and addresses; tests build one that points at a local server.
#[derive(Clone)]
pub struct Net {
    /// A host is allowed when it is one of these or a subdomain of one.
    pub allow: Vec<String>,
    pub https_only: bool,
    /// CKAN's `package_show`, up to the dataset id.
    pub ckan: String,
    /// The ABS data gateway (SDMX REST), without a trailing slash.
    pub abs_api: String,
    /// The ABN Lookup JSON services, with a trailing slash.
    pub abn_json: String,
    /// The ASGS boundary files, pinned (ABS publishes them on a page, not in CKAN).
    pub asgs: Vec<String>,
}

impl Net {
    pub fn official() -> Net {
        let asgs = "https://www.abs.gov.au/statistics/standards/australian-statistical-geography-standard-asgs/edition-3-july-2021-june-2026/access-and-downloads/digital-boundary-files/";
        Net {
            allow: ["data.gov.au", "abs.gov.au", "abr.business.gov.au"].map(String::from).to_vec(),
            https_only: true,
            ckan: "https://data.gov.au/data/api/3/action/package_show?id=".into(),
            abs_api: "https://data.api.abs.gov.au/rest".into(),
            abn_json: "https://abr.business.gov.au/json/".into(),
            asgs: vec![format!("{asgs}ASGS_2021_MAIN_STRUCTURE_GPKG_GDA2020.zip")],
        }
    }

    /// Whether `url` may be fetched: the scheme, and the host against the list. A name with a user (`a@b`) is never allowed.
    pub fn allowed(&self, url: &str) -> bool {
        let Some((scheme, rest)) = url.split_once("://") else { return false };
        if scheme != "https" && !(scheme == "http" && !self.https_only) {
            return false;
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.contains('@') || authority.contains('[') {
            return false;
        }
        let host = match authority.rsplit_once(':') {
            Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
            _ => authority,
        };
        let host = host.to_ascii_lowercase();
        !host.is_empty() && self.allow.iter().any(|a| host == *a || host.ends_with(&format!(".{a}")))
    }

    /// One GET (or HEAD) with a redirect loop that checks each hop. An error status is answered, not failed: the caller reads it.
    pub fn send(&self, url: &str, head: bool, headers: &[(&str, String)], timeout: Duration) -> Res<Response<ureq::Body>> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(AGENT)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(timeout))
            .build()
            .into();
        let mut at = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            if !self.allowed(&at) {
                return Err(GovError::refresh(format!("{} is not an official data host", host_of(&at))));
            }
            let mut req = if head { agent.head(&at) } else { agent.get(&at) };
            for (k, v) in headers {
                req = req.header(*k, v.as_str());
            }
            let resp = req.call().map_err(|e| GovError::refresh(format!("could not reach {}: {}", host_of(&at), failure(&e))))?;
            if !resp.status().is_redirection() {
                return Ok(resp);
            }
            let location = resp.headers().get("location").and_then(|v| v.to_str().ok());
            let Some(location) = location else { return Err(GovError::refresh("a redirect with no Location")) };
            at = join(&at, location);
        }
        Err(GovError::refresh("too many redirects"))
    }

    /// A GET that must answer 200, read whole (at most `limit` bytes).
    pub fn bytes(&self, url: &str, headers: &[(&str, String)], timeout: Duration, limit: u64) -> Res<Vec<u8>> {
        let mut resp = self.send(url, false, headers, timeout)?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(GovError::refresh(format!("{} answered HTTP {status}", host_of(url))));
        }
        read_limited(&mut resp, limit).map_err(|why| GovError::refresh(format!("{}: {why}", host_of(url))))
    }
}

/// The body, whole, refusing more than `limit` bytes.
pub fn read_limited(resp: &mut Response<ureq::Body>, limit: u64) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let reader = resp.body_mut().with_config().limit(limit.saturating_add(1)).reader();
    reader.take(limit.saturating_add(1)).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > limit {
        return Err(format!("the answer is larger than {limit} bytes"));
    }
    Ok(buf)
}

/// Only the host of an address, for a message (an address can carry a secret in its query).
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    host.rsplit('@').next().unwrap_or_default().chars().take(80).collect()
}

fn failure(e: &ureq::Error) -> String {
    use ureq::Error::*;
    match e {
        Timeout(_) => "timed out".into(),
        HostNotFound => "the host name was not found".into(),
        ConnectionFailed => "the connection failed".into(),
        Io(_) => "the connection broke".into(),
        BodyExceedsLimit(_) => "the answer is too large".into(),
        _ => "the request failed".into(),
    }
}

/// A `Location` against the address it answered.
fn join(base: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if location.starts_with('/') {
        return format!("{scheme}://{authority}{location}");
    }
    let path = rest.get(authority.len()..).unwrap_or_default().split(['?', '#']).next().unwrap_or_default();
    let dir = path.rfind('/').and_then(|i| path.get(..=i)).unwrap_or("/");
    format!("{scheme}://{authority}{dir}{location}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_official_hosts_over_https() {
        let net = Net::official();
        for ok in [
            "https://data.gov.au/data/api/3/action/package_show?id=x",
            "https://www.abs.gov.au/a/b.zip",
            "https://data.api.abs.gov.au/rest/dataflow/all",
            "https://abr.business.gov.au/json/AbnDetails.aspx",
            "https://DATA.GOV.AU:443/x",
        ] {
            assert!(net.allowed(ok), "{ok}");
        }
        for no in [
            "http://data.gov.au/x",
            "https://data.gov.au.evil.com/x",
            "https://evildata.gov.au/x",
            "https://data.gov.au@evil.com/x",
            "https://evil.com/data.gov.au",
            "https://evil.com/?data.gov.au",
            "https://[::1]/x",
            "ftp://data.gov.au/x",
            "data.gov.au/x",
            "https:///x",
        ] {
            assert!(!net.allowed(no), "{no}");
        }
        let local = Net { allow: vec!["127.0.0.1".into()], https_only: false, ..Net::official() };
        assert!(local.allowed("http://127.0.0.1:8080/x") && !local.allowed("http://localhost:8080/x"));
    }

    #[test]
    fn pinned_addresses_are_official() {
        let net = Net::official();
        assert!(net.asgs.iter().all(|u| net.allowed(u) && u.ends_with(".zip")));
        assert!(net.allowed(&net.ckan) && net.allowed(&net.abs_api) && net.allowed(&net.abn_json));
    }

    #[test]
    fn redirects_join() {
        assert_eq!(join("https://a.gov.au/x/y.zip", "https://b.gov.au/z"), "https://b.gov.au/z");
        assert_eq!(join("https://a.gov.au/x/y.zip?q=1", "/z/w"), "https://a.gov.au/z/w");
        assert_eq!(join("https://a.gov.au/x/y.zip", "w.zip"), "https://a.gov.au/x/w.zip");
        assert_eq!(join("https://a.gov.au", "w"), "https://a.gov.au/w");
    }

    #[test]
    fn messages_hold_a_host_only() {
        assert_eq!(host_of("https://abr.business.gov.au/json/AbnDetails.aspx?guid=SECRET"), "abr.business.gov.au");
        assert_eq!(host_of("https://user:pw@h.gov.au/x"), "h.gov.au");
    }
}
