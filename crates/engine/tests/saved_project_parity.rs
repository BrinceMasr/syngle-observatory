//! A map drawn from its saved copy is the map the editor draws (Syngle-Ecosystem #382, #419).
//!
//! The app keeps a map's project in the editor on the person's computer and pushes its saved text
//! (`info.projectText`) to the machine that has the data. `syngle-map` on that machine draws and scans
//! from that text with no editor attached. So the two must draw the same picture from the same project.

use rustgis_engine::cmd::share::map_png;
use rustgis_engine::{Project, Session};
use serde_json::{Value, json};

/// `n` points in a fixed scatter over Europe, as GeoJSON text. Deterministic: no clock, no random source.
fn points(n: usize) -> String {
    let mut seed: u64 = 0x5eed_1234_abcd_ef01;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out = String::with_capacity(n * 96);
    out.push_str(r#"{"type":"FeatureCollection","features":["#);
    for i in 0..n {
        if i > 0 {
            out.push(',');
        }
        let (lon, lat) = (-10.0 + next() * 40.0, 36.0 + next() * 24.0);
        out.push_str(&format!(
            r#"{{"type":"Feature","properties":{{"id":{i},"value":{}}},"geometry":{{"type":"Point","coordinates":[{lon:.5},{lat:.5}]}}}}"#,
            i % 97
        ));
    }
    out.push_str("]}");
    out
}

fn editor_with(n: usize) -> Session {
    let mut s = Session::new(Project::default());
    s.run("layer.addDataText", &json!({"fileName": "sites.geojson", "text": points(n)})).unwrap();
    s
}

fn saved_text(s: &mut Session) -> String {
    s.run("info.projectText", &Value::Null).unwrap()["text"].as_str().unwrap().to_string()
}

#[test]
fn a_render_from_the_saved_project_matches_the_editors() {
    let mut editor = editor_with(5_000);
    let text = saved_text(&mut editor);
    let mut machine = Session::new(Project::from_json(&text).unwrap());
    let here = map_png(&mut editor, Some(800.0), Some(600.0), None).unwrap();
    let there = map_png(&mut machine, Some(800.0), Some(600.0), None).unwrap();
    assert!(here == there, "the saved project draws another picture ({} and {} bytes)", here.len(), there.len());
    // Drawing is a read: the saved text is what it was.
    assert_eq!(saved_text(&mut editor), text);
}

#[test]
fn a_machine_process_that_starts_again_draws_the_same_map() {
    let mut editor = editor_with(100);
    let text = saved_text(&mut editor);
    // One process per verb: stopping one and starting another loses nothing, because each starts
    // from the text and none writes it.
    let draw = || map_png(&mut Session::new(Project::from_json(&text).unwrap()), Some(640.0), Some(480.0), None).unwrap();
    let (first, second) = (draw(), draw());
    assert!(first == second, "two processes from one saved project drew two pictures");
    assert!(first == map_png(&mut editor, Some(640.0), Some(480.0), None).unwrap());
}

/// The figure #382 asks for. Run at the end of a sprint, on the machine being measured:
/// `cargo test --release -p rustgis-engine --test saved_project_parity -- --ignored --nocapture`
#[test]
#[ignore = "a measurement: a million points, release build"]
fn map_png_of_a_million_points_is_timed() {
    let mut editor = editor_with(1_000_000);
    let text = saved_text(&mut editor);
    let opened = std::time::Instant::now();
    let mut machine = Session::new(Project::from_json(&text).unwrap());
    let open_ms = opened.elapsed().as_millis();
    let drawn = std::time::Instant::now();
    let there = map_png(&mut machine, Some(1600.0), Some(900.0), None).unwrap();
    let draw_ms = drawn.elapsed().as_millis();
    let here = map_png(&mut editor, Some(1600.0), Some(900.0), None).unwrap();
    println!("map_png 1M points 1600x900: open {open_ms} ms, draw {draw_ms} ms, {} bytes, project text {} bytes", there.len(), text.len());
    assert!(here == there, "the saved project draws another picture");
}
