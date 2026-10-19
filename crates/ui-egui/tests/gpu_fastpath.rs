//! The GPU camera fast path must yield one full re-render once the camera settles:
//! the scene's contents depend on the render-time camera (extent culling, the 0.35 px
//! decimation, label placement). Regression test for the PR #309 review (coderabbit +
//! github-actions, both flagging the missing settle render).
//! Planetary tile scenes also reuse uploaded images and preview layer changes immediately.

use rustgis_ui_egui::RustGisApp;
use serde_json::{Value, json};

fn wait_for_frame(h: &mut egui_kittest::Harness<'_, RustGisApp>) {
    for _ in 0..40 {
        h.step();
    }
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().map.drawing() {
        assert!(std::time::Instant::now() < end, "frame worker did not finish");
        std::thread::sleep(std::time::Duration::from_millis(2));
        h.step();
    }
}

fn serve_planet_tiles() {
    use rustgis_ui_egui::rustgis_engine::render::rustgis_tiles;
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_fn(256, 256, |x, y| image::Rgba([100 + (x / 2) as u8, 30 + (y / 8) as u8, 50, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("tile");
    let mut sharp = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_fn(256, 256, |x, y| {
        let c = if (x + y) % 2 == 0 { 240 } else { 20 };
        image::Rgba([c, c, c, 255])
    })
    .write_to(&mut sharp, image::ImageFormat::Png)
    .expect("sharp tile");
    rustgis_tiles::cache::set_fetcher(move |url, done| {
        done(Ok(if url.contains("quality-wms.test") { sharp.get_ref().clone() } else { png.get_ref().clone() }))
    });
}

/// A wgpu harness sharing one render state with the app (the ui_shot pattern), so the GPU
/// backend engages. `None` on hosts without an adapter.
fn harness() -> Option<egui_kittest::Harness<'static, RustGisApp>> {
    harness_at_density(1.0)
}

fn harness_at_density(ppp: f32) -> Option<egui_kittest::Harness<'static, RustGisApp>> {
    let setup = egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew::without_display_handle());
    let instance = pollster::block_on(setup.new_instance());
    let rs = pollster::block_on(egui_wgpu::RenderState::create(
        &egui_wgpu::WgpuConfiguration { wgpu_setup: setup, ..Default::default() },
        &instance,
        None,
        egui_wgpu::RendererOptions::PREDICTABLE,
    ))
    .ok()?;
    let mut app = RustGisApp::with_sample();
    app.set_render_state(Some(rs.clone()));
    app.run("ui.map.gpu", json!({"on": true})).expect("gpu on");
    Some(
        egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 550.0))
            .with_pixels_per_point(ppp)
            .with_max_steps(10_000)
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs))
            .build_ui_state(
                |ui, app: &mut RustGisApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            ),
    )
}

#[test]
fn settling_after_a_camera_animation_re_renders_fully() {
    let Some(mut h) = harness() else { return };
    for _ in 0..6 {
        h.step();
    }
    let app = h.state_mut();
    app.run("animation.add", Value::Null).expect("keyframe 1");
    app.run("map.setScale", json!({"scale": 20_000_000.0})).expect("zoom keyframe");
    app.run("animation.add", Value::Null).expect("keyframe 2");
    app.run("ui.animationPlay", Value::Null).expect("play");
    // Pump frames until the animation settles (playing is None again), then a few more.
    let mut settled = false;
    for _ in 0..9_000 {
        h.step();
        if h.state().ws.playing.is_none() {
            settled = true;
            break;
        }
    }
    assert!(settled, "the animation finished");
    for _ in 0..5 {
        h.step();
    }
    let app = h.state();
    let (cam, tex_cam) = (app.session.map().camera, app.map.full_frame_camera());
    assert_eq!((tex_cam.center, tex_cam.res), (cam.center, cam.res), "a full re-render followed the settled camera (tex_camera refreshed)");
}

/// Compare the displayed GPU picture with the exact ground under the current camera.
fn assert_gpu_picture_matches_cpu(h: &mut egui_kittest::Harness<'_, RustGisApp>) {
    assert!(h.output().shapes.iter().any(|s| matches!(s.shape, egui::Shape::Callback(_))), "the GPU backend engaged");
    // Exercise the actual blit shader as well: a shifted/scaled image must sample the same
    // ground as an exact CPU render, even when its rectangle extends past the window.
    let image = h.render().expect("moving GPU image");
    let rect = h.state().map.view_rect.expect("map rectangle").0;
    let view = rustgis_ui_egui::rustgis_engine::render::View {
        width: rect.width() as f64,
        height: rect.height() as f64,
        pixels_per_point: 1.0,
        camera: h.state().session.map().camera,
        sky: true,
    };
    let expected = rustgis_ui_egui::rustgis_engine::render::render(&mut h.state().session.map().clone(), &view).expect("CPU reference");
    let mut error = 0u32;
    let mut samples = 0u32;
    for y in (rect.height() as u32 / 3..rect.height() as u32 * 2 / 3).step_by(11) {
        for x in (rect.width() as u32 / 3..rect.width() as u32 * 2 / 3).step_by(11) {
            let actual = image.get_pixel(rect.min.x.round() as u32 + x, rect.min.y.round() as u32 + y);
            let expected = expected.pixmap.pixel(x, y).expect("reference pixel");
            error += actual[0].abs_diff(expected.red()) as u32 + actual[1].abs_diff(expected.green()) as u32;
            samples += 2;
        }
    }
    assert!(error < samples * 6, "cached GPU image moved to the wrong ground: mean color error {}", error / samples);
}

#[test]
fn zooming_a_planet_with_raster_tiles_reuses_the_gpu_frame() {
    use rustgis_ui_egui::rustgis_engine::map::{TileKind, TileLayer};
    serve_planet_tiles();
    let Some(mut h) = harness() else { return };
    h.state_mut().run("map.body", json!({"name": "moon", "basemap": false})).expect("Moon");
    h.state_mut().session.map_mut().layers.clear();
    h.state_mut().session.map_mut().graticule = false;
    // Keep both pictures at the same tile level: this test compares geometry, while the
    // exact render can otherwise choose finer tiles than the cached moving picture.
    let mut tiles = TileLayer::new("Moon", TileKind::Raster, "https://planet.test/{z}/{x}/{y}.png");
    tiles.min_zoom = 3;
    tiles.max_zoom = 3;
    h.state_mut().session.map_mut().tiles.push(tiles);
    wait_for_frame(&mut h);
    assert!(h.output().shapes.iter().any(|s| matches!(s.shape, egui::Shape::Callback(_))), "the GPU backend engaged");
    let before = h.state().map.full_frame_camera();
    for _ in 0..8 {
        h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(600.0, 350.0)));
        h.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 2.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        });
        h.step();
    }
    assert_ne!(h.state().session.map().camera.res, before.res, "zoom engaged");
    assert_eq!(h.state().map.full_frame_camera(), before, "navigation must reuse the uploaded image instead of redrawing every frame");
    assert_gpu_picture_matches_cpu(&mut h);
    wait_for_frame(&mut h);
    assert_eq!(h.state().map.full_frame_camera(), h.state().session.map().camera, "settling sharpens the image");
    // Start from a CPU preview, then force the first GPU frame to fill a pan gap. Its
    // oversized picture must use cropped UVs immediately, including this very first frame.
    h.state_mut().map.invalidate();
    let from = egui::pos2(600.0, 350.0);
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.step();
    h.state_mut().map.invalidate();
    h.input_mut().events.push(egui::Event::PointerMoved(from + egui::vec2(16.0, 0.0)));
    h.step();
    assert_gpu_picture_matches_cpu(&mut h);
}

#[test]
fn planet_layer_toggles_show_in_the_next_frame_and_stale_jobs_do_not_restore_them() {
    use rustgis_ui_egui::rustgis_engine::{
        data::{GeometryType, io::read_geojson},
        map::{Renderer, Rgba, Symbol, TileKind, TileLayer},
    };
    serve_planet_tiles();
    let Some(mut h) = harness() else { return };
    h.state_mut().run("map.body", json!({"name": "moon", "basemap": false})).expect("Moon");
    let m = h.state_mut().session.map_mut();
    m.layers.clear();
    m.graticule = false;
    m.tiles.push(TileLayer::new("Moon", TileKind::Raster, "https://planet.test/{z}/{x}/{y}.png"));
    let (data, _) = read_geojson(r#"{"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[-30,-20],[50,-20],[50,55],[-30,55],[-30,-20]]]}}"#, "Overlay").expect("overlay");
    let id = m.add_dataset(data);
    m.layer_mut(id).expect("layer").renderer =
        Renderer::Single(Symbol { fill: Rgba::rgb(20, 220, 20), ..Symbol::default_for(GeometryType::Polygon, 0) });
    for projection in ["globe", "mercator"] {
        for gpu in [false, true] {
            h.state_mut().run("map.projection", json!({"type": projection})).expect("projection");
            h.state_mut().run("ui.map.gpu", json!({"on": gpu})).expect("backend");
            wait_for_frame(&mut h);
            for visible in [true, false, true, false] {
                h.state_mut().run("layer.visibility", json!({"layer": id, "visible": visible})).expect("toggle");
                h.step();
                assert!(h.state().map.drawing(), "exact drawing runs off the UI thread");
                let image = h.render().expect("visibility preview");
                let center = h.state().map.view_rect.expect("map").0.center();
                let p = image.get_pixel(center.x.round() as u32, center.y.round() as u32);
                assert_eq!(
                    u16::from(p[1]) > u16::from(p[0]) * 2,
                    visible,
                    "{projection} GPU={gpu} visibility did not update immediately: {p:?}"
                );
            }
            wait_for_frame(&mut h);
            let image = h.render().expect("settled frame");
            let center = h.state().map.view_rect.expect("map").0.center();
            let p = image.get_pixel(center.x.round() as u32, center.y.round() as u32);
            assert!(p[0] > p[1], "an old worker restored a hidden overlay: {p:?}");
        }
    }
}

#[test]
fn tiled_camera_playback_does_not_start_settling_jobs() {
    use rustgis_ui_egui::rustgis_engine::map::{TileKind, TileLayer};
    serve_planet_tiles();
    for gpu in [false, true] {
        let Some(mut h) = harness() else { return };
        h.state_mut().run("ui.map.gpu", json!({"on": gpu})).expect("backend");
        h.state_mut().run("map.body", json!({"name": "moon", "basemap": false})).expect("Moon");
        h.state_mut().session.map_mut().layers.clear();
        h.state_mut().session.map_mut().tiles.push(TileLayer::new("Moon", TileKind::Raster, "https://planet.test/{z}/{x}/{y}.png"));
        wait_for_frame(&mut h);
        let start = h.state().session.map().camera;
        h.state_mut().run("animation.add", Value::Null).expect("first frame");
        h.state_mut().run("map.setScale", json!({"scale": 20_000_000.0})).expect("zoom");
        h.state_mut().run("animation.add", Value::Null).expect("second frame");
        h.state_mut().run("ui.animationPlay", Value::Null).expect("play");
        for _ in 0..4 {
            h.step();
            assert!(h.state().ws.playing.is_some(), "animation is playing");
            assert!(!h.state().map.drawing(), "playback must render the current frame, not start a stale settling job");
            assert_eq!(h.state().map.full_frame_camera(), h.state().session.map().camera);
        }
        assert_ne!(h.state().session.map().camera.res, start.res, "playback moved the camera");
    }
}

#[test]
fn dragging_a_planet_globe_keeps_point_symbols_their_settled_size() {
    use rustgis_ui_egui::rustgis_engine::{
        data::{GeometryType, io::read_geojson},
        geom::{Coord, lonlat_to_merc},
        map::{Camera, Projection, Renderer, Rgba, Symbol},
    };
    for (body, lat, ppp) in [("moon", 0.0, 1.0), ("moon", 0.0, 2.0), ("mars", 45.0, 1.0), ("mars", 45.0, 2.0), ("earth", 60.0, 2.0)] {
        for gpu in [false, true] {
            let Some(mut h) = harness_at_density(ppp) else { return };
            h.state_mut().run("ui.map.gpu", json!({"on": gpu})).expect("backend");
            h.state_mut().run("map.body", json!({"name": body, "basemap": false})).expect("body");
            let m = h.state_mut().session.map_mut();
            m.layers.clear();
            m.tiles.clear();
            m.graticule = false;
            m.projection = Projection::Globe;
            m.camera = Camera { center: lonlat_to_merc(Coord::new(0.0, lat)), res: 5_000.0, bearing: 0.0, pitch: 0.0 };
            let geojson = json!({"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [0.0, lat]}});
            let (data, _) = read_geojson(&geojson.to_string(), "Point").expect("point");
            let id = m.add_dataset(data);
            m.layer_mut(id).expect("layer").renderer = Renderer::Single(Symbol {
                fill: Rgba::rgb(20, 240, 20),
                stroke_width: 0.0,
                size: 16.0,
                ..Symbol::default_for(GeometryType::Point, 0)
            });
            wait_for_frame(&mut h);
            let point_width = |h: &mut egui_kittest::Harness<'_, RustGisApp>| {
                let rect = h.state().map.view_rect.expect("map").0;
                let image = h.render().expect("point screenshot");
                let mut xs = Vec::new();
                for y in (rect.top() * ppp).ceil() as u32..(rect.bottom() * ppp).floor() as u32 {
                    for x in (rect.left() * ppp).ceil() as u32..(rect.right() * ppp).floor() as u32 {
                        let p = image.get_pixel(x, y);
                        if p[1] > 80 && u16::from(p[1]) > u16::from(p[0]) * 2 && u16::from(p[1]) > u16::from(p[2]) * 2 {
                            xs.push(x);
                        }
                    }
                }
                assert!(!xs.is_empty(), "point must be visible");
                xs.iter().max().unwrap() - xs.iter().min().unwrap() + 1
            };
            let settled = point_width(&mut h);
            let from = h.state().map.view_rect.expect("map").0.center() + egui::vec2(80.0, 50.0);
            h.input_mut().events.push(egui::Event::PointerMoved(from));
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            h.step();
            let to = from + egui::vec2(12.0, 0.0);
            h.input_mut().events.push(egui::Event::PointerMoved(to));
            h.step();
            let dragging = point_width(&mut h);
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
            wait_for_frame(&mut h);
            let stopped = point_width(&mut h);
            assert!(settled.abs_diff(stopped) <= (2.0 * ppp) as u32, "small pan must keep point size");
            assert!(
                dragging.abs_diff(stopped) <= (2.0 * ppp) as u32,
                "{body} GPU={gpu} density={ppp}: point width while dragging {dragging}, settled {stopped}"
            );
        }
    }
}

#[test]
fn a_globe_keeps_native_wms_and_vector_detail_during_navigation_and_release() {
    use rustgis_ui_egui::rustgis_engine::{
        data::{GeometryType, io::read_geojson},
        geom::{Coord, lonlat_to_merc},
        map::{Camera, Projection, Renderer, Rgba, Symbol, TileKind, TileLayer},
        render::{View, render},
    };
    serve_planet_tiles();
    for gpu in [false, true] {
        let Some(mut h) = harness_at_density(2.0) else { return };
        h.state_mut().run("ui.map.gpu", json!({"on":gpu})).expect("backend");
        h.state_mut().run("map.body", json!({"name":"moon", "basemap":false})).expect("body");
        let m = h.state_mut().session.map_mut();
        m.layers.clear();
        m.tiles.clear();
        m.graticule = false;
        m.projection = Projection::Globe;
        m.camera = Camera { center: lonlat_to_merc(Coord::new(15.0, 25.0)), res: 5_000.0, bearing: 0.0, pitch: 0.0 };
        let mut wms = TileLayer::new(
            "WMS",
            TileKind::Raster,
            "https://quality-wms.test/wms?SERVICE=WMS&REQUEST=GetMap&LAYERS=imagery&SRS=EPSG:3857&BBOX={bbox-epsg-3857}&WIDTH={width}&HEIGHT={height}&FORMAT=image/png",
        );
        // Keep the fixture's ground pattern identical across zoom levels.
        wms.max_zoom = 4;
        m.tiles.push(wms);
        for (kind, coordinates) in [
            ("Point", json!([15.0, 25.0])),
            ("LineString", json!([[14.0, 24.7], [16.0, 24.7]])),
            ("Polygon", json!([[[14.5, 25.3], [15.5, 25.3], [15.5, 25.8], [14.5, 25.8], [14.5, 25.3]]])),
        ] {
            let text = json!({"type":"Feature","properties":{},"geometry":{"type":kind,"coordinates":coordinates}}).to_string();
            let (data, _) = read_geojson(&text, kind).expect("vector");
            let id = m.add_dataset(data);
            m.layer_mut(id).expect("layer").renderer = Renderer::Single(Symbol {
                fill: Rgba::rgb(20, 240, 20).with_alpha(if kind == "Polygon" { 0 } else { 255 }),
                stroke: Rgba::rgb(20, 240, 20),
                stroke_width: 2.0,
                size: 16.0,
                ..Symbol::default_for(GeometryType::Point, 0)
            });
        }
        wait_for_frame(&mut h);
        let check = |h: &mut egui_kittest::Harness<'_, RustGisApp>, stage: &str| {
            let rect = h.state().map.view_rect.expect("map").0;
            let image = h.render().expect("native globe screenshot");
            let view = View {
                width: f64::from(rect.width()),
                height: f64::from(rect.height()),
                pixels_per_point: 2.0,
                camera: h.state().session.map().camera,
                sky: true,
            };
            let reference = render(&mut h.state().session.map().clone(), &view).expect("native reference");
            let (w, hpx) = (reference.pixmap.width(), reference.pixmap.height());
            let mut error = 0u64;
            let mut samples = 0u64;
            let (mut darkest, mut brightest) = (255u8, 0u8);
            for y in (hpx / 4..hpx * 3 / 4).step_by(3) {
                for x in (w / 4..w * 3 / 4).step_by(3) {
                    let actual = image.get_pixel((rect.min.x * 2.0).round() as u32 + x, (rect.min.y * 2.0).round() as u32 + y);
                    let expected = reference.pixmap.pixel(x, y).expect("reference pixel");
                    darkest = darkest.min(expected.red());
                    brightest = brightest.max(expected.red());
                    error += u64::from(actual[0].abs_diff(expected.red()));
                    samples += 1;
                }
            }
            assert!(brightest.saturating_sub(darkest) > 120, "WMS test must actually display fine image detail");
            // Allow one image reprojection, but reject the additional coarse Mercator copy.
            assert!(error < samples * 17, "GPU={gpu} {stage}: native detail changed by {} gray levels", error / samples);
        };
        let from = h.state().map.view_rect.expect("map").0.center() + egui::vec2(100.0, 80.0);
        h.input_mut().events.push(egui::Event::PointerMoved(from));
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        h.step();
        let to = from + egui::vec2(12.0, 0.0);
        h.input_mut().events.push(egui::Event::PointerMoved(to));
        h.step();
        check(&mut h, "dragging");
        let before = h.state().map.full_frame_camera();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut offset = 0.0;
        while h.state().map.full_frame_camera() == before {
            assert!(std::time::Instant::now() < deadline, "native globe frames must refresh before the drag ends");
            offset += 0.1;
            h.input_mut().events.push(egui::Event::PointerMoved(to + egui::vec2(offset, 0.0)));
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        h.step();
        // Check the first released frame, before waiting for any background render.
        check(&mut h, "first frame after release");
        for delta in [12.0, -24.0] {
            h.input_mut().events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, delta),
                phase: egui::TouchPhase::Move,
                modifiers: Default::default(),
            });
            h.step();
            check(&mut h, "zooming");
        }
        wait_for_frame(&mut h);
        check(&mut h, "settled");
    }
}

#[test]
fn globe_vectors_keep_their_shapes_through_large_camera_changes() {
    use rustgis_ui_egui::rustgis_engine::{
        data::{GeometryType, io::read_geojson},
        geom::{Coord, lonlat_to_merc},
        map::{Camera, Projection, Renderer, Rgba, Symbol},
        render::{View, render},
    };
    for gpu in [false, true] {
        for kind in ["Point", "LineString", "Polygon"] {
            let Some(mut h) = harness_at_density(2.0) else { return };
            h.state_mut().run("ui.map.gpu", json!({"on":gpu})).expect("backend");
            let m = h.state_mut().session.map_mut();
            m.body = "moon".into();
            m.layers.clear();
            m.tiles.clear();
            m.graticule = false;
            m.projection = Projection::Globe;
            m.camera = Camera { center: lonlat_to_merc(Coord::new(0.0, 0.0)), res: 20_000.0, bearing: 0.0, pitch: 0.0 };
            let coordinates = match kind {
                "Point" => json!([20.0, 0.0]),
                "LineString" => json!([[15.0, -5.0], [25.0, 5.0]]),
                _ => json!([
                    [[15.0, -5.0], [25.0, -5.0], [25.0, 5.0], [15.0, 5.0], [15.0, -5.0]],
                    [[18.0, -2.0], [18.0, 2.0], [22.0, 2.0], [22.0, -2.0], [18.0, -2.0]]
                ]),
            };
            let text = json!({"type":"Feature","properties":{},"geometry":{"type":kind,"coordinates":coordinates}}).to_string();
            let (data, _) = read_geojson(&text, kind).expect("geometry");
            let id = m.add_dataset(data);
            m.layer_mut(id).expect("layer").renderer = Renderer::Single(Symbol {
                fill: Rgba::rgb(20, 240, 20),
                stroke: Rgba::rgb(20, 240, 20),
                stroke_width: if kind == "Point" { 0.0 } else { 2.0 },
                size: 24.0,
                ..Symbol::default_for(GeometryType::Point, 0)
            });
            wait_for_frame(&mut h);
            let rect = h.state().map.view_rect.expect("map").0;
            let from = rect.center();
            h.input_mut().events.push(egui::Event::PointerMoved(from));
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            h.step();
            // Advance the camera in one frame, without letting a background result catch up.
            // Reprojecting the previous point image stretches its circle into an ellipse;
            // overlapping old and preview polygon images produce displaced edges and seams.
            h.state_mut().session.map_mut().camera =
                Camera { center: lonlat_to_merc(Coord::new(25.0, 10.0)), res: 10_000.0, bearing: 25.0, pitch: 35.0 };
            h.step();
            assert!(
                h.output().shapes.iter().any(|s| matches!(s.shape, egui::Shape::Callback(_))),
                "live globe geometry uses direct presentation"
            );
            let actual = h.render().expect("moving shapes");
            let view = View {
                width: f64::from(rect.width()),
                height: f64::from(rect.height()),
                pixels_per_point: 2.0,
                camera: h.state().session.map().camera,
                sky: true,
            };
            let reference = render(&mut h.state().session.map().clone(), &view).expect("reference");
            let green = |r: u8, g: u8, b: u8| g > 80 && u16::from(g) > 2 * u16::from(r) && u16::from(g) > 2 * u16::from(b);
            let (mut mismatch, mut expected_count) = (0, 0);
            for y in 0..reference.pixmap.height() {
                for x in 0..reference.pixmap.width() {
                    let p = actual.get_pixel((rect.left() * 2.0).round() as u32 + x, (rect.top() * 2.0).round() as u32 + y);
                    let q = reference.pixmap.pixel(x, y).expect("pixel");
                    let expected = green(q.red(), q.green(), q.blue());
                    expected_count += usize::from(expected);
                    let present = green(p[0], p[1], p[2]);
                    if present != expected {
                        // MSAA and analytic rasterization can disagree at a one-pixel edge.
                        let nearby = (y.saturating_sub(1)..=(y + 1).min(reference.pixmap.height() - 1)).any(|ny| {
                            (x.saturating_sub(1)..=(x + 1).min(reference.pixmap.width() - 1)).any(|nx| {
                                if present {
                                    let p = reference.pixmap.pixel(nx, ny).expect("nearby reference");
                                    green(p.red(), p.green(), p.blue())
                                } else {
                                    let p =
                                        actual.get_pixel((rect.left() * 2.0).round() as u32 + nx, (rect.top() * 2.0).round() as u32 + ny);
                                    green(p[0], p[1], p[2])
                                }
                            })
                        });
                        mismatch += usize::from(!nearby);
                    }
                }
            }
            assert!(expected_count > 100, "fixture must show its geometry");
            assert!(
                mismatch < expected_count / 20 + 20,
                "GPU={gpu} {kind}: {mismatch} pixels differ from the actual shape ({expected_count} pixels)"
            );
        }
    }
}
