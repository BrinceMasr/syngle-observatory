//! Search Open Data and the Web Data services built on it: datasets found by text or where the map
//! is, in a CKAN catalogue (data.gov's by default), on Socrata or ArcGIS Hub, or in one service
//! (ArcGIS Online, Earthdata GIS, OpenAerialMap, USGS 3DEP, IGN LiDAR HD, GeoLens, Hugging Face,
//! the Ocean Data Platform, US agencies', states' and local governments' portals), and their
//! resources added with a click. The search goes off the frame through the tile cache's downloads
//! (a thread natively, fetch on the web); a file is added by its address through `ui.addUrl`, a
//! service opens Add From Service connected to it.

use egui::vec2;
use rustgis_engine::render::rustgis_tiles::cache;
use rustgis_engine::render::rustgis_tiles::portal::{DEM_DATASETS, Dataset, Portal, Use};
use rustgis_engine::render::rustgis_tiles::usgis::{self, Search, Set};
use serde_json::json;

use crate::{RustGisApp, Slot};

type Answer = Slot<Result<Vec<u8>, String>>;

/// The portals Search Open Data offers: the kind, its name, and what its address field takes.
const PORTALS: [(&str, &str, &str); 3] = [
    ("ckan", "data.gov and CKAN catalogues", "catalog.data.gov, or a CKAN site such as https://open.canada.ca/data"),
    ("socrata", "Socrata", "every Socrata site, or one such as data.cityofnewyork.us"),
    ("arcgis-hub", "ArcGIS Hub", "hub.arcgis.com, or an organisation's Hub site"),
];

/// A Web Data service the dialog opens on: its id, its title, the portal searched, what its
/// address field takes (none: no field), and the US portals it picks from.
pub struct Service {
    pub id: &'static str,
    pub title: &'static str,
    portal: &'static str,
    address: Option<&'static str>,
    sets: Option<&'static [Set]>,
}

/// The services.
pub const SERVICES: [Service; 11] = [
    Service {
        id: "arcgis-portal",
        title: "ArcGIS Portal",
        portal: "arcgis-portal",
        address: Some("ArcGIS Online, or a portal such as https://gis.example.org/portal"),
        sets: None,
    },
    Service { id: "earthdata-gis", title: "Earthdata GIS", portal: "earthdata-gis", address: None, sets: None },
    Service {
        id: "geolens",
        title: "GeoLens",
        portal: "geolens",
        address: Some("https://datasets.geolibre.app, or another GeoLens server"),
        sets: None,
    },
    Service { id: "huggingface", title: "Hugging Face", portal: "huggingface", address: None, sets: None },
    Service { id: "ocean-data", title: "Ocean Data Platform", portal: "ocean-data", address: None, sets: None },
    Service { id: "openaerialmap", title: "OpenAerialMap", portal: "openaerialmap", address: None, sets: None },
    Service { id: "usgs-dem", title: "USGS 3DEP DEM", portal: "usgs-dem", address: None, sets: None },
    Service { id: "ign-lidar", title: "IGN LiDAR HD", portal: "ign-lidar", address: None, sets: None },
    Service { id: "us-federal", title: "US Federal GIS", portal: "arcgis-portal", address: None, sets: Some(usgis::FEDERAL) },
    Service { id: "us-state", title: "US State GIS", portal: "arcgis-portal", address: None, sets: Some(usgis::STATE) },
    Service { id: "us-local", title: "US Local GIS", portal: "arcgis-portal", address: None, sets: Some(usgis::LOCAL) },
];

fn service(id: &str) -> Option<&'static Service> {
    SERVICES.iter().find(|s| s.id == id)
}

/// The dialog's choices, the search under way and what it found.
pub struct State {
    /// The Web Data service the dialog is on, or none (Search Open Data, any portal).
    pub service: Option<&'static str>,
    pub portal: &'static str,
    pub url: String,
    pub query: String,
    /// Only datasets where the map is.
    pub here: bool,
    /// The US portal picked: its set and its place in it.
    pub set: usize,
    pub catalog: usize,
    /// The 3DEP elevation model searched (in [`DEM_DATASETS`]).
    pub dem: usize,
    /// The search under way: the portal, its address, the box it keeps to, its words and what it
    /// is on (the US portal picked when it began).
    waiting: Option<(Portal, String, Option<[f64; 4]>, String, String, Answer)>,
    /// A dataset's files being listed: the dataset (its place in `found`), the portal, the
    /// listing's address.
    listing: Option<(usize, Portal, String, Answer)>,
    pub found: Vec<Dataset>,
    pub error: Option<String>,
    pub info: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            service: None,
            portal: "ckan",
            url: String::new(),
            query: String::new(),
            here: false,
            set: 0,
            catalog: 0,
            dem: 2,
            waiting: None,
            listing: None,
            found: Vec::new(),
            error: None,
            info: None,
        }
    }
}

impl State {
    /// The dialog's title: the service's, or Search Open Data's.
    pub fn title(&self) -> &'static str {
        self.service.and_then(service).map(|s| s.title).unwrap_or("Search Open Data")
    }

    /// What the search was on, in its count: the US portal picked, the service, or the portal.
    fn source(&self, portal: &Portal) -> String {
        let svc = self.service.and_then(service);
        let picked = svc.and_then(|s| s.sets).and_then(|sets| sets.get(self.set)).and_then(|s| s.catalogs.get(self.catalog));
        match (picked, svc) {
            (Some(c), _) => c.name.to_string(),
            (None, Some(s)) if s.address.is_none() || self.url.trim().is_empty() => tl!(s.title).to_string(),
            _ => portal.label(),
        }
    }
}

/// Lands the search's answer, and a dataset's listing.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    poll_listing(app, ctx);
    let Some((_, _, _, _, _, slot)) = &app.ui.opendata.waiting else { return };
    let Some(r) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let Some((portal, url, area, query, source, _)) = app.ui.opendata.waiting.take() else { return };
    let st = &mut app.ui.opendata;
    match r.and_then(|b| portal.parse(&url, &b, area, &query)) {
        Ok(found) => {
            st.info = Some(if found.is_empty() {
                tl!("No datasets with data RustGIS can add").to_string()
            } else {
                crate::i18n::fmt("{n} datasets on {portal}", &[("n", &found.len().to_string()), ("portal", &source)])
            });
            st.found = found;
        }
        Err(e) => st.error = Some(e),
    }
}

fn poll_listing(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some((_, _, _, slot)) = &app.ui.opendata.listing else { return };
    let Some(r) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let Some((i, portal, url, _)) = app.ui.opendata.listing.take() else { return };
    let st = &mut app.ui.opendata;
    match r.and_then(|b| portal.parse_listing(&url, &b)) {
        Ok(files) => {
            if files.is_empty() {
                st.info = Some(tl!("The dataset has no files RustGIS can add").to_string());
            }
            if let Some(d) = st.found.get_mut(i) {
                d.resources = files;
                d.listing = None;
            }
        }
        Err(e) => st.error = Some(e),
    }
}

/// Fills the dialog from `ui.searchOpenData`'s parameters (`service`, `portal`, `url`, `query`,
/// `view`) and, with a query, starts the search. A service opens the dialog on it alone.
pub fn start(app: &mut RustGisApp, p: &serde_json::Value) {
    let text = |k: &str| p.get(k).and_then(serde_json::Value::as_str).map(str::trim);
    let svc = text("service").and_then(service);
    let st = &mut app.ui.opendata;
    if st.service != svc.map(|s| s.id) {
        // Another service: its own choices.
        *st = State { service: svc.map(|s| s.id), ..State::default() };
        if let Some(s) = svc {
            st.portal = s.portal;
            // A search by place is where the map is.
            st.here = Portal::new(Some(s.portal), None).is_ok_and(|p| p.needs_place());
        }
    }
    if svc.is_none()
        && let Some(kind) = text("portal")
    {
        st.portal = match kind {
            "socrata" => "socrata",
            "arcgis-hub" | "hub" | "arcgis" => "arcgis-hub",
            _ => "ckan",
        };
    }
    if let Some(u) = text("url") {
        st.url = u.to_string();
    }
    if let Some(v) = p.get("view").and_then(serde_json::Value::as_bool) {
        // A search by place is always where the map is.
        st.here = v || Portal::new(Some(st.portal), None).is_ok_and(|p| p.needs_place());
    }
    if let Some(q) = text("query").filter(|q| !q.is_empty()) {
        st.query = q.to_string();
        if st.waiting.is_none() {
            st.error = None;
            st.info = None;
            search(app);
        }
    }
}

/// The search's parameters, as `portal.search` takes them.
fn params(st: &State) -> serde_json::Value {
    let mut p = json!({"portal": st.portal, "query": st.query.trim(), "view": st.here});
    if !st.url.trim().is_empty() {
        p["url"] = json!(st.url.trim());
    }
    let svc = st.service.and_then(service);
    if let Some(sets) = svc.and_then(|s| s.sets)
        && let Some(c) = sets.get(st.set).and_then(|s| s.catalogs.get(st.catalog))
    {
        match c.search {
            Search::ArcGis(org) => p["org"] = json!(org),
            Search::Socrata(domain) => {
                p["portal"] = json!("socrata");
                p["url"] = json!(domain);
            }
        }
    }
    if svc.is_some_and(|s| s.id == "usgs-dem")
        && let Some((label, _)) = DEM_DATASETS.get(st.dem)
    {
        p["dataset"] = json!(label);
    }
    p
}

/// Starts a search for the dialog's choices.
fn search(app: &mut RustGisApp) {
    let p = params(&app.ui.opendata);
    match rustgis_engine::cmd::portals::portal_query(&app.session, &p) {
        Ok((portal, query, area, limit)) => {
            let url = portal.search_url(&query, area, limit);
            let slot: Answer = Slot::default();
            let land = slot.clone();
            cache::download(&url, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
            let st = &mut app.ui.opendata;
            st.found.clear();
            st.listing = None;
            let source = st.source(&portal);
            st.waiting = Some((portal, url, area, query, source, slot));
        }
        Err(e) => app.ui.opendata.error = Some(e),
    }
}

/// Starts listing a dataset's files.
fn list_files(app: &mut RustGisApp, i: usize) {
    let st = &app.ui.opendata;
    let Some(url) = st.found.get(i).and_then(|d| d.listing.clone()) else { return };
    match Portal::new(Some(st.portal), None) {
        Ok(portal) => {
            let slot: Answer = Slot::default();
            let land = slot.clone();
            cache::download(&url, Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)));
            app.ui.opendata.listing = Some((i, portal, url, slot));
        }
        Err(e) => app.ui.opendata.error = Some(e),
    }
}

/// What the dialog says the service is.
fn about(id: Option<&str>) -> &'static str {
    match id {
        Some("arcgis-portal") => {
            "Public layers on ArcGIS Online or another ArcGIS portal: feature, map, image and vector tile services, and GeoJSON files."
        }
        Some("earthdata-gis") => "NASA's Earthdata GIS: imagery, maps and features of Earth science data as ArcGIS services.",
        Some("geolens") => "A GeoLens server's datasets, read as features (the first 1,000 of each).",
        Some("huggingface") => {
            "Datasets on Hugging Face by name or owner/name; browse a dataset's files and add GeoTIFFs, GeoParquet, PMTiles, LiDAR and other files."
        }
        Some("ocean-data") => "HUB Ocean's Ocean Data Platform: marine datasets, read as features (up to 10,000 each).",
        Some("openaerialmap") => "Drone and aerial imagery shared on OpenAerialMap, newest first, added as cloud-optimised GeoTIFFs.",
        Some("usgs-dem") => "USGS 3DEP elevation models (GeoTIFF tiles from The National Map) where the map is.",
        Some("ign-lidar") => "IGN's LiDAR HD point clouds of France: 1 km tiles where the map is, added as COPC point clouds.",
        Some("us-federal") => "Public GIS portals of US federal agencies, by department.",
        Some("us-state") => "Public GIS portals of US states and the District of Columbia.",
        Some("us-local") => "Public GIS portals of US counties and cities, by state.",
        _ => {
            "Open data catalogues searched by text. Files are added by their address (read in place when they can be); a service opens Add From Service to pick its layer."
        }
    }
}

/// The dialog: a portal (or the service's catalog), a search, and the datasets found with their
/// resources.
pub fn dialog(app: &mut RustGisApp, ui: &mut egui::Ui) {
    let svc = app.ui.opendata.service.and_then(service);
    ui.label(egui::RichText::new(tl!(about(svc.map(|s| s.id)))).weak());
    let busy = app.ui.opendata.waiting.is_some();
    let mut go = false;
    // The fields fill the pane, beside their labels.
    let w = (ui.available_width() - 120.0).clamp(160.0, 320.0);
    egui::Grid::new("opendata").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        let st = &mut app.ui.opendata;
        match svc {
            None => {
                ui.label(tl!("Portal"));
                let shown = PORTALS.iter().find(|(k, _, _)| *k == st.portal).map(|(_, n, _)| tl!(*n).to_string()).unwrap_or_default();
                egui::ComboBox::from_id_salt("od_portal").selected_text(shown).width(w).show_ui(ui, |ui| {
                    for (kind, name, _) in PORTALS {
                        if ui.selectable_label(st.portal == kind, tl!(name)).clicked() && st.portal != kind {
                            st.portal = kind;
                            st.url.clear();
                        }
                    }
                });
                ui.end_row();
                ui.label(tl!("Address"));
                let hint = PORTALS.iter().find(|(k, _, _)| *k == st.portal).map(|(_, _, h)| tl!(*h)).unwrap_or_default();
                ui.add(egui::TextEdit::singleline(&mut st.url).hint_text(hint).desired_width(w));
                ui.end_row();
            }
            Some(s) => {
                if let Some(hint) = s.address {
                    ui.label(tl!("Address"));
                    ui.add(egui::TextEdit::singleline(&mut st.url).hint_text(tl!(hint)).desired_width(w));
                    ui.end_row();
                }
                if let Some(sets) = s.sets {
                    catalog_picker(ui, st, sets, w);
                }
                if s.id == "usgs-dem" {
                    ui.label(tl!("Elevation model"));
                    let shown = DEM_DATASETS.get(st.dem).map(|(l, _)| tl!(*l).to_string()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("od_dem").selected_text(shown).width(w).show_ui(ui, |ui| {
                        for (i, (label, _)) in DEM_DATASETS.iter().enumerate() {
                            ui.selectable_value(&mut st.dem, i, tl!(*label));
                        }
                    });
                    ui.end_row();
                }
            }
        }
        let place = Portal::new(Some(st.portal), None).is_ok_and(|p| p.needs_place());
        ui.label(tl!("Search"));
        // A Socrata site among the US portals is searched by words only.
        let needs = {
            let p = params(st);
            Portal::new(p["portal"].as_str(), p["url"].as_str()).is_ok_and(|p| p.needs_query())
        };
        ui.horizontal(|ui| {
            let hint = match svc.map(|s| s.id) {
                Some("huggingface") => tl!("a name, or owner/name such as giswqs/geospatial"),
                _ if place => tl!("words to keep to (optional)"),
                Some(_) if needs => tl!("words (this portal needs some)"),
                Some(_) => tl!("words (optional)"),
                None => tl!("parks, bike lanes, flood zones…"),
            };
            let r = ui.add(egui::TextEdit::singleline(&mut st.query).hint_text(hint).desired_width(w - 100.0));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            go = (ui.add_enabled(!busy, egui::Button::new(tl!("Find Datasets"))).clicked() || enter) && !busy;
        });
        ui.end_row();
        ui.label("");
        // A search by place is always where the map is.
        ui.add_enabled(!place, egui::Checkbox::new(&mut st.here, tl!("Only where the map is")));
        if place {
            st.here = true;
        }
        ui.end_row();
    });
    if go {
        app.ui.opendata.error = None;
        app.ui.opendata.info = None;
        search(app);
    }
    if busy || app.ui.opendata.listing.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(if busy { tl!("Searching…") } else { tl!("Listing files…") });
        });
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    if let Some(e) = &app.ui.opendata.error {
        ui.label(egui::RichText::new(e).color(t.red));
    }
    if let Some(i) = &app.ui.opendata.info {
        ui.label(egui::RichText::new(i).weak());
    }
    // A resource clicked: its address, how it is used, and the dataset's title.
    let mut picked = None;
    let mut browse = None;
    let listing = app.ui.opendata.listing.is_some();
    // The pane scrolls the list.
    ui.vertical(|ui| {
        for (i, d) in app.ui.opendata.found.iter().enumerate() {
            ui.push_id(i, |ui| {
                ui.label(egui::RichText::new(&d.title).strong());
                if !d.organization.is_empty() && d.organization != d.title {
                    ui.label(egui::RichText::new(&d.organization).small());
                }
                if !d.description.is_empty() {
                    ui.label(egui::RichText::new(&d.description).small().weak());
                }
                ui.horizontal_wrapped(|ui| {
                    if d.listing.is_some() && ui.add_enabled(!busy && !listing, egui::Button::new(tl!("Browse Files")).small()).clicked() {
                        browse = Some(i);
                    }
                    for r in &d.resources {
                        let label = crate::i18n::fmt("Add {format}", &[("format", &r.format)]);
                        if ui.add_enabled(!busy, egui::Button::new(label).small()).on_hover_text(&r.url).clicked() {
                            // A file of a dataset is named after the file.
                            let name = if d.listing.is_none() && d.resources.len() > 1 && r.format.contains('.') {
                                r.format.rsplit('/').next().unwrap_or(&r.format).to_string()
                            } else {
                                d.title.clone()
                            };
                            picked = Some((r.url.clone(), r.using.clone(), name));
                        }
                    }
                    if let Some(page) = &d.page {
                        ui.hyperlink_to(tl!("Page"), page);
                    }
                });
                ui.separator();
            });
        }
    });
    if let Some(i) = browse {
        app.ui.opendata.error = None;
        list_files(app, i);
    }
    if let Some((url, using, title)) = picked {
        app.ui.opendata.error = None;
        let r = match using {
            Use::File => app.run("ui.addUrl", json!({"url": url, "name": title})).map(|_| ()),
            // A service: Add From Service, connected, to pick its layer.
            Use::Service { kind, .. } => app
                .run("ui.dialog", json!({"name": "addService"}))
                .and_then(|_| app.run("ui.addService", json!({"url": url, "type": kind})))
                .map(|_| ()),
        };
        if let Err(e) = r {
            app.ui.opendata.error = Some(e);
        }
    }
}

/// The US portal picker: a set (department or state), then its portal.
fn catalog_picker(ui: &mut egui::Ui, st: &mut State, sets: &'static [Set], w: f32) {
    ui.label(tl!("Group"));
    let shown = sets.get(st.set).map(|s| s.name).unwrap_or_default();
    egui::ComboBox::from_id_salt("od_set").selected_text(shown).width(w).height(400.0).show_ui(ui, |ui| {
        for (i, s) in sets.iter().enumerate() {
            if ui.selectable_label(st.set == i, s.name).clicked() && st.set != i {
                st.set = i;
                st.catalog = 0;
            }
        }
    });
    ui.end_row();
    ui.label(tl!("Portal"));
    let catalogs = sets.get(st.set).map(|s| s.catalogs).unwrap_or_default();
    let shown = catalogs.get(st.catalog).map(|c| c.name).unwrap_or_default();
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("od_catalog").selected_text(shown).width(w - 44.0).height(400.0).show_ui(ui, |ui| {
            for (i, c) in catalogs.iter().enumerate() {
                ui.selectable_value(&mut st.catalog, i, c.name);
            }
        });
        if let Some(c) = catalogs.get(st.catalog) {
            ui.hyperlink_to(tl!("Open"), c.url);
        }
    });
    ui.end_row();
}
