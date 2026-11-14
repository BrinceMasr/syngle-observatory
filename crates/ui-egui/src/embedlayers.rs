//! `ui.layers`: the embedding page's layer list (which layers are shown, and in what order) applied to the map in view,
//! by name, without opening the project again.
//!
//! The page used to send the whole project for a change that only moves a switch or an order. A project opens again
//! from its sources: every streamed layer drops its window and reads the view again, every dataset is fetched by
//! revision, and the first frame after it shows the layer empty. Names are all this command takes, so it cannot read a
//! file or reach an address, which is why the embedding page may run it (`embed::runnable`).

use rustgis_engine::map::Map;
use serde_json::Value;

/// The most layers one command may name: a project file is hostile input and so is a page.
const MOST: usize = 10_000;

/// Applies `{"layers": [name | {"name", "visible"?}, ...]}` to `map`: the layers named come first, in that order, and
/// the ones not named keep their place behind them. A name that is shared goes to the first layer of that name not
/// taken yet. Nothing changes unless every name is found. `true` when the order or a switch changed.
pub(crate) fn arrange(map: &mut Map, p: &Value) -> Result<bool, String> {
    let list = p.get("layers").and_then(Value::as_array).ok_or("layers needs an array")?;
    if list.len() > MOST {
        return Err(format!("layers names {} layers: at most {MOST}", list.len()));
    }
    let mut taken = vec![false; map.layers.len()];
    let mut order: Vec<usize> = Vec::with_capacity(map.layers.len());
    let mut shown: Vec<(usize, bool)> = Vec::new();
    for entry in list {
        let (name, visible) = match entry {
            Value::String(name) => (name.as_str(), None),
            Value::Object(o) => {
                (o.get("name").and_then(Value::as_str).ok_or("a layer needs a name")?, o.get("visible").and_then(Value::as_bool))
            }
            _ => return Err("a layer is a name or {name, visible}".into()),
        };
        let at = map
            .layers
            .iter()
            .enumerate()
            .find(|(i, l)| l.name == name && taken.get(*i) == Some(&false))
            .map(|(i, _)| i)
            .ok_or_else(|| format!("there is no layer '{name}'"))?;
        if let Some(t) = taken.get_mut(at) {
            *t = true;
        }
        order.push(at);
        if let Some(v) = visible {
            shown.push((at, v));
        }
    }
    order.extend(taken.iter().enumerate().filter(|(_, t)| !**t).map(|(i, _)| i));
    let moved = order.iter().enumerate().any(|(to, from)| to != *from);
    let mut changed = moved;
    for (at, v) in shown {
        if let Some(l) = map.layers.get_mut(at)
            && l.visible != v
        {
            l.visible = v;
            changed = true;
        }
    }
    if moved {
        let mut slots: Vec<Option<_>> = std::mem::take(&mut map.layers).into_iter().map(Some).collect();
        map.layers = order.iter().filter_map(|i| slots.get_mut(*i).and_then(Option::take)).collect();
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RustGisApp;
    use serde_json::json;

    fn names(map: &Map) -> Vec<(String, bool)> {
        map.layers.iter().map(|l| (l.name.clone(), l.visible)).collect()
    }

    /// The sample project's session (the app holds it; the layers are `session.map()`'s).
    fn sample() -> RustGisApp {
        RustGisApp::with_sample()
    }

    #[test]
    fn a_switch_and_an_order_apply_by_name() {
        let mut app = sample();
        let s = &mut app.session;
        let all: Vec<String> = s.map().layers.iter().map(|l| l.name.clone()).collect();
        assert!(all.len() >= 3, "the sample has layers: {all:?}");
        let last = all.last().cloned().unwrap_or_default();
        let before = s.map().layers.iter().map(|l| l.id).collect::<Vec<_>>();
        let changed = arrange(s.map_mut(), &json!({"layers": [{"name": last, "visible": false}]})).unwrap();
        assert!(changed);
        let now = names(s.map());
        assert_eq!(now[0], (last.clone(), false), "named layers come first");
        assert_eq!(now.iter().skip(1).map(|n| n.0.clone()).collect::<Vec<_>>(), all[..all.len() - 1], "the rest keep their order behind");
        assert_eq!(
            s.map().layers.iter().map(|l| l.id).collect::<std::collections::HashSet<_>>(),
            before.into_iter().collect(),
            "no layer is lost"
        );
    }

    #[test]
    fn the_same_list_changes_nothing() {
        let mut app = sample();
        let s = &mut app.session;
        let list: Vec<Value> = s.map().layers.iter().map(|l| json!({"name": l.name, "visible": l.visible})).collect();
        let before = names(s.map());
        assert!(!arrange(s.map_mut(), &json!({"layers": list})).unwrap());
        assert_eq!(names(s.map()), before);
    }

    #[test]
    fn an_unknown_name_or_a_bad_list_changes_nothing() {
        let mut app = sample();
        let s = &mut app.session;
        let first = s.map().layers.first().map(|l| l.name.clone()).unwrap_or_default();
        let before = names(s.map());
        for p in [
            json!({"layers": [{"name": first, "visible": false}, "No Such Layer"]}),
            json!({"layers": [first, first]}),
            json!({"layers": "x"}),
            json!({"layers": [1]}),
            json!({"layers": [{"visible": true}]}),
            json!({}),
            Value::Null,
        ] {
            assert!(arrange(s.map_mut(), &p).is_err(), "{p}");
            assert_eq!(names(s.map()), before, "{p}");
        }
    }

    #[test]
    fn a_shared_name_goes_to_each_layer_of_that_name_in_turn() {
        let mut app = sample();
        let s = &mut app.session;
        let first = s.map().layers.first().map(|l| l.name.clone()).unwrap_or_default();
        for l in &mut s.map_mut().layers {
            l.name = first.clone();
        }
        let ids: Vec<u32> = s.map().layers.iter().map(|l| l.id).collect();
        let list: Vec<Value> = ids.iter().map(|_| json!({"name": first, "visible": false})).collect();
        assert!(arrange(s.map_mut(), &json!({"layers": list})).unwrap());
        assert!(s.map().layers.iter().all(|l| !l.visible));
        assert_eq!(s.map().layers.iter().map(|l| l.id).collect::<Vec<_>>(), ids, "each name took the next layer, so the order stays");
    }

    #[test]
    fn too_long_a_list_is_refused() {
        let mut app = sample();
        let s = &mut app.session;
        let list = vec![json!("x"); MOST + 1];
        assert!(arrange(s.map_mut(), &json!({"layers": list})).is_err());
    }
}

#[cfg(test)]
mod command_tests {
    use crate::RustGisApp;
    use serde_json::json;

    #[test]
    fn the_command_changes_the_view_and_is_not_an_edit() {
        let mut app = RustGisApp::with_sample();
        let revision = app.session.revision;
        let undo = app.session.can_undo();
        let first = app.session.map().layers.first().map(|l| l.name.clone()).unwrap_or_default();
        app.run("ui.layers", json!({"layers": [{"name": first, "visible": false}]})).unwrap();
        assert!(app.session.map().layers.first().is_some_and(|l| !l.visible));
        assert!(app.session.revision > revision, "the map draws again");
        assert_eq!(app.session.can_undo(), undo, "a switch the page holds is not an undo step");
        let after = app.session.revision;
        app.run("ui.layers", json!({"layers": [{"name": first, "visible": false}]})).unwrap();
        assert_eq!(app.session.revision, after, "nothing changed, so nothing draws again");
        assert!(app.run("ui.layers", json!({"layers": ["No Such Layer"]})).is_err());
    }
}
