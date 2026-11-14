//! `map.findPlace` with `provider: "gnaf"`: an Australian address found offline, in the G-NAF index of `observatory-gov` on this
//! machine (built with `syngle-map gov refresh --from <folder>`, see `docs/gov.md`), answered in the shape the other providers
//! answer in (Syngle-Ecosystem #635, after #629).
//!
//! The index is read through the library's own entry, the one `syngle-map gov address` runs, so the two cannot disagree: same
//! snapshot, same ranking (the closest address first, the G-NAF PID breaking ties), same refusals. Nothing is fetched. A machine
//! with no index gets no places and a note that says how to build it, never an error (a person or an agent can ask before there
//! is data, and the answer tells them what to do).

use observatory_gov::Env;
use serde_json::{Value, json};

use crate::cmd::{b, ps};
use crate::render::rustgis_tiles::geocode::Place;
use crate::{CmdResult, Session};

/// The longest address one search takes (a real one is under 100): a longer text is not an address.
const MAX_QUERY: usize = 300;

/// What a search found: the places, the data they came from (version, date, licence and attribution, which the G-NAF
/// licence asks to be shown with it), and what to do when there is no index.
pub(super) struct Found {
    pub places: Vec<Place>,
    pub source: Option<Value>,
    pub note: Option<&'static str>,
}

const NO_INDEX: &str = "there is no G-NAF address data on this machine: index the official files with `syngle-map gov refresh --from <folder>` (see the Government Data page), then ask again";

/// Whether the command asks for this provider.
pub(super) fn wanted(p: &Value) -> bool {
    ps(p, "provider").is_some_and(|v| v.trim().eq_ignore_ascii_case("gnaf"))
}

/// The command: search, then zoom to the first address unless told not to.
pub(super) fn find(s: &mut Session, p: &Value, query: &str, limit: usize, env: &Env) -> CmdResult {
    if ps(p, "url").is_some_and(|u| !u.trim().is_empty()) {
        return Err("provider gnaf reads the address index on this machine and takes no url".into());
    }
    let found = search(env, query, limit)?;
    if b(p, "go").unwrap_or(true)
        && let Some(first) = found.places.first()
    {
        super::geocode::go_to_place(s, first);
    }
    let mut out = json!({"count": found.places.len(), "places": found.places.iter().map(super::geocode::place_json).collect::<Vec<_>>()});
    if let Some(o) = out.as_object_mut() {
        if let Some(source) = found.source {
            o.insert("source".into(), source);
        }
        if let Some(note) = found.note {
            o.insert("note".into(), note.into());
        }
    }
    Ok(out)
}

/// Runs `gov address` for a text and reads its answer.
pub(super) fn search(env: &Env, query: &str, limit: usize) -> Result<Found, String> {
    // The library reads its arguments as a command line: a text that opens with `--` would be a flag (a data folder, say),
    // and leading dashes are no part of an address (the index reads a dash as a space).
    let text = query.trim().trim_start_matches('-').trim_start();
    if text.is_empty() {
        return Err("give an address to find".into());
    }
    if text.chars().count() > MAX_QUERY {
        return Err(format!("an address is up to {MAX_QUERY} characters"));
    }
    let args = ["address".to_string(), text.to_string(), "--limit".to_string(), limit.clamp(1, 20).to_string()];
    let out = observatory_gov::run(&args, env);
    parse(&out.stdout)
}

/// The answer line of `gov address` as places, or the reason it refused.
fn parse(line: &str) -> Result<Found, String> {
    let v: Value = serde_json::from_str(line).map_err(|_| "the address index answered with something that is not JSON".to_string())?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        let e = v.get("error");
        let code = e.and_then(|e| e.get("code")).and_then(Value::as_str).unwrap_or("");
        let message = e.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("it gave no reason");
        // Nothing indexed is an answer of no places; a refusal of anything else (a damaged index, a bad query) is an error.
        return if code == "no-data" {
            Ok(Found { places: Vec::new(), source: None, note: Some(NO_INDEX) })
        } else {
            Err(format!("G-NAF: {message}"))
        };
    }
    // A place the index could not give a point for (a damaged record) is left out, as the other providers leave out theirs.
    let places = v
        .get("results")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let (lat, lon) = (r.get("lat")?.as_f64()?, r.get("lon")?.as_f64()?);
                    let name = r.get("address")?.as_str()?.to_string();
                    let valid = lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0;
                    valid.then_some(Place { name, lon, lat, bbox: None })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let source = v.get("sources").and_then(|s| s.get("gnaf")).map(|g| {
        let field = |k: &str| g.get(k).cloned().unwrap_or(Value::Null);
        json!({"name": "G-NAF", "dataVersion": field("dataVersion"), "asOf": field("asOf"), "licence": field("licence"), "attribution": field("attribution")})
    });
    Ok(Found { places, source, note: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    const FIXTURES: &str = "../../evals/gov-data/fixtures/v1";

    /// A data folder with the evaluator's G-NAF fixture indexed, as `syngle-map gov refresh --from` leaves it.
    fn indexed(tag: &str) -> (Env, PathBuf) {
        let dir = std::env::temp_dir().join(format!("engine-gnaf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let env = Env { gov_dir: Some(dir.clone()), local_data: None, sort_run_bytes: 4096 };
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES);
        let out = observatory_gov::run(&["refresh".into(), "--from".into(), from.to_string_lossy().into_owned()], &env);
        assert_eq!(out.exit, 0, "{}", out.stdout);
        (env, dir)
    }

    #[test]
    fn an_address_is_found_in_the_index_with_its_source() {
        let (env, dir) = indexed("found");
        let f = search(&env, "12 Smith St Sydney", 5).unwrap();
        // Two addresses have those words; the one with the fewer extra words (no unit) comes first.
        assert_eq!(f.places.len(), 2);
        assert_eq!(f.places[0].name, "12 SMITH STREET, SYDNEY NSW 2000");
        assert!((f.places[0].lon - 151.2094).abs() < 1e-9 && (f.places[0].lat + 33.8689).abs() < 1e-9);
        assert_eq!(f.places[1].name, "UNIT 3/12 SMITH STREET, SYDNEY NSW 2000");
        assert_eq!(f.places[0].bbox, None);
        let src = f.source.unwrap();
        assert_eq!((src["name"].as_str(), src["asOf"].as_str()), (Some("G-NAF"), Some("2026-08-17")));
        assert!(
            src["licence"].as_str().is_some_and(|l| !l.is_empty()) && src["attribution"].as_str().is_some_and(|a| !a.is_empty()),
            "{src}"
        );
        // The limit holds, and a text of no address finds nothing (an answer, not an error).
        assert_eq!(search(&env, "12 Smith St Sydney", 1).unwrap().places.len(), 1);
        assert_eq!(search(&env, "zzzz nowhere", 5).unwrap().places.len(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_command_answers_like_the_other_providers_and_goes_to_the_first_address() {
        let (env, dir) = indexed("command");
        let mut s = Session::new(crate::Project::default());
        let before = s.map().camera.center;
        let r = find(&mut s, &json!({"query": "x", "provider": "GNAF "}), "unit 3/12 smith st, sydney nsw 2000", 5, &env).unwrap();
        assert_eq!(r["count"], 1);
        let place = &r["places"][0];
        assert_eq!(place["name"], "UNIT 3/12 SMITH STREET, SYDNEY NSW 2000");
        assert!(place["lon"].is_number() && place["lat"].is_number() && place["bbox"].is_null(), "{place}");
        assert_eq!(r["source"]["asOf"], "2026-08-17");
        assert!(s.map().camera.center != before, "the view moved to the address");
        // `go: false` leaves the view, and a url is refused (the index is local).
        let kept = s.map().camera.center;
        find(&mut s, &json!({"go": false}), "12 smith st sydney", 5, &env).unwrap();
        assert_eq!(s.map().camera.center, kept);
        assert!(find(&mut s, &json!({"url": "https://example.org"}), "12 smith st", 5, &env).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_text_cannot_pass_a_flag_to_the_library() {
        let (env, dir) = indexed("flags");
        // `--data-dir` would send the search to another folder, `--id` to a lookup by PID: neither is read as a flag.
        let other = std::env::temp_dir().join("engine-gnaf-no-such-folder");
        let text = format!("--data-dir {} 12 smith st sydney", other.display());
        let f = search(&env, &text, 5);
        assert!(f.is_ok_and(|f| f.places.is_empty()), "the words of the folder's name are words of the address");
        assert_eq!(search(&env, "--id GANSW000000001", 5).unwrap().places.len(), 0);
        assert!(search(&env, " -- ", 5).is_err() && search(&env, "---", 5).is_err());
        assert!(search(&env, &"a ".repeat(MAX_QUERY), 5).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_data_and_a_damaged_answer_are_told_plainly() {
        let empty = Env {
            gov_dir: Some(std::env::temp_dir().join(format!("engine-gnaf-none-{}", std::process::id()))),
            local_data: None,
            sort_run_bytes: 4096,
        };
        let f = search(&empty, "12 smith st", 5).unwrap();
        let note = f.note.unwrap_or_default();
        assert!(f.places.is_empty() && f.source.is_none(), "no index, no places");
        assert!(note.contains("no G-NAF address data") && note.contains("gov refresh --from"), "{note}");
        let mut s = Session::new(crate::Project::default());
        let r = find(&mut s, &json!({}), "12 smith st", 5, &empty).unwrap();
        assert_eq!(r["count"], 0);
        assert!(r["note"].as_str().is_some_and(|n| n.contains("syngle-map gov refresh")), "{r}");
        assert!(parse("not json").is_err() && parse("").is_err() && parse("{}").is_err());
        let e = parse(r#"{"ok":false,"error":{"code":"empty-query","message":"give an address to search for"}}"#).err().unwrap_or_default();
        assert_eq!(e, "G-NAF: give an address to search for");
        // A row with a missing or impossible point is left out, the others stay.
        let line = r#"{"ok":true,"count":4,"results":[{"address":"A","lat":-33.0,"lon":151.0},{"address":"B","lat":95.0,"lon":1.0},{"address":"C","lat":1.0},{"lat":1.0,"lon":1.0},{"address":"D","lat":"x","lon":1.0}]}"#;
        let f = parse(line).unwrap();
        assert_eq!(f.places.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["A"]);
        assert!(f.source.is_none());
    }
}
