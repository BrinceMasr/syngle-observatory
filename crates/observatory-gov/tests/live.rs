//! `gov abn --live` against a local server that answers as the ABN Lookup JSON services do (a JSONP callback, a `Message` for a
//! refusal): the entity in `gov abn`'s shape, and the GUID, which travels in the request, nowhere else.

mod support;

use serde_json::Value;
use std::sync::{Arc, Mutex};
use support::{Req, Resp, Server, go, temp};

const GUID: &str = "0a1b2c3d-4e5f-6789-abcd-ef0123456789";
const ABN: &str = "51824753556";

const ENTITY: &str = r#"{"Abn":"51824753556","AbnStatus":"Active","AbnStatusEffectiveFrom":"2000-07-01","Acn":"","AddressDate":"2019-01-01","AddressPostcode":"2600","AddressState":"ACT","BusinessName":["Kangaroo Co"],"EntityName":"EXAMPLE PTY LTD","EntityTypeCode":"PRV","EntityTypeName":"Australian Private Company","Gst":"2000-07-01","Message":""}"#;

/// What https://abr.business.gov.au/json/AbnDetails.aspx answered, byte for byte, to a request with a GUID it does not know
/// (recorded on 14 November 2026 with `callback=callback`). The reply to a registered GUID could not be recorded: it needs Omar's.
const UNREGISTERED: &str = r#"callback({"Abn":"","AbnStatus":"","AbnStatusEffectiveFrom":"","Acn":"","AddressDate":null,"AddressPostcode":"","AddressState":"","BusinessName":[],"EntityName":"","EntityTypeCode":"","EntityTypeName":"","Gst":null,"Message":"The GUID entered is not recognised as a Registered Party"})"#;

fn query_of(req: &Req, name: &str) -> String {
    let q = req.target.split_once('?').map_or("", |(_, q)| q);
    q.split('&').find_map(|kv| kv.strip_prefix(&format!("{name}="))).unwrap_or_default().to_string()
}

fn jsonp(json: &str) -> Resp {
    Resp::body(format!("c({json})").into_bytes())
}

fn refusal(message: &str) -> String {
    serde_json::json!({"Abn": "", "AbnStatus": "", "Acn": "", "BusinessName": [], "EntityName": "", "Message": message}).to_string()
}

/// A server that knows one registered GUID.
fn abr(failing: Arc<Mutex<bool>>) -> Server {
    Server::start(move |req: &Req| {
        if *failing.lock().unwrap() {
            return Resp::status(500);
        }
        let guid = query_of(req, "guid");
        match req.path() {
            _ if guid != GUID => Resp::body(UNREGISTERED.as_bytes().to_vec()),
            "/json/AbnDetails.aspx" if query_of(req, "abn") == ABN => jsonp(ENTITY),
            "/json/AbnDetails.aspx" => jsonp(&refusal("No records found")),
            "/json/AcnDetails.aspx" => jsonp(&refusal(&format!("Rejected {guid}"))),
            _ => Resp::status(404),
        }
    })
}

#[test]
fn a_live_answer_has_the_shape_of_gov_abn_and_says_it_is_live() {
    let (server, data) = (abr(Arc::new(Mutex::new(false))), temp("live"));
    let net = server.net();
    // No snapshot is needed, and nothing is stored.
    let (a, text, exit) = go(&data, &net, Some(GUID), &["abn", "--live", "51 824 753 556"]);
    assert_eq!(exit, 0, "{text}");
    assert_eq!((a["live"].as_bool(), a["count"].as_u64(), a["dataVersion"].as_str()), (Some(true), Some(1), Some("live")));
    let src = &a["sources"]["abnLookup"];
    assert_eq!((src["asOfKind"].as_str(), src["dataVersion"].as_str()), (Some("fetched"), Some("live")));
    assert_eq!(src["asOf"].as_str().map(str::len), Some(10));
    assert_eq!(
        a["results"][0],
        serde_json::from_str::<Value>(r#"{"abn":"51824753556","status":"Active","statusFrom":"2000-07-01","entityType":"Australian Private Company","name":"EXAMPLE PTY LTD","state":"ACT","postcode":"2600","acn":null,"gst":true,"names":[{"type":"Business name","text":"Kangaroo Co"}]}"#).unwrap()
    );
    assert_eq!(a["query"].to_string(), format!("{{\"abn\":\"{ABN}\",\"live\":true,\"verb\":\"abn\"}}"));
    assert!(!text.contains(GUID));
    assert!(server.lines().iter().any(|l| l.contains(&format!("guid={GUID}"))), "the GUID is sent to the service");
    assert!(!data.join("current").exists() && !data.join("snapshots").exists(), "nothing is stored");
    // Not found is an answer, not an error.
    let (none, _, exit) = go(&data, &net, Some(GUID), &["abn", "--live", "53004085616"]);
    assert_eq!((exit, none["count"].as_u64()), (0, Some(0)), "{none}");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn the_guid_is_never_shown_and_a_bad_request_never_leaves_the_machine() {
    let (failing, data) = (Arc::new(Mutex::new(false)), temp("live-guid"));
    let server = abr(failing.clone());
    let net = server.net();
    let code = |v: &Value| v["error"]["code"].as_str().map(String::from);
    // Refused before any request: no GUID, a GUID of the wrong shape (which could carry a second parameter), a bad ABN.
    let (v, _, exit) = go(&data, &net, None, &["abn", "--live", ABN]);
    assert_eq!((exit, code(&v).as_deref()), (2, Some("no-data")));
    assert!(v["error"]["message"].as_str().unwrap().contains("SYNGLE_ABN_LOOKUP_GUID"));
    for bad in ["", "   ", "not-a-guid&abn=1", "0a1b2c3d-4e5f-6789-abcd-ef0123456789&x=1"] {
        let (v, text, exit) = go(&data, &net, Some(bad), &["abn", "--live", ABN]);
        assert_eq!(exit, 2);
        assert!(["no-data", "bad-source"].contains(&code(&v).unwrap().as_str()), "{text}");
        assert!(bad.trim().is_empty() || !text.contains(bad), "{text}");
    }
    let (v, _, exit) = go(&data, &net, Some(GUID), &["abn", "--live", "12345678901"]);
    assert_eq!((exit, code(&v).as_deref()), (2, Some("invalid-abn")));
    assert_eq!(server.hits("json/"), 0, "nothing was sent");
    // A GUID the service does not know, an echo of the GUID in the service's own message, a failing service.
    let other = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    let (v, text, exit) = go(&data, &net, Some(other), &["abn", "--live", ABN]);
    assert_eq!((exit, code(&v).as_deref()), (2, Some("refresh-failed")), "{text}");
    assert!(!text.contains(other) && text.contains("does not recognise the GUID"));
    let (v, text, exit) = go(&data, &net, Some(GUID), &["abn", "--live", "100200306"]);
    assert_eq!((exit, code(&v).as_deref()), (2, Some("refresh-failed")), "{text}");
    assert!(!text.to_ascii_lowercase().contains(GUID) && text.contains("ABN Lookup said: Rejected <guid>"), "{text}");
    *failing.lock().unwrap() = true;
    let (v, text, exit) = go(&data, &net, Some(GUID), &["abn", "--live", ABN]);
    assert_eq!((exit, code(&v).as_deref()), (2, Some("refresh-failed")), "{text}");
    assert!(!text.contains(GUID) && !text.contains("guid="), "{text}");
    // A host off the list is not called, with a GUID or without one.
    let mut off = net.clone();
    off.abn_json = "http://example.invalid/json/".into();
    let (v, text, _) = go(&data, &off, Some(GUID), &["abn", "--live", ABN]);
    assert_eq!(code(&v).as_deref(), Some("refresh-failed"));
    assert!(text.contains("not an official data host") && !text.contains(GUID), "{text}");
    let _ = std::fs::remove_dir_all(&data);
}
