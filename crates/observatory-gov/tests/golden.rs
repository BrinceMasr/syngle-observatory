//! The evaluator's fixtures and golden cases (`evals/gov-data`), run in process: the same answers the frozen evaluator
//! asks for, so a red here is a red there, minutes sooner. Source kinds not built yet are skipped (`BUILT`).

use observatory_gov::{Env, run};
use serde_json::Value;
use std::path::{Path, PathBuf};

const BUILT: &[&str] = &["abn", "name", "company"];

fn evals() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/gov-data/fixtures")
}

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obs-gov-golden-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn env(dir: &Path, sort_run_bytes: usize) -> Env {
    Env { gov_dir: Some(dir.to_path_buf()), local_data: None, sort_run_bytes }
}

fn call(dir: &Path, args: &[&str]) -> (Value, String, u8) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let out = run(&args, &env(dir, 256 << 20));
    let json = serde_json::from_str(&out.stdout).unwrap_or(Value::Null);
    (json, out.stdout, out.exit)
}

fn get<'a>(v: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted.split('.').try_fold(v, |cur, k| match k.parse::<usize>() {
        Ok(i) if cur.is_array() => cur.get(i),
        _ => cur.get(k),
    })
}

fn golden() -> Value {
    serde_json::from_str(&std::fs::read_to_string(evals().join("golden.json")).unwrap()).unwrap()
}

fn refresh(dir: &Path, version: &str, sort_run_bytes: usize) -> Value {
    let from = evals().join(version);
    let args = vec!["refresh".to_string(), "--from".to_string(), from.to_string_lossy().into_owned()];
    let out = run(&args, &env(dir, sort_run_bytes));
    serde_json::from_str(&out.stdout).unwrap()
}

fn check_cases(dir: &Path, version: &str, g: &Value) -> Vec<String> {
    let mut failed = Vec::new();
    for c in g["cases"].as_array().unwrap() {
        let kind = c["kind"].as_str().unwrap();
        if c["v"] != version || !BUILT.contains(&kind) {
            continue;
        }
        let args: Vec<&str> = c["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        let (json, text, exit) = call(dir, &args);
        let wanted_ok = c["expect"]["ok"].as_bool().unwrap_or(true);
        if exit != if wanted_ok { 0 } else { 2 } {
            failed.push(format!("{}: exit {exit}", c["id"]));
        }
        for (path, want) in c["expect"].as_object().unwrap() {
            if get(&json, path) != Some(want) {
                failed.push(format!(
                    "{}: {path} wanted {want} got {:?} in {}",
                    c["id"],
                    get(&json, path),
                    text.chars().take(300).collect::<String>()
                ));
            }
        }
    }
    failed
}

#[test]
fn golden_cases_pass_at_both_versions() {
    let (dir, g) = (temp("cases"), golden());
    let r1 = refresh(&dir, "v1", 256 << 20);
    assert_eq!(r1["ok"], true, "{r1}");
    assert_eq!(check_cases(&dir, "v1", &g), Vec::<String>::new());
    let r2 = refresh(&dir, "v2", 256 << 20);
    assert_eq!(r2["ok"], true, "{r2}");
    assert_ne!(r1["dataVersion"], r2["dataVersion"]);
    assert_eq!(check_cases(&dir, "v2", &g), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_same_files_give_the_same_version_and_the_same_bytes_however_they_are_sorted() {
    let (a, b) = (temp("det-a"), temp("det-b"));
    let ra = refresh(&a, "v1", 256 << 20);
    // A sort run of 1 KiB makes every table go through the external merge.
    let rb = refresh(&b, "v1", 1024);
    assert_eq!(ra["dataVersion"], rb["dataVersion"]);
    for args in
        [["abn", "14100200301"], ["name", "harbour"], ["name", "kangaroo red"], ["company", "100200306"], ["name", "harbour and co"]]
    {
        assert_eq!(call(&a, &args).1, call(&b, &args).1, "{args:?}");
    }
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

#[test]
fn lifecycle_pinned_unchanged_corrupt_and_before_any_refresh() {
    let dir = temp("life");
    assert_eq!(call(&dir, &["abn", "14100200301"]).0["error"]["code"], "no-data");
    let r1 = refresh(&dir, "v1", 256 << 20);
    let again = refresh(&dir, "v1", 256 << 20);
    assert_eq!((again["built"].as_bool(), &again["dataVersion"]), (Some(false), &r1["dataVersion"]));
    let before = call(&dir, &["abn", "14100200301"]).1;
    let r2 = refresh(&dir, "v2", 256 << 20);
    let pin = r1["dataVersion"].as_str().unwrap();
    assert_eq!(call(&dir, &["abn", "14100200301", "--data-version", pin]).1, before);
    assert_eq!(call(&dir, &["abn", "14100200301", "--data-version", "gov-00000000"]).0["error"]["code"], "unknown-data-version");

    let bad = temp("life-bad");
    std::fs::create_dir_all(bad.join("abn")).unwrap();
    for f in ["manifest.json", "abn/public_split_1_10.xml", "asic/company_202611.csv", "asic/business_names_202611.csv"] {
        let to = bad.join(f);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(evals().join("v2").join(f), to).unwrap();
    }
    let xml = std::fs::read_to_string(bad.join("abn/public_split_1_10.xml")).unwrap();
    std::fs::write(bad.join("abn/public_split_1_10.xml"), &xml[..700]).unwrap();
    let args = vec!["refresh".to_string(), "--from".to_string(), bad.to_string_lossy().into_owned()];
    let refused = run(&args, &env(&dir, 256 << 20));
    assert_eq!(refused.exit, 2);
    let status = call(&dir, &["status"]).0;
    assert_eq!(status["dataVersion"], r2["dataVersion"]);
    assert_eq!(call(&dir, &["abn", "14100200301"]).0["dataVersion"], r2["dataVersion"]);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&bad);
}

#[test]
fn hostile_input_is_answered_in_json() {
    let (dir, g) = (temp("hostile"), golden());
    refresh(&dir, "v1", 256 << 20);
    for h in g["hostile"].as_array().unwrap() {
        let args: Vec<&str> = h.as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        let (json, text, exit) = call(&dir, &args);
        assert!(exit == 0 || exit == 2, "{args:?}");
        assert!(json["ok"].is_boolean(), "{args:?} gave {text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_answer_names_its_version_sources_licence_and_attribution() {
    let dir = temp("versioned");
    let r = refresh(&dir, "v1", 256 << 20);
    let a = call(&dir, &["name", "harbour"]).0;
    assert_eq!(a["dataVersion"], r["dataVersion"]);
    for source in ["abr", "asic", "asicBn"] {
        let s = &a["sources"][source];
        assert!(s["dataVersion"].as_str().unwrap().starts_with(source));
        assert_eq!(s["asOf"].as_str().unwrap().len(), 10);
        assert!(!s["licence"].as_str().unwrap().is_empty() && !s["attribution"].as_str().unwrap().is_empty());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
