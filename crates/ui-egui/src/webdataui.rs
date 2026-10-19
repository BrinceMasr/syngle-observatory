//! Home › Web Data dialogs for services that are a fixed set of addresses: The National Map (its
//! map services by category, each added as tiles) and Fields of the World (a year's field
//! boundaries and density). Both run engine commands (`nationalMap.add`, `fieldsOfTheWorld.add`).

use egui::vec2;
use serde_json::{Value, json};

use crate::RustGisApp;
use crate::theme::Tokens;

/// The National Map: its services by category, filtered by words, each with an Add button.
pub fn national_map(app: &mut RustGisApp, ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(tl!("USGS The National Map's map services (topographic maps, imagery, elevation, hydrography, boundaries and more) and a few partners'. Each is added as a tile layer."))
            .weak(),
    );
    ui.horizontal(|ui| {
        ui.label(tl!("Filter"));
        ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).hint_text(tl!("words in the name")).desired_width(ui.available_width()));
    });
    let catalog = app.session.run("nationalMap.services", &Value::Null).unwrap_or_default();
    let words: Vec<String> = app.dialogs.text.split_whitespace().map(str::to_lowercase).collect();
    let none = Vec::new();
    let services = catalog.get("services").and_then(Value::as_array).unwrap_or(&none);
    let mut add = None;
    // No scroll area of its own: in the Web Data pane, the pane scrolls.
    {
        let categories = catalog.get("categories").and_then(Value::as_array).unwrap_or(&none);
        for category in categories.iter().filter_map(Value::as_str) {
            let shown: Vec<&Value> = services
                .iter()
                .filter(|s| s.get("category").and_then(Value::as_str) == Some(category))
                .filter(|s| {
                    let hay = format!(
                        "{} {} {}",
                        s.get("title").and_then(Value::as_str).unwrap_or(""),
                        s.get("id").and_then(Value::as_str).unwrap_or(""),
                        category
                    )
                    .to_lowercase();
                    words.iter().all(|w| hay.contains(w.as_str()))
                })
                .collect();
            if shown.is_empty() {
                continue;
            }
            ui.label(egui::RichText::new(tl!(category)).strong());
            for s in shown {
                let id = s.get("id").and_then(Value::as_str).unwrap_or("");
                let title = s.get("title").and_then(Value::as_str).unwrap_or(id);
                ui.horizontal(|ui| {
                    if ui.small_button(tl!("Add")).clicked() {
                        add = Some((id.to_string(), title.to_string()));
                    }
                    ui.label(title).on_hover_text(s.get("url").and_then(Value::as_str).unwrap_or(""));
                });
            }
        }
    }
    if let Some((id, title)) = add {
        app.dialogs.error = None;
        match app.run("nationalMap.add", json!({"service": id})) {
            Ok(_) => {
                app.dialogs.info = Some(crate::i18n::fmt("Added {name}", &[("name", &title)]));
                app.map.invalidate();
            }
            Err(e) => app.dialogs.error = Some(e),
        }
    }
}

/// Fields of the World: a year, which layers, and the lowest score shown.
pub fn fields_of_the_world(app: &mut RustGisApp, ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(tl!("Agricultural field boundaries the Fields of the World model found in Sentinel-2 imagery, worldwide, a year at a time (2nd Edition, CC BY 4.0). Fields are colored by the model's score and drawn from zoom 9; zoomed out, the share of each cell covered by fields."))
            .weak(),
    );
    const LAYERS: [(&str, &str); 3] = [("both", "Fields and field density"), ("fields", "Field boundaries"), ("density", "Field density")];
    let years = rustgis_engine::cmd::webdata::FIELDS_YEARS;
    egui::Grid::new("ftw").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label(tl!("Year"));
        egui::ComboBox::from_id_salt("ftw_year").selected_text(app.dialogs.a.clone()).show_ui(ui, |ui| {
            for y in years.clone().rev() {
                ui.selectable_value(&mut app.dialogs.a, y.to_string(), y.to_string());
            }
        });
        ui.end_row();
        ui.label(tl!("Layers"));
        let shown = LAYERS.iter().find(|(c, _)| *c == app.dialogs.mode).map(|(_, l)| tl!(*l).to_string()).unwrap_or_default();
        egui::ComboBox::from_id_salt("ftw_layers").selected_text(shown).show_ui(ui, |ui| {
            for (code, label) in LAYERS {
                ui.selectable_value(&mut app.dialogs.mode, code, tl!(label));
            }
        });
        ui.end_row();
        ui.label(tl!("Lowest score"));
        ui.add(egui::Slider::new(&mut app.dialogs.size, 0.0..=100.0).step_by(1.0));
        ui.end_row();
    });
    let t = Tokens::get(ui.ctx());
    if crate::widgets::dialog_button(ui, tl!("Add"), Some(t.accent), vec2(96.0, 28.0)).clicked() {
        app.dialogs.error = None;
        let p = json!({"year": app.dialogs.a, "draw": app.dialogs.mode, "minScore": app.dialogs.size});
        match app.run("fieldsOfTheWorld.add", p) {
            Ok(v) => {
                let name = v.get("name").and_then(Value::as_str).unwrap_or("");
                let mut info = crate::i18n::fmt("Added {name}", &[("name", name)]);
                if v.get("note").is_some() {
                    info.push_str(". ");
                    info.push_str(tl!("Fields are drawn from zoom 9: zoom in to see them."));
                }
                app.dialogs.info = Some(info);
                app.map.invalidate();
            }
            Err(e) => app.dialogs.error = Some(e),
        }
    }
}
