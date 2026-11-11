//! One undo stack per map (Syngle-Ecosystem #472, ADR 0022): Undo in the map in view takes back the last step made in it,
//! never another map's; a row edit is one change to the data in every map that holds it and undoes in all of them.

use super::*;
use serde_json::json;

/// The sample project with a second map that is a copy of the first (so they share every layer's rows), the first in view.
fn two_maps() -> Session {
    crate::tests::isolate_plugins();
    let mut s = Session::new(Project::default());
    s.run("project.sample", &Value::Null).unwrap();
    s.run("map.duplicate", &json!({"name": "Overview"})).unwrap();
    s.run("map.activate", &json!({"map": 1})).unwrap();
    s
}

fn color(s: &Session, map: &str, layer: &str) -> String {
    let m = s.project.maps.iter().find(|m| m.name == map).unwrap();
    format!("{:?}", m.layer_by_name(layer).unwrap().renderer)
}

fn rows(s: &Session, map: &str) -> usize {
    s.project.maps.iter().find(|m| m.name == map).unwrap().layer_by_name("Cities").unwrap().dataset.features.len()
}

#[test]
fn undo_in_one_map_never_changes_another_maps_styles() {
    let mut s = two_maps();
    let first = s.project.maps[0].name.clone();
    s.run("layer.color", &json!({"layer": "Cities", "color": "#112233"})).unwrap();
    let (a_styled, b_plain) = (color(&s, &first, "Cities"), color(&s, "Overview", "Cities"));
    assert_ne!(a_styled, b_plain);
    // A step in the other map, made from the first map's view by naming it.
    s.run("layer.color", &json!({"map": "Overview", "layer": "Cities", "color": "#445566"})).unwrap();
    let b_styled = color(&s, "Overview", "Cities");
    // The copy of the map is a step of the project, on both stacks; each map's own style step is on its own.
    assert_eq!((s.undo_depth(), s.map_history(s.project.maps[1].id).0), (2, 2), "each map has its own step");
    // The map in view undoes its own step, and the other map keeps its style.
    assert!(s.undo());
    assert_eq!(color(&s, "Overview", "Cities"), b_styled);
    assert_ne!(color(&s, &first, "Cities"), a_styled);
    assert_eq!(s.undo_label(), Some("Duplicate Map"), "the map in view is left with the project's step");
    // Redo brings back only that map's step.
    assert!(s.redo());
    assert_eq!(color(&s, &first, "Cities"), a_styled);
    assert_eq!(color(&s, "Overview", "Cities"), b_styled);
    // The other map, once in view, undoes its own.
    s.run("map.activate", &json!({"map": "Overview"})).unwrap();
    assert_eq!(s.undo_depth(), 2);
    assert!(s.undo());
    assert_eq!(color(&s, "Overview", "Cities"), b_plain);
    assert_eq!(color(&s, &first, "Cities"), a_styled, "and the first map's style stays");
}

#[test]
fn a_new_step_in_one_map_does_not_end_another_maps_redo() {
    let mut s = two_maps();
    s.run("layer.color", &json!({"layer": "Cities", "color": "#112233"})).unwrap();
    assert!(s.undo());
    assert_eq!(s.redo_depth(), 1);
    s.run("layer.color", &json!({"map": "Overview", "layer": "Cities", "color": "#445566"})).unwrap();
    assert_eq!(s.redo_depth(), 1, "the map in view can still redo");
    s.run("layer.color", &json!({"layer": "Cities", "color": "#778899"})).unwrap();
    assert_eq!(s.redo_depth(), 0, "its own redo ends with a step of its own");
}

#[test]
fn a_row_edit_is_the_same_data_in_every_map_and_undoes_in_all_of_them() {
    let mut s = two_maps();
    let before = rows(&s, "Overview");
    assert_eq!(rows(&s, &s.project.maps[0].name.clone()), before);
    s.run("edit.addFeature", &json!({"layer": "Cities", "geometry": {"type": "Point", "coordinates": [1.0, 2.0]}})).unwrap();
    let first = s.project.maps[0].name.clone();
    assert_eq!((rows(&s, &first), rows(&s, "Overview")), (before + 1, before + 1), "visible in the other map at once");
    assert!(
        s.project.maps[0]
            .layer_by_name("Cities")
            .unwrap()
            .dataset
            .features
            .shares(&s.project.maps[1].layer_by_name("Cities").unwrap().dataset.features),
        "still one set of rows"
    );
    assert!(s.undo_label_full().unwrap().ends_with(", also in Overview"), "{:?}", s.undo_label_full());
    // The other map's own style, made after the edit, is not undone by it.
    s.run("layer.color", &json!({"map": "Overview", "layer": "Cities", "color": "#445566"})).unwrap();
    let styled = color(&s, "Overview", "Cities");
    assert!(s.undo());
    assert_eq!((rows(&s, &first), rows(&s, "Overview")), (before, before), "undone in both");
    assert_eq!(color(&s, "Overview", "Cities"), styled, "and the style stays");
    assert!(s.redo());
    assert_eq!((rows(&s, &first), rows(&s, "Overview")), (before + 1, before + 1));
}

#[test]
fn a_step_that_changes_the_project_beyond_one_map_is_the_projects() {
    let mut s = two_maps();
    s.run("map.new", &json!({"name": "Third"})).unwrap();
    // Every map has it on its stack: a map was added.
    for m in s.project.maps.clone() {
        assert_eq!(s.map_history(m.id).0, 2, "{} (the copy and the new map)", m.name);
    }
    assert!(s.undo());
    assert_eq!(s.project.maps.len(), 2);
}

#[test]
fn a_hundred_steps_on_a_large_layer_share_its_rows() {
    let mut s = two_maps();
    let layer = s.map().layer_by_name("Cities").unwrap().id;
    let ds = &s.map().layer(layer).unwrap().dataset;
    let rows_before = ds.features.id();
    for i in 0..100 {
        s.run("layer.color", &json!({"layer": "Cities", "color": format!("#{:02x}0000", i)})).unwrap();
    }
    // Every step kept the map it changed, which holds the rows by reference: one allocation however many steps.
    assert!(s.undo.iter().filter(|u| u.owner.is_some()).all(|u| u.project.maps.len() == 1), "a map's own step keeps only that map");
    assert!(
        s.undo
            .iter()
            .filter(|u| u.owner.is_some())
            .all(|u| u.project.maps[0].layer(layer).is_some_and(|l| l.dataset.features.id() == rows_before))
    );
    let own = s.undo.iter().filter(|u| u.owner.is_some()).count();
    assert_eq!(own, UNDO_LIMIT, "the stack keeps its last steps");
}
