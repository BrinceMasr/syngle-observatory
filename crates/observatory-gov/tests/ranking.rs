//! `gov name` against a brute force over generated data: words that are in a large share of the names (so the search must
//! bound its work), several names per entity (so one entity must be one result), both statuses, and a sort that spills to
//! disk. The brute force is the contract's ranking written the plain way.

use observatory_gov::{Env, run};
use serde_json::Value;
use std::path::{Path, PathBuf};

const WORDS: [&str; 8] = ["PTY", "LTD", "HOLDINGS", "GREEN", "BLUE", "TRADING", "GROUP", "PLUMBING"];
const ENTITIES: u64 = 3000;

struct Entity {
    abn: u64,
    active: bool,
    state: &'static str,
    names: Vec<String>,
}

fn entities() -> Vec<Entity> {
    let mut seed = 12345u64;
    let mut next = move |m: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % m
    };
    (0..ENTITIES)
        .map(|i| {
            let mut name = |extra: bool| {
                let words: Vec<&str> = (0..1 + next(4)).map(|_| WORDS[next(WORDS.len() as u64) as usize]).collect();
                if extra { format!("{} {}", words.join(" "), next(50)) } else { words.join(" ") }
            };
            let names = vec![name(true), name(false), name(false)];
            Entity { abn: 10_000_000_000 + i * 7 + 1, active: next(4) != 0, state: ["NSW", "VIC", "QLD"][next(3) as usize], names }
        })
        .collect()
}

fn write_files(dir: &Path, all: &[Entity]) {
    let mut xml = format!("<?xml version=\"1.0\"?>\n<Transfer><TransferInfo><RecordCount>{}</RecordCount></TransferInfo>\n", all.len());
    for e in all {
        let status = if e.active { "ACT" } else { "CAN" };
        xml.push_str(&format!(
            "<ABR><ABN status=\"{status}\" ABNStatusFromDate=\"20200101\">{:011}</ABN><EntityType><EntityTypeText>Australian Private Company</EntityTypeText></EntityType>\
<MainEntity><NonIndividualName type=\"MN\"><NonIndividualNameText>{}</NonIndividualNameText></NonIndividualName><BusinessAddress><AddressDetails><State>{}</State><Postcode>2000</Postcode></AddressDetails></BusinessAddress></MainEntity>",
            e.abn, e.names[0], e.state
        ));
        for n in &e.names[1..] {
            xml.push_str(&format!("<OtherEntity><NonIndividualName type=\"TRD\"><NonIndividualNameText>{n}</NonIndividualNameText></NonIndividualName></OtherEntity>"));
        }
        xml.push_str("</ABR>\n");
    }
    xml.push_str("</Transfer>\n");
    std::fs::create_dir_all(dir.join("abn")).unwrap();
    std::fs::write(dir.join("abn/e.xml"), xml).unwrap();
    std::fs::write(dir.join("manifest.json"), r#"{"format":1,"sources":{"abr":{"asOf":"2026-11-05","files":["abn/e.xml"]}}}"#).unwrap();
}

fn call(data: &Path, run_bytes: usize, args: &[&str]) -> Value {
    let env = Env { gov_dir: Some(data.to_path_buf()), local_data: None, sort_run_bytes: run_bytes };
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    serde_json::from_str(&run(&args, &env).stdout).unwrap()
}

/// The plain ranking: per entity its best name by (class, length), entities by (class, status, length, name, ABN).
fn brute(all: &[Entity], query: &str, state: Option<&str>, active: bool, limit: usize) -> Vec<(u64, String)> {
    let words: Vec<&str> = query.split(' ').collect();
    let mut ranked = Vec::new();
    for e in all {
        if (active && !e.active) || state.is_some_and(|s| s != e.state) {
            continue;
        }
        let mut best: Option<((u8, usize), &String)> = None;
        for n in &e.names {
            let cls = if n == query {
                0
            } else if n.starts_with(query) {
                1
            } else if words.iter().all(|w| n.split(' ').any(|x| x == *w)) {
                2
            } else {
                continue;
            };
            if best.is_none_or(|(k, _)| (cls, n.len()) < k) {
                best = Some(((cls, n.len()), n));
            }
        }
        if let Some(((cls, len), n)) = best {
            ranked.push(((cls, !e.active, len, n.clone(), e.abn), e.abn));
        }
    }
    ranked.sort();
    ranked.into_iter().take(limit).map(|(k, abn)| (abn, k.3)).collect()
}

#[test]
fn common_words_rank_as_the_plain_ranking_does() {
    let (all, root) = (entities(), std::env::temp_dir().join(format!("obs-gov-rank-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("src");
    write_files(&src, &all);
    let data: PathBuf = root.join("data");
    for run_bytes in [256 << 20, 4096] {
        let _ = std::fs::remove_dir_all(&data);
        let r = call(&data, run_bytes, &["refresh", "--from", src.to_str().unwrap()]);
        assert_eq!(r["ok"], true, "{r}");
        for query in ["PTY", "pty", "pty ltd", "green", "holdings group", "blue trading pty", "plumbing 7", "zzz", "p"] {
            for (limit, state, active) in
                [(5, None, false), (17, None, true), (50, Some("NSW"), false), (3, Some("VIC"), true), (1, None, false)]
            {
                let mut args = vec!["name", query, "--limit"];
                let limit_text = limit.to_string();
                args.push(&limit_text);
                if let Some(s) = state {
                    args.extend(["--state", s]);
                }
                if active {
                    args.push("--active");
                }
                let got = call(&data, run_bytes, &args);
                let got: Vec<(u64, String)> = got["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| (r["abn"].as_str().unwrap().parse().unwrap(), r["matched"].as_str().unwrap().to_string()))
                    .collect();
                if query == "pty" || query == "p" {
                    assert_eq!(
                        got.len(),
                        if state.is_some() || active { got.len().min(limit) } else { limit },
                        "a common word fills the limit"
                    );
                    assert!(!got.is_empty());
                }
                let norm = query.to_uppercase();
                assert_eq!(
                    got,
                    brute(&all, &norm, state, active, limit),
                    "{query} limit {limit} state {state:?} active {active} run {run_bytes}"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}
