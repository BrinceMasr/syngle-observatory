//! The pinned ASGS addresses answer on the real ABS site, and CKAN still lists what the refresh looks for. These touch the
//! internet, so they are ignored by default: run them on purpose with
//! `cargo test -p observatory-gov --test pinned -- --ignored --nocapture` (the boat box does).

use observatory_gov::Net;
use std::process::Command;

fn head(url: &str) -> (String, String) {
    let out = Command::new("curl").args(["-sS", "-I", "-L", "-m", "60", "-A", "syngle-map-gov-test", url]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let status = text.lines().rfind(|l| l.starts_with("HTTP/")).unwrap_or_default().to_string();
    (status, text.to_ascii_lowercase())
}

#[test]
#[ignore = "touches the internet"]
fn every_pinned_asgs_address_answers_with_a_zip() {
    let net = Net::official();
    assert!(!net.asgs.is_empty());
    for url in &net.asgs {
        let (status, text) = head(url);
        assert!(status.contains(" 200"), "{url}: {status}");
        assert!(text.contains("last-modified:"), "{url} has no Last-Modified: {text}");
        assert!(text.contains("accept-ranges: bytes"), "{url} does not take ranges");
        println!("ok {url}");
    }
}

#[test]
#[ignore = "touches the internet"]
fn ckan_lists_the_files_each_source_looks_for() {
    for (source, id) in [
        ("abr", "5bd7fcab-e315-42cb-8daf-50b7efc2027e"),
        ("asic", "7b8656f9-606d-4337-af29-66b89b2eeefb"),
        ("asicBn", "bc515135-4bb6-4d50-957a-3713709a76d3"),
        ("gnaf", "geocoded-national-address-file-g-naf"),
    ] {
        let out = Command::new("curl").args(["-sS", "-m", "60", &format!("{}{id}", Net::official().ckan)]).output().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(json["success"], true, "{source}");
        let zips = json["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["format"].as_str().is_some_and(|f| f.eq_ignore_ascii_case("zip")))
            .count();
        assert!(zips >= 1, "{source} lists no ZIP");
        println!("ok {source}: {zips} ZIP resources");
    }
}
