//! Reproject the last full-resolution globe frame while the next one is drawing.
//! Sampling its own screen grid avoids the moving Mercator drape's resolution reduction.

use egui::{Color32, Mesh, Painter, Rect, TextureHandle, pos2};
use rustgis_engine::geom::Coord;
use rustgis_engine::map::{Camera, Globe};
use rustgis_engine::render::View;

pub(crate) struct GlobeFrame {
    texture: TextureHandle,
    columns: usize,
    rows: usize,
    ground: Vec<Option<Coord>>,
}

impl GlobeFrame {
    /// Keep the image and its ground coordinates; projecting them later needs no map redraw.
    pub fn new(texture: TextureHandle, view: &View) -> Self {
        let columns = ((view.width / 12.0).ceil() as usize).clamp(4, 128);
        let rows = ((view.height / 12.0).ceil() as usize).clamp(4, 128);
        let globe = Globe::new(&view.camera, view.width, view.height);
        let ground = (0..=rows)
            .flat_map(|j| {
                (0..=columns).map(move |i| globe.inverse(view.width * i as f64 / columns as f64, view.height * j as f64 / rows as f64))
            })
            .collect();
        Self { texture, columns, rows, ground }
    }

    pub fn paint(&self, painter: &Painter, rect: Rect, camera: &Camera) {
        if let Some(mesh) = self.mesh(rect, camera) {
            painter.add(egui::Shape::mesh(mesh));
        }
    }

    fn mesh(&self, rect: Rect, camera: &Camera) -> Option<Mesh> {
        let globe = Globe::new(camera, f64::from(rect.width()), f64::from(rect.height()));
        let mut mesh = Mesh::with_texture(self.texture.id());
        let mut visible = Vec::with_capacity(self.ground.len());
        for (n, ground) in self.ground.iter().enumerate() {
            let at = ground.and_then(|c| globe.forward(c)).filter(|(x, y)| x.is_finite() && y.is_finite());
            visible.push(at.is_some());
            let (x, y) = at.unwrap_or((0.0, 0.0));
            mesh.vertices.push(egui::epaint::Vertex {
                pos: rect.min + egui::vec2(x as f32, y as f32),
                uv: pos2((n % (self.columns + 1)) as f32 / self.columns as f32, (n / (self.columns + 1)) as f32 / self.rows as f32),
                color: Color32::WHITE,
            });
        }
        for j in 0..self.rows {
            for i in 0..self.columns {
                let a = j * (self.columns + 1) + i;
                let b = a + 1;
                let c = a + self.columns + 1;
                let d = c + 1;
                for tri in [[a, c, b], [b, c, d]] {
                    if tri.iter().all(|n| visible.get(*n) == Some(&true)) {
                        mesh.indices.extend(tri.map(|n| n as u32));
                    }
                }
            }
        }
        (!mesh.indices.is_empty()).then_some(mesh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_engine::geom::lonlat_to_merc;

    fn frame(lon: f64, res: f64) -> (GlobeFrame, Rect) {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture("sharp", egui::ColorImage::new([16, 16], vec![Color32::WHITE; 256]), egui::TextureOptions::LINEAR);
        let view = View {
            width: 800.0,
            height: 600.0,
            pixels_per_point: 2.0,
            camera: Camera { center: lonlat_to_merc(Coord::new(lon, 0.0)), res, bearing: 0.0, pitch: 0.0 },
            sky: true,
        };
        (GlobeFrame::new(texture, &view), Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0)))
    }

    #[test]
    fn a_sharp_frame_behind_the_globe_has_no_empty_gpu_mesh() {
        let (frame, rect) = frame(0.0, 5_000.0);
        let camera = Camera { center: lonlat_to_merc(Coord::new(180.0, 0.0)), res: 5_000.0, bearing: 0.0, pitch: 0.0 };
        assert!(frame.mesh(rect, &camera).is_none());
    }

    #[test]
    fn sharp_frames_reproject_across_the_antimeridian_when_panning_and_zooming() {
        let (frame, rect) = frame(170.0, 20_000.0);
        for (lon, res, pitch) in [(-175.0, 20_000.0, 0.0), (-175.0, 10_000.0, 0.0), (175.0, 30_000.0, 60.0)] {
            let camera = Camera { center: lonlat_to_merc(Coord::new(lon, 0.0)), res, bearing: 0.0, pitch };
            let mesh = frame.mesh(rect, &camera).expect("visible native frame");
            assert!(mesh.is_valid() && !mesh.indices.is_empty());
            assert!(mesh.vertices.iter().all(|v| v.pos.is_finite() && v.uv.is_finite()));
        }
    }
}
