//! `gov refresh` from the official hosts, against a local server that stands in for CKAN, the file hosts and the ABS gateway:
//! what `--check` says and exits, a refresh that downloads only what changed, a download that breaks and resumes by range, and
//! every way a refresh is refused (a foreign host, a redirect off the list, too little disk, a damaged ZIP) leaving the index
//! as it was.

mod support;

use serde_json::Value;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use support::{Req, Resp, Server, file, fixtures, go, package, temp, zipped};

const ASIC: &str = "7b8656f9-606d-4337-af29-66b89b2eeefb";
const ASIC_BN: &str = "bc515135-4bb6-4d50-957a-3713709a76d3";
const MODIFIED: &str = "2026-11-09T13:53:06.104534";

fn asic_zip() -> Vec<u8> {
    zipped("company_202611.csv", &fs::read(fixtures().join("asic/company_202611.csv")).unwrap())
}

fn bn_zip() -> Vec<u8> {
    zipped("business_names_202611.csv", &fs::read(fixtures().join("asic/business_names_202611.csv")).unwrap())
}

/// A server with both ASIC datasets; `modified` is the date CKAN gives the company file.
fn ckan(modified: Arc<Mutex<String>>) -> Server {
    let (company, names) = (asic_zip(), bn_zip());
    Server::start(move |req: &Req| match req.path() {
        "/ckan" if req.target.ends_with(ASIC) => {
            let at = modified.lock().unwrap().clone();
            Resp::json(&package(&[
                ("Company Dataset - Help File", "PDF", format!("{}/files/help.pdf", req.base), 5, "2025-07-11T05:11:45"),
                ("Company Dataset - Current", "ZIP", format!("{}/files/company_202611.zip", req.base), company.len() as u64, &at),
            ]))
        }
        "/ckan" if req.target.ends_with(ASIC_BN) => Resp::json(&package(&[(
            "Business Names Dataset - Current",
            "ZIP",
            format!("{}/files/business_names_202611.zip", req.base),
            names.len() as u64,
            "2026-11-10T19:35:59.333216",
        )])),
        "/files/company_202611.zip" => file(req, &company),
        "/files/business_names_202611.zip" => file(req, &names),
        _ => Resp::status(404),
    })
}

fn code(v: &Value) -> Option<&str> {
    v["error"]["code"].as_str()
}

#[test]
fn check_then_refresh_then_only_what_changed() {
    let (modified, data) = (Arc::new(Mutex::new(MODIFIED.to_string())), temp("cycle"));
    let server = ckan(modified.clone());
    let net = server.net();

    // Nothing is turned on, so a plain check lists what is available and finds no update; naming a source asks for it.
    let (all, _, exit) = go(&data, &net, None, &["refresh", "--check", "--source", "asic,asicBn"]);
    assert_eq!(
        (exit, all["upToDate"].as_bool(), all["updates"].to_string()),
        (10, Some(false), "[\"asic\",\"asicBn\"]".to_string()),
        "{all}"
    );
    assert_eq!(all["sources"][0]["latest"]["lastModified"], MODIFIED);
    assert_eq!(all["sources"][0]["status"], "not-installed");
    assert_eq!(server.hits("/files/"), 0, "a check downloads nothing");
    assert_eq!(go(&data, &net, None, &["refresh"]).0["error"]["code"], "no-data");

    // Turn both on: one snapshot, from files that arrived whole.
    let (done, _, exit) = go(&data, &net, None, &["refresh", "--source", "asic,asicBn"]);
    assert_eq!(
        (exit, done["built"].as_bool(), done["downloaded"].to_string()),
        (0, Some(true), "[\"asic\",\"asicBn\"]".to_string()),
        "{done}"
    );
    assert_eq!(done["sources"][0]["asOf"], "2026-11-09");
    let (company, text, _) = go(&data, &net, None, &["company", "100200306"]);
    assert_eq!(company["results"][0]["name"], "HARBOUR ROOFING PTY LTD");
    assert!(text.contains(done["dataVersion"].as_str().unwrap()) && text.contains("\"asOf\":\"2026-11-09\""));
    assert_eq!(go(&data, &net, None, &["business-names", "35100201200"]).0["count"], 1);

    // Up to date: the check exits 0 and a refresh does nothing.
    let (now, _, exit) = go(&data, &net, None, &["refresh", "--check"]);
    assert_eq!((exit, now["upToDate"].as_bool()), (0, Some(true)), "{now}");
    assert_eq!(now["sources"][0]["status"], "current");
    let (again, _, exit) = go(&data, &net, None, &["refresh"]);
    assert_eq!((exit, again["upToDate"].as_bool(), again["dataVersion"].clone()), (0, Some(true), done["dataVersion"].clone()));
    assert_eq!(server.hits("GET /files/"), 2);

    // The company file is newer: only it is fetched, and the business names are kept.
    *modified.lock().unwrap() = "2026-12-09T13:53:06.104534".into();
    let (stale, _, exit) = go(&data, &net, None, &["refresh", "--check"]);
    assert_eq!(
        (exit, stale["updates"].to_string(), stale["sources"][0]["status"].clone()),
        (10, "[\"asic\"]".to_string(), Value::from("update")),
        "{stale}"
    );
    let (next, _, exit) = go(&data, &net, None, &["refresh"]);
    assert_eq!((exit, next["downloaded"].to_string()), (0, "[\"asic\"]".to_string()), "{next}");
    assert_eq!(next["sources"][0]["asOf"], "2026-12-09");
    assert_ne!(next["dataVersion"], done["dataVersion"]);
    assert_eq!((server.hits("GET /files/company"), server.hits("GET /files/business")), (2, 1));
    assert_eq!(go(&data, &net, None, &["business-names", "35100201200"]).0["count"], 1);
    assert_eq!(go(&data, &net, None, &["refresh", "--check"]).2, 0);
    // The old release's folder is gone.
    assert_eq!(fs::read_dir(data.join("downloads/asic")).unwrap().count(), 1);
    let _ = fs::remove_dir_all(&data);
}

#[test]
fn a_download_that_breaks_resumes_by_range() {
    let data = temp("resume");
    let (company, names) = (asic_zip(), bn_zip());
    assert!(company.len() > 400, "the cut must leave the file unfinished");
    let breaks = Arc::new(AtomicUsize::new(4));
    let left = breaks.clone();
    let server = Server::start(move |req: &Req| match req.path() {
        "/ckan" if req.target.ends_with(ASIC) => Resp::json(&package(&[(
            "Company Dataset - Current",
            "ZIP",
            format!("{}/files/company_202611.zip", req.base),
            company.len() as u64,
            MODIFIED,
        )])),
        "/ckan" if req.target.ends_with(ASIC_BN) => Resp::json(&package(&[(
            "Business Names Dataset - Current",
            "ZIP",
            format!("{}/files/business_names_202611.zip", req.base),
            names.len() as u64,
            "2026-11-10T19:35:59",
        )])),
        "/files/company_202611.zip" => {
            let mut resp = file(req, &company);
            if left.load(Ordering::SeqCst) > 0 {
                left.fetch_sub(1, Ordering::SeqCst);
                resp.cut = Some(40);
            }
            resp
        }
        "/files/business_names_202611.zip" => file(req, &names),
        _ => Resp::status(404),
    });
    let net = server.net();
    // The index there is already: business names, from an earlier refresh.
    assert_eq!(go(&data, &net, None, &["refresh", "--source", "asicBn"]).2, 0);
    let before = go(&data, &net, None, &["status"]).0["dataVersion"].clone();

    // The connection breaks on every try of this run: the run fails, the index answers as before, 160 bytes wait on disk.
    let (failed, _, exit) = go(&data, &net, None, &["refresh", "--source", "asic"]);
    assert_eq!((exit, code(&failed)), (2, Some("refresh-failed")), "{failed}");
    assert_eq!(go(&data, &net, None, &["status"]).0["dataVersion"], before);
    assert_eq!(go(&data, &net, None, &["business-names", "35100201200"]).0["count"], 1);
    let release = fs::read_dir(data.join("downloads/asic")).unwrap().next().unwrap().unwrap().path();
    let parts: Vec<u64> = fs::read_dir(&release)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
        .map(|e| e.metadata().unwrap().len())
        .collect();
    assert_eq!(parts, [160]);

    // A rerun continues from byte 160 and finishes.
    let (done, _, exit) = go(&data, &net, None, &["refresh", "--source", "asic"]);
    assert_eq!(exit, 0, "{done}");
    assert!(server.hits("range=160") >= 1, "{:?}", server.lines());
    assert_eq!(go(&data, &net, None, &["company", "100200306"]).0["count"], 1);
    assert_eq!(go(&data, &net, None, &["business-names", "35100201200"]).0["count"], 1);
    assert!(fs::read_dir(&release).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
    let _ = fs::remove_dir_all(&data);
}

#[test]
fn a_server_that_ignores_range_sends_the_file_again_and_it_ends_whole() {
    // The ABS site says `accept-ranges: bytes` and answers a range with 200 and the whole file.
    let data = temp("norange");
    let company = asic_zip();
    let breaks = Arc::new(AtomicUsize::new(1));
    let left = breaks.clone();
    let server = Server::start(move |req: &Req| match req.path() {
        "/ckan" => Resp::json(&package(&[(
            "Company Dataset - Current",
            "ZIP",
            format!("{}/files/company_202611.zip", req.base),
            company.len() as u64,
            MODIFIED,
        )])),
        "/files/company_202611.zip" => {
            let mut resp = Resp::body(company.clone());
            if left.load(Ordering::SeqCst) > 0 {
                left.fetch_sub(1, Ordering::SeqCst);
                resp.cut = Some(40);
            }
            resp
        }
        _ => Resp::status(404),
    });
    let net = server.net();
    let (done, _, exit) = go(&data, &net, None, &["refresh", "--source", "asic"]);
    assert_eq!(exit, 0, "{done}");
    assert_eq!((server.hits("GET /files/company"), server.hits("range=40")), (2, 1), "{:?}", server.lines());
    assert_eq!(go(&data, &net, None, &["company", "100200306"]).0["count"], 1);
    let _ = fs::remove_dir_all(&data);
}

#[test]
fn a_refresh_that_must_be_refused_changes_nothing() {
    let company = asic_zip();
    let size = company.len() as u64;
    let mode = Arc::new(AtomicUsize::new(0));
    let which = mode.clone();
    let server = Server::start(move |req: &Req| match (req.path(), which.load(Ordering::SeqCst)) {
        ("/ckan", 0) => {
            Resp::json(&package(&[("Company Dataset - Current", "ZIP", "http://example.invalid/company.zip".into(), size, MODIFIED)]))
        }
        ("/ckan", 1) => {
            Resp::json(&package(&[("Company Dataset - Current", "ZIP", format!("{}/files/company_202611.zip", req.base), size, MODIFIED)]))
        }
        ("/ckan", 2) => Resp::json(&package(&[(
            "Company Dataset - Current",
            "ZIP",
            format!("{}/files/company_202611.zip", req.base),
            1_000_000_000_000_000,
            MODIFIED,
        )])),
        ("/ckan", _) => {
            Resp::json(&package(&[("Company Dataset - Current", "ZIP", format!("{}/files/company_202611.zip", req.base), 600, MODIFIED)]))
        }
        ("/files/company_202611.zip", 1) => Resp::status(302).header("Location", "http://example.com/company.zip"),
        ("/files/company_202611.zip", 3) => Resp::body(vec![7u8; 600]),
        _ => Resp::status(404),
    });
    let (net, data) = (server.net(), temp("refused"));
    let refused = |data: &std::path::Path, why: &str| {
        let (r, text, exit) = go(data, &server.net(), None, &["refresh", "--source", "asic"]);
        assert_eq!((exit, code(&r)), (2, Some("refresh-failed")), "{text}");
        assert!(r["error"]["message"].as_str().unwrap().contains(why), "wanted {why:?} in {text}");
        assert_eq!(go(data, &net, None, &["status"]).0["error"]["code"], "no-data", "nothing was indexed");
    };
    refused(&data, "not an official data host"); // CKAN names a host off the list
    mode.store(1, Ordering::SeqCst);
    refused(&data, "not an official data host"); // the file host redirects off the list
    assert_eq!(server.hits("example.com"), 0);
    mode.store(2, Ordering::SeqCst);
    let before = server.hits("GET /files/");
    refused(&data, "not enough free disk space"); // a file larger than the disk could take
    assert_eq!(server.hits("GET /files/"), before, "refused before any byte was fetched");
    mode.store(3, Ordering::SeqCst);
    refused(&data, "failed its check"); // the right size, and not a ZIP
    let left: usize =
        fs::read_dir(data.join("downloads/asic")).unwrap().flatten().map(|release| fs::read_dir(release.path()).unwrap().count()).sum();
    assert_eq!(left, 0, "no file of a refused release stays");
    let _ = fs::remove_dir_all(&data);
}

#[test]
fn an_index_made_from_files_is_not_dropped_by_a_refresh_of_one_source() {
    let (modified, data) = (Arc::new(Mutex::new(MODIFIED.to_string())), temp("from"));
    let server = ckan(modified);
    let net = server.net();
    let from = fixtures();
    let (built, _, exit) = go(&data, &net, None, &["refresh", "--from", from.to_str().unwrap()]);
    assert_eq!(exit, 0, "{built}");
    let (r, _, exit) = go(&data, &net, None, &["refresh", "--source", "asic"]);
    assert_eq!((exit, code(&r)), (2, Some("refresh-failed")), "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("--from"), "{r}");
    assert_eq!(go(&data, &net, None, &["status"]).0["dataVersion"], built["dataVersion"]);
    // --from and the network are not mixed in one call.
    assert_eq!(go(&data, &net, None, &["refresh", "--from", from.to_str().unwrap(), "--check"]).0["error"]["code"], "bad-source");
    assert_eq!(go(&data, &net, None, &["refresh", "--source", "nope"]).0["error"]["code"], "bad-source");
    assert_eq!(go(&data, &net, None, &["refresh", "--source"]).0["error"]["code"], "bad-source");
    let _ = fs::remove_dir_all(&data);
}

#[test]
fn abs_flows_are_fetched_whole_or_in_slices() {
    let data = temp("abs");
    let catalogue = serde_json::json!({"data": {"dataflows": [{"id": "POP", "agencyID": "ABS", "version": "1.0.0", "name": "Population"}, {"id": "OTHER", "agencyID": "ABS", "version": "1.0", "name": "Other"}]}});
    let head = "DATAFLOW,REGION,TIME_PERIOD,OBS_VALUE,UNIT_MEASURE\n";
    let server = Server::start(move |req: &Req| match req.target.as_str() {
        "/abs/dataflow/all" => Resp::json(&catalogue),
        "/abs/data/ABS,POP,1.0.0/all" => Resp::status(413),
        "/abs/data/ABS,POP,1.0.0/all?startPeriod=2020&endPeriod=2024" => {
            Resp::body(format!("{head}ABS:POP(1.0.0),1,2021,100,NUM\nABS:POP(1.0.0),1,2022,110,NUM\n").into_bytes())
        }
        "/abs/data/ABS,POP,1.0.0/all?startPeriod=2025&endPeriod=2029" => {
            Resp::body(format!("{head}ABS:POP(1.0.0),1,2025,120,NUM\n").into_bytes())
        }
        t if t.starts_with("/abs/data/") => Resp::status(404).header("Content-Type", "text/plain"),
        _ => Resp::status(404),
    });
    let net = server.net();
    assert_eq!(go(&data, &net, None, &["refresh", "--source", "abs", "--flow", "NOPE"]).0["error"]["code"], "bad-source");
    assert_eq!(go(&data, &net, None, &["refresh", "--source", "asic", "--flow", "POP"]).0["error"]["code"], "bad-source");
    assert_eq!(go(&data, &net, None, &["refresh", "--source", "abs", "--flow", "../x"]).0["error"]["code"], "bad-source");
    let (done, _, exit) = go(&data, &net, None, &["refresh", "--source", "abs", "--flow", "POP"]);
    assert_eq!((exit, done["downloaded"].to_string()), (0, "[\"abs\"]".to_string()), "{done}");
    let (rows, _, _) = go(&data, &net, None, &["stats", "get", "POP"]);
    let periods: Vec<&str> = rows["results"].as_array().unwrap().iter().map(|r| r["period"].as_str().unwrap()).collect();
    assert_eq!(periods, ["2021", "2022", "2025"], "{rows}");
    assert_eq!(rows["sources"]["abs"]["asOfKind"], "fetched");
    assert_eq!(go(&data, &net, None, &["stats", "find", "population"]).0["count"], 1);
    // The ABS says nothing of when its data changed: a check never reports an update, and the same day's fetch is kept.
    let (check, _, exit) = go(&data, &net, None, &["refresh", "--check"]);
    assert_eq!((exit, check["sources"][0]["status"].clone()), (0, Value::from("unknown")), "{check}");
    let fetched = server.hits("/abs/dataflow/all");
    assert_eq!(go(&data, &net, None, &["refresh"]).2, 0);
    assert_eq!(server.hits("/abs/dataflow/all"), fetched);
    let _ = fs::remove_dir_all(&data);
}

#[test]
fn the_pinned_asgs_file_is_found_by_its_headers() {
    let data = temp("asgs");
    let geojson = fs::read(fixtures().join("asgs/SA2_FIXTURE.geojson")).unwrap();
    let server = Server::start(move |req: &Req| match req.path() {
        "/asgs/SA2_FIXTURE.geojson" => file(req, &geojson).header("Last-Modified", "Thu, 24 Jun 2021 22:47:40 GMT"),
        _ => Resp::status(404),
    });
    let mut net = server.net();
    net.asgs = vec![format!("{}/asgs/SA2_FIXTURE.geojson", server.base)];
    let (check, _, exit) = go(&data, &net, None, &["refresh", "--check", "--source", "asgs"]);
    assert_eq!(
        (exit, check["sources"][0]["latest"]["lastModified"].clone(), check["sources"][0]["latest"]["asOf"].clone()),
        (10, Value::from("2021-06-24T22:47:40"), Value::from("2021-06-24")),
        "{check}"
    );
    assert_eq!(server.hits("GET"), 0);
    let (done, _, exit) = go(&data, &net, None, &["refresh", "--source", "asgs"]);
    assert_eq!(exit, 0, "{done}");
    let (area, _, _) = go(&data, &net, None, &["area", "--lat", "-33.87", "--lon", "151.2"]);
    assert_eq!(area["results"][0]["code"], "100000001", "{area}");
    assert_eq!(go(&data, &net, None, &["refresh", "--check"]).2, 0);
    let _ = fs::remove_dir_all(&data);
}

#[test]
fn a_damaged_state_file_is_ignored_and_removes_nothing() {
    let (modified, data) = (Arc::new(Mutex::new(MODIFIED.to_string())), temp("state"));
    let server = ckan(modified);
    let net = server.net();
    let precious = data.join("keepme");
    fs::create_dir_all(&precious).unwrap();
    fs::write(precious.join("file.txt"), "mine").unwrap();
    fs::create_dir_all(data.join("downloads")).unwrap();
    let state = r#"{"sources":{
        "../keepme":{"asOf":"2026-01-01","dir":"deadbeef","files":[]},
        "asicBn":{"asOf":"2026-01-01","dir":"../../keepme","files":[]},
        "asic":{"asOf":"2026-01-01","dir":"deadbeef","files":[{"name":"../x","url":"","size":1,"lastModified":"2026-01-01"}]},
        "abs":{"asOf":"2026-01-01","dir":"deadbeef","files":[],"flows":["../../keepme"],"fetched":true}}}"#;
    fs::write(data.join("downloads/state.json"), state).unwrap();
    // Every entry is dropped: nothing is installed, and a refresh builds from what it fetches, not from what the file names.
    let (check, _, exit) = go(&data, &net, None, &["refresh", "--check", "--source", "asic"]);
    assert_eq!((exit, check["sources"][0]["status"].clone()), (10, Value::from("not-installed")), "{check}");
    let (done, _, exit) = go(&data, &net, None, &["refresh", "--source", "asic"]);
    assert_eq!(exit, 0, "{done}");
    assert_eq!(fs::read_to_string(precious.join("file.txt")).unwrap(), "mine");
    assert_eq!(go(&data, &net, None, &["company", "100200306"]).0["count"], 1);
    let _ = fs::remove_dir_all(&data);
}

#[test]
fn a_whole_file_is_kept_when_the_server_states_no_size() {
    // The ASGS file: no Content-Length on the HEAD, so the release is known by its date alone. A file already there is whole.
    let data = temp("nosize");
    let geojson = fs::read(fixtures().join("asgs/SA2_FIXTURE.geojson")).unwrap();
    let server = Server::start(move |req: &Req| match req.path() {
        "/asgs/SA2_FIXTURE.geojson" => file(req, &geojson).header("Last-Modified", "Thu, 24 Jun 2021 22:47:40 GMT"),
        _ => Resp::status(404),
    });
    let mut net = server.net();
    net.asgs = vec![format!("{}/asgs/SA2_FIXTURE.geojson", server.base)];
    assert_eq!(go(&data, &net, None, &["refresh", "--source", "asgs"]).2, 0);
    // Forget that it was indexed (as if the run died before the state moved): the file is reused, not fetched again.
    fs::remove_file(data.join("downloads/state.json")).unwrap();
    assert_eq!(go(&data, &net, None, &["refresh", "--source", "asgs"]).2, 0);
    assert_eq!(server.hits("GET /asgs"), 1, "{:?}", server.lines());
    let _ = fs::remove_dir_all(&data);
}
