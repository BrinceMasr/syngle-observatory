//! Lyon tessellation of layer geometry into GPU meshes, matching the tiny-skia CPU path's
//! rules: EvenOdd holes, round joins and caps, the 0.35 physical-pixel decimation of
//! `Ctx::ring_path`, and screen-constant stroke widths — strokes tessellate at width 1.0 in
//! map units and the vertex shader displaces along lyon's unit normal by
//! `stroke_px * map_per_px / 2`, so one tessellation serves every zoom.

use lyon::path::Path;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, LineCap, LineJoin, StrokeOptions, StrokeTessellator, VertexBuffers,
    VertexSource,
};
use rustgis_data::Geometry;
use rustgis_geom::{Coord, Extent};
use rustgis_map::{Layer, Rgba, Symbol};

use crate::SELECTION;
use crate::gpu_scene::{DrawCmd, MeshVertex, NO_REACH, PointInstance};

/// Lyon refused to tessellate something tiny-skia would draw; the caller falls the layer (or
/// the outline) back to the CPU path. Empty marker for `Result<_, TessError>` so clippy's
/// `result_unit_err` stays happy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TessError;

/// Premultiplied 0..1 RGBA for the GPU (tiny-skia stores premultiplied; wgpu blends the same way).
pub(crate) fn premul(c: Rgba) -> [f32; 4] {
    let a = c.a() as f32 / 255.0;
    [c.r() as f32 / 255.0 * a, c.g() as f32 / 255.0 * a, c.b() as f32 / 255.0 * a, a]
}

/// Origin-relative f32 position (the f64 subtraction happens here, where the precision is).
fn rel(c: Coord, origin: [f64; 2]) -> [f32; 2] {
    [(c.x - origin[0]) as f32, (c.y - origin[1]) as f32]
}

/// Drops consecutive points closer than 0.35 physical px, keeping the first and always the
/// last — the exact rule of `Ctx::ring_path` (the dropped point doesn't become the anchor) —
/// and yields one run per maximal finite segment: a point the projection can't show breaks
/// the ring there rather than being bridged (also `ring_path`; lyon's path builder panics on
/// NaN, so non-finite coordinates never reach it).
/// `map_per_px` is map units per physical pixel (`camera.res / pixels_per_point` on the flat
/// map, where `to_screen` is affine).
fn decimate_runs(ring: &[Coord], map_per_px: f64) -> Vec<Vec<Coord>> {
    let threshold = 0.35 * map_per_px;
    let n = ring.len();
    let mut runs = Vec::new();
    let mut out: Vec<Coord> = Vec::new();
    let mut last: Option<Coord> = None;
    for (i, c) in ring.iter().enumerate() {
        if !c.is_finite() {
            if !out.is_empty() {
                runs.push(std::mem::take(&mut out));
            }
            last = None;
            continue;
        }
        let keep = !matches!(last, Some(l) if (c.x - l.x).abs() + (c.y - l.y).abs() < threshold && i + 1 != n);
        if keep {
            out.push(*c);
            last = Some(*c);
        }
    }
    if !out.is_empty() {
        runs.push(out);
    }
    runs
}

/// One closed subpath per decimated finite run of every ring. `None` when nothing survives
/// (the caller's `?` then skips or falls the layer back).
fn path_of(rings: &[Vec<Coord>], origin: [f64; 2], map_per_px: f64, min_points: usize) -> Option<Path> {
    let mut b = Path::builder();
    let mut any = false;
    for ring in rings {
        for pts in decimate_runs(ring, map_per_px) {
            if pts.len() < min_points {
                continue;
            }
            any = true;
            let p0 = rel(pts[0], origin);
            b.begin(lyon::math::point(p0[0], p0[1]));
            for c in &pts[1..] {
                let p = rel(*c, origin);
                b.line_to(lyon::math::point(p[0], p[1]));
            }
            b.close();
        }
    }
    any.then(|| b.build())
}

/// Tessellates polygon fills (all rings in one EvenOdd path, like the CPU's `Ctx::polygon`).
/// `Ok(None)` when nothing would draw; `Err` when lyon fails (the caller falls the layer back
/// to the CPU pixmap — tiny-skia still draws geometry lyon refuses). Non-finite coordinates
/// never reach lyon: `decimate` drops them.
pub(crate) fn fill_polygon(
    polys: &[Vec<Vec<Coord>>],
    origin: [f64; 2],
    color: [f32; 4],
    map_per_px: f64,
) -> Result<Option<(Vec<MeshVertex>, Vec<u32>)>, TessError> {
    let mut out: VertexBuffers<MeshVertex, u32> = VertexBuffers::new();
    let mut tess = FillTessellator::new();
    for rings in polys {
        let Some(path) = path_of(rings, origin, map_per_px, 3) else { continue };
        tess.tessellate_path(
            &path,
            &FillOptions::default().with_fill_rule(FillRule::EvenOdd),
            &mut BuffersBuilder::new(&mut out, |v: lyon::tessellation::FillVertex| {
                let p = v.position();
                MeshVertex { position: [p.x, p.y], normal: [0.0, 0.0], color, stroke_px: 0.0, reach: NO_REACH }
            }),
        )
        .map_err(|_| TessError)?;
    }
    Ok((!out.indices.is_empty()).then_some((out.vertices, out.indices)))
}

/// Strokes rings (closed, polygon outlines) or line parts (open) at width 1.0; the vertex
/// shader applies the real width. Ring outlines take no width floor (`Ctx::polygon`),
/// line parts floor at half a physical pixel (`Ctx::line`).
fn stroke(
    rings: &[Vec<Coord>],
    origin: [f64; 2],
    color: [f32; 4],
    width_px: f32,
    map_per_px: f64,
    closed: bool,
    min_points: usize,
) -> Result<Option<(Vec<MeshVertex>, Vec<u32>)>, TessError> {
    let mut out: VertexBuffers<MeshVertex, u32> = VertexBuffers::new();
    let mut tess = StrokeTessellator::new();
    // Tolerance is in the tessellation's own units (the radius here is 0.5): 0.005 gives ~31
    // segments per full circle of round cap/join — lyon's default 0.1 gives ~5, and the
    // vertex shader scales the normals up to any stroke_px, which would read as polygonal
    // next to tiny-skia's analytic curves.
    let options =
        StrokeOptions::default().with_line_width(1.0).with_tolerance(0.005).with_line_cap(LineCap::Round).with_line_join(LineJoin::Round);
    for ring in rings {
        for pts in decimate_runs(ring, map_per_px) {
            if pts.len() < min_points {
                continue;
            }
            let at: Vec<[f32; 2]> = pts.iter().map(|c| rel(*c, origin)).collect();
            let Some((first, rest)) = at.split_first() else { continue };
            let mut b = Path::builder();
            b.begin(lyon::math::point(first[0], first[1]));
            for p in rest {
                b.line_to(lyon::math::point(p[0], p[1]));
            }
            b.end(closed);
            // position_on_path = position - normal * half_width, half_width = 0.5 at width 1.0.
            tess.tessellate_path(
                &b.build(),
                &options,
                &mut BuffersBuilder::new(&mut out, |v: lyon::tessellation::StrokeVertex| {
                    let (p, n) = (v.position(), v.normal());
                    let reach = match v.source() {
                        VertexSource::Endpoint { id } => inner_reach(&at, id.to_usize(), closed, n.square_length()),
                        VertexSource::Edge { .. } => NO_REACH,
                    };
                    MeshVertex { position: [p.x - n.x * 0.5, p.y - n.y * 0.5], normal: [n.x, n.y], color, stroke_px: width_px, reach }
                }),
            )
            .map_err(|_| TessError)?;
        }
    }
    Ok((!out.indices.is_empty()).then_some((out.vertices, out.indices)))
}

/// The most the half width may be, in map units, where the shader pushes the vertex at `at[i]`
/// out along its normal (whose squared length is `normal_sq`), for the vertex to stay on the stroke.
///
/// Lyon puts the inside corner of a turn at the one point where the two inside offset lines
/// meet: `1 / cos(turn / 2)` half widths away (`tan(turn / 2)` of them along each segment): 11
/// for a turn of 170 degrees, 57 for 178, and lyon stops only at 10 000. (It folds the join
/// instead when both segments are shorter than that reach at width 1.0, a metre in Web Mercator,
/// which a coastline's are not.) The tessellation is made at width 1.0 and the shader scales the
/// normals to the real width, a pixel or more of kilometres at a coastline's default zoom: the
/// meeting point then lies far past the ends of the segments, as a spike tens of half widths long.
/// Holding the half width at half of the shorter neighbouring segment over `tan(turn / 2)` keeps
/// the point inside both segments (half, so that the two ends of one segment never pass each
/// other). Outer joins, caps and a plain bend have a normal of 1 and no limit; the turn is read
/// from the normal's length, so a round join outside and this limit inside agree on the angle.
fn inner_reach(at: &[[f32; 2]], i: usize, closed: bool, normal_sq: f32) -> f32 {
    if normal_sq.is_nan() || normal_sq <= 1.0 + 1e-3 {
        return NO_REACH;
    }
    let n = at.len();
    let prev = i.checked_sub(1).or(closed.then_some(n.saturating_sub(1)));
    let next = (i + 1 < n).then_some(i + 1).or(closed.then_some(0));
    let (Some(prev), Some(next)) = (prev, next) else { return NO_REACH };
    let (Some(a), Some(c), Some(b)) = (at.get(prev), at.get(i), at.get(next)) else { return NO_REACH };
    let (l_in, l_out) = ((c[0] - a[0]).hypot(c[1] - a[1]), (b[0] - c[0]).hypot(b[1] - c[1]));
    let tan_half_turn = (normal_sq - 1.0).sqrt();
    let reach = 0.5 * l_in.min(l_out) / tan_half_turn;
    if reach.is_finite() { reach } else { NO_REACH }
}

/// Polygon outlines: every ring stroked (the CPU strokes exterior and holes alike).
pub(crate) fn stroke_rings(
    polys: &[Vec<Vec<Coord>>],
    origin: [f64; 2],
    color: [f32; 4],
    width_px: f32,
    map_per_px: f64,
) -> Result<Option<(Vec<MeshVertex>, Vec<u32>)>, TessError> {
    let rings: Vec<Vec<Coord>> = polys.iter().flatten().cloned().collect();
    stroke(&rings, origin, color, width_px, map_per_px, true, 3)
}

/// Line features (round caps, round joins, 0.5 px minimum width).
pub(crate) fn stroke_lines(
    parts: &[Vec<Coord>],
    origin: [f64; 2],
    color: [f32; 4],
    width_px: f32,
    map_per_px: f64,
) -> Result<Option<(Vec<MeshVertex>, Vec<u32>)>, TessError> {
    stroke(parts, origin, color, width_px.max(0.5), map_per_px, false, 2)
}

/// One marker as instances of a unit-space shape template: the selection halo first (a
/// larger filled shape, matching the CPU's 3.5 px halo stroke drawn under the fill), then the
/// outline (a larger fill in the stroke colour), then the fill SHRUNK by half the stroke
/// width — the CPU strokes the outline centered on the shape's edge (half over the fill), so
/// stroke at r + w/2 under fill at r − w/2 reproduces the exact band (`Ctx::marker`).
pub(crate) fn marker_instances(c: Coord, origin: [f64; 2], s: &Symbol, ppp: f32, halo: Option<Rgba>) -> Vec<PointInstance> {
    let r = (s.size.max(1.0) * ppp) / 2.0;
    let shape = s.marker as u32;
    let mut out = Vec::with_capacity(3);
    if let Some(h) = halo {
        out.push(PointInstance { position: rel(c, origin), color: premul(h), radius_px: r + 1.75 * ppp, shape });
    }
    let stroke_active = s.stroke_width > 0.0 && s.stroke.a() > 0 && (s.stroke_width * ppp).is_finite();
    if stroke_active {
        out.push(PointInstance { position: rel(c, origin), color: premul(s.stroke), radius_px: r + s.stroke_width * ppp / 2.0, shape });
    }
    let fill_r = if stroke_active { (r - s.stroke_width * ppp / 2.0).max(0.0) } else { r };
    out.push(PointInstance { position: rel(c, origin), color: premul(s.fill), radius_px: fill_r, shape });
    out
}

/// Appends a tessellated part with its indices rebased onto the shared vertex list. `Err` when
/// lyon failed the part or the result would overflow the u32 index space.
fn extend_mesh(
    v: &mut Vec<MeshVertex>,
    i: &mut Vec<u32>,
    part: Result<Option<(Vec<MeshVertex>, Vec<u32>)>, TessError>,
) -> Result<(), TessError> {
    if let Some((pv, pi)) = part? {
        if v.len() + pv.len() > u32::MAX as usize {
            return Err(TessError); // a mesh this big can't index itself (u32): the layer falls back to the CPU
        }
        let base = v.len() as u32;
        v.extend(pv);
        i.extend(pi.into_iter().map(|x| x + base));
    }
    Ok(())
}

/// Appends one feature's GPU geometry, in the CPU path's per-feature order (fill, then
/// outline — `Ctx::feature`). Transparent fills/strokes are skipped: tiny-skia draws them as
/// no-ops, and not emitting them is identical output for less work. `Err` when lyon refuses
/// the geometry — the layer then falls back to the CPU path entirely.
fn append_geometry(
    v: &mut Vec<MeshVertex>,
    i: &mut Vec<u32>,
    pts: &mut Vec<PointInstance>,
    g: &Geometry,
    s: &Symbol,
    origin: [f64; 2],
    wireframe: bool,
    ppp: f32,
    map_per_px: f64,
) -> Result<(), TessError> {
    match g {
        Geometry::Point(c) => pts.extend(marker_instances(*c, origin, s, ppp, None)),
        Geometry::MultiPoint(cs) => cs.iter().for_each(|c| pts.extend(marker_instances(*c, origin, s, ppp, None))),
        Geometry::Line(parts) => {
            // A non-finite width (hostile symbol state) would put NaN vertices on the GPU.
            if s.stroke.a() > 0 && (s.stroke_width * ppp).is_finite() {
                extend_mesh(v, i, stroke_lines(parts, origin, premul(s.stroke), s.stroke_width * ppp, map_per_px))?;
            }
        }
        Geometry::Polygon(polys) if wireframe => {
            extend_mesh(v, i, stroke_rings(polys, origin, premul(s.stroke.with_alpha(255)), s.stroke_width.max(1.0) * ppp, map_per_px))?;
        }
        Geometry::Polygon(polys) => {
            if s.fill.a() > 0 {
                extend_mesh(v, i, fill_polygon(polys, origin, premul(s.fill), map_per_px))?;
            }
            if s.stroke_width > 0.0 && s.stroke.a() > 0 {
                extend_mesh(v, i, stroke_rings(polys, origin, premul(s.stroke), s.stroke_width * ppp, map_per_px))?;
            }
        }
    }
    Ok(())
}

/// A layer with more point markers than this in view draws one marker per symbol and physical pixel (see [`Cells`]).
const LOD_ABOVE: usize = 10_000;
/// The most distinct symbols [`Cells`] tells apart: a layer with more draws the rest marker by marker.
const LOD_SYMBOLS: usize = 16;
/// The most pixels of the view [`Cells`] keeps a mark for, per symbol (a bit each: 1 MB): a larger view is drawn in full.
const LOD_PIXELS: u64 = 1 << 23;

/// The level of detail of a dense point layer: a marker whose centre falls in a physical pixel that already holds a marker
/// of the same opaque symbol adds no ink (it covers the same pixels to within a pixel), so it is not sent to the card.
/// Every feature stays in the layer, for identify, tables and the next frame at another scale; only the submission is
/// thinned. A see-through marker is never thinned (the overlap is the picture), and a marker the grid cannot place is drawn.
struct Cells {
    /// The grid's top left corner (map units), and the size of a cell: one physical pixel.
    x0: f64,
    y0: f64,
    cell: f64,
    /// Cells across and down, and the words of bits that mark them.
    cols: u64,
    rows: u64,
    words: usize,
    /// The symbols seen so far, in order, and for each the cells that hold a marker of it (a bit each).
    symbols: Vec<(Symbol, Vec<u64>)>,
}

impl Cells {
    /// A grid over `ext` of cells `map_per_px` across, or `None` when there is no such grid (a view too large to mark).
    fn new(ext: &Extent, map_per_px: f64) -> Option<Cells> {
        let size = |lo: f64, hi: f64| ((hi - lo) / map_per_px).ceil() + 1.0;
        let (cols, rows) = (size(ext.min_x, ext.max_x), size(ext.min_y, ext.max_y));
        // Written so that a NaN fails.
        let fits = map_per_px.is_finite() && map_per_px > 0.0 && cols >= 1.0 && rows >= 1.0 && cols * rows <= LOD_PIXELS as f64;
        fits.then(|| {
            let (cols, rows) = (cols as u64, rows as u64);
            Cells {
                x0: ext.min_x,
                y0: ext.max_y,
                cell: map_per_px,
                cols,
                rows,
                words: usize::try_from((cols * rows).div_ceil(64)).unwrap_or(0),
                symbols: Vec::new(),
            }
        })
    }

    /// Whether the marker for `c` in symbol `s` adds nothing to what is drawn, noting its cell when it is the first there.
    fn covered(&mut self, c: Coord, s: &Symbol) -> bool {
        let opaque = s.fill.a() == 255 && (s.stroke_width <= 0.0 || s.stroke.a() == 255 || s.stroke.a() == 0);
        if !opaque {
            return false;
        }
        let (fx, fy) = (((c.x - self.x0) / self.cell).floor(), ((self.y0 - c.y) / self.cell).floor());
        // (Written so that a NaN fails.)
        if !(fx >= 0.0 && fy >= 0.0 && fx < self.cols as f64 && fy < self.rows as f64) {
            return false;
        }
        let at = fy as u64 * self.cols + fx as u64;
        let class = match self.symbols.iter().position(|(k, _)| k == s) {
            Some(i) => i,
            None if self.symbols.len() < LOD_SYMBOLS => {
                self.symbols.push((*s, vec![0; self.words]));
                self.symbols.len() - 1
            }
            None => return false,
        };
        let Some(word) = self.symbols.get_mut(class).and_then(|(_, bits)| bits.get_mut((at / 64) as usize)) else { return false };
        let bit = 1u64 << (at % 64);
        let seen = *word & bit != 0;
        *word |= bit;
        seen
    }
}

/// Collects one GPU-eligible layer into Mesh/Points commands. `geoms`, `rows` and `visible`
/// are the render loop's own (projected geometries, row filter, extent cull), so the GPU
/// draws exactly the features the CPU would. `Ok(None)` when nothing visible was collected;
/// `Err(TessError)` when lyon refused a feature — the caller then draws the whole layer with
/// tiny-skia instead (nothing is pushed part-way).
pub(crate) fn collect_layer(
    cmds: &mut Vec<DrawCmd>,
    layer: &Layer,
    geoms: &[Option<&Geometry>],
    rows: &[bool],
    visible: &dyn Fn(&Extent) -> bool,
    wireframe: bool,
    ppp: f32,
    map_per_px: f64,
) -> Result<Option<[f64; 2]>, TessError> {
    // Stable origin: the centre of the drawn features' extent. Two passes cost one extra
    // extent fold — the same order of work as the culling pass the CPU render already does.
    let mut ext = Extent::EMPTY;
    let mut markers = 0usize;
    for (row, g) in geoms.iter().enumerate() {
        let Some(g) = g else { continue };
        if rows.get(row) != Some(&true) || !visible(&g.extent()) {
            continue;
        }
        ext = ext.union(&g.extent());
        markers += usize::from(matches!(g, Geometry::Point(_)));
    }
    if ext.is_empty() {
        return Ok(None);
    }
    let origin = [ext.center().x, ext.center().y];
    let (mut v, mut i, mut pts) = (Vec::new(), Vec::new(), Vec::new());
    let mut cells = if markers > LOD_ABOVE { Cells::new(&ext, map_per_px) } else { None };
    for (row, g) in geoms.iter().enumerate() {
        let Some(g) = g else { continue };
        if rows.get(row) != Some(&true) || !visible(&g.extent()) {
            continue;
        }
        let symbol = layer.symbol(row);
        if let (Some(cells), Geometry::Point(c)) = (cells.as_mut(), g)
            && cells.covered(*c, &symbol)
        {
            continue;
        }
        append_geometry(&mut v, &mut i, &mut pts, g, &symbol, origin, wireframe, ppp, map_per_px)?;
    }
    if !i.is_empty() {
        cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
    }
    if !pts.is_empty() {
        cmds.push(DrawCmd::Points { origin, instances: pts });
    }
    Ok(Some(origin))
}

/// Selection highlight of one feature, after its layer (`Ctx::highlight`): haloed markers,
/// a wider cyan under-stroke for lines, translucent cyan fill + outline for polygons.
pub(crate) fn collect_highlight(cmds: &mut Vec<DrawCmd>, g: &Geometry, s: &Symbol, ppp: f32, map_per_px: f64) -> Result<(), TessError> {
    let e = g.extent();
    let origin = [e.center().x, e.center().y];
    match g {
        Geometry::Point(c) => cmds.push(DrawCmd::Points { origin, instances: marker_instances(*c, origin, s, ppp, Some(SELECTION)) }),
        Geometry::MultiPoint(cs) => {
            let instances: Vec<PointInstance> = cs.iter().flat_map(|c| marker_instances(*c, origin, s, ppp, Some(SELECTION))).collect();
            cmds.push(DrawCmd::Points { origin, instances });
        }
        Geometry::Line(parts) => {
            let (mut v, mut i) = (Vec::new(), Vec::new());
            extend_mesh(&mut v, &mut i, stroke_lines(parts, origin, premul(SELECTION), (s.stroke_width + 3.0) * ppp, map_per_px))?;
            extend_mesh(&mut v, &mut i, stroke_lines(parts, origin, premul(s.stroke), s.stroke_width * ppp, map_per_px))?;
            if !i.is_empty() {
                cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
            }
        }
        Geometry::Polygon(polys) => {
            let (mut v, mut i) = (Vec::new(), Vec::new());
            extend_mesh(&mut v, &mut i, fill_polygon(polys, origin, premul(SELECTION.with_alpha(40)), map_per_px))?;
            extend_mesh(&mut v, &mut i, stroke_rings(polys, origin, premul(SELECTION), 2.0 * ppp, map_per_px))?;
            if !i.is_empty() {
                cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
            }
        }
    }
    Ok(())
}

/// The built-in basemap's land polygons (already in map units), fill plus optional outline.
pub(crate) fn collect_basemap(
    cmds: &mut Vec<DrawCmd>,
    polys: &[Vec<Vec<Coord>>],
    origin: [f64; 2],
    land: Rgba,
    outline: Rgba,
    ppp: f32,
    map_per_px: f64,
) -> Result<(), TessError> {
    let (mut v, mut i) = (Vec::new(), Vec::new());
    extend_mesh(&mut v, &mut i, fill_polygon(polys, origin, premul(land), map_per_px))?;
    if outline.a() > 0 {
        extend_mesh(&mut v, &mut i, stroke_rings(polys, origin, premul(outline), 0.6 * ppp, map_per_px))?;
    }
    if !i.is_empty() {
        cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
    }
    Ok(())
}

/// The graticule lines (already in map units).
pub(crate) fn collect_graticule(
    cmds: &mut Vec<DrawCmd>,
    lines: &[Vec<Coord>],
    origin: [f64; 2],
    grat: Rgba,
    ppp: f32,
    map_per_px: f64,
) -> Result<(), TessError> {
    let (mut v, mut i) = (Vec::new(), Vec::new());
    extend_mesh(&mut v, &mut i, stroke_lines(lines, origin, premul(grat), 0.6 * ppp, map_per_px))?;
    if !i.is_empty() {
        cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_map::Marker;

    fn square(x: f64, y: f64, s: f64) -> Vec<Coord> {
        vec![Coord::new(x, y), Coord::new(x + s, y), Coord::new(x + s, y + s), Coord::new(x, y + s)]
    }

    fn point_covered(verts: &[MeshVertex], idx: &[u32], x: f32, y: f32) -> bool {
        idx.chunks(3).any(|t| {
            let [a, b, c] = [verts[t[0] as usize].position, verts[t[1] as usize].position, verts[t[2] as usize].position];
            let d = |o: [f32; 2], u: [f32; 2], v: [f32; 2]| (u[0] - o[0]) * (v[1] - o[1]) - (v[0] - o[0]) * (u[1] - o[1]);
            let (da, db, dc) = (d(a, b, c), d([x, y], b, c), d([x, y], c, a));
            da != 0.0 && db.signum() == da.signum() && dc.signum() == da.signum() && d([x, y], a, b).signum() == da.signum()
        })
    }

    /// A point layer of `n` points on a 100 x 100 grid ten map units apart: the second ten thousand sit 0.3 units east of the
    /// first, so with one unit cells they share a cell with a twin.
    fn dense_points(n: usize, symbol: rustgis_map::Symbol) -> Layer {
        use rustgis_data::{Dataset, Feature, GeometryType};
        let mut ds = Dataset::new("dense", GeometryType::Point, Vec::new());
        ds.features = (0..n)
            .map(|i| {
                let (gx, gy) = ((i % 100) as f64, ((i / 100) % 100) as f64);
                let jitter = if i >= 10_000 { 0.3 } else { 0.0 };
                Feature { id: i as u64 + 1, geometry: Some(Geometry::Point(Coord::new(gx * 10.0 + jitter, gy * 10.0))), attrs: Vec::new() }
            })
            .collect();
        let mut layer = Layer::new(1, ds);
        layer.renderer = rustgis_map::Renderer::Single(symbol);
        layer
    }

    /// The marker instances `collect_layer` sends for `layer` at one map unit per physical pixel.
    fn sent(layer: &Layer) -> usize {
        let geoms: Vec<Option<&Geometry>> = layer.dataset.features.iter().map(|f| f.geometry.as_ref()).collect();
        let rows = vec![true; geoms.len()];
        let mut cmds = Vec::new();
        collect_layer(&mut cmds, layer, &geoms, &rows, &|_: &Extent| true, false, 1.0, 1.0).expect("tessellate");
        cmds.iter().map(|c| if let DrawCmd::Points { instances, .. } = c { instances.len() } else { 0 }).sum()
    }

    #[test]
    fn a_dense_point_layer_sends_one_marker_per_pixel() {
        let s = Symbol::default_for(rustgis_data::GeometryType::Point, 1);
        let layer = dense_points(20_000, s);
        // 10,000 cells, each with a stroke and a fill instance: the 10,000 twins add no ink.
        assert_eq!(sent(&layer), 20_000);
        assert_eq!(layer.dataset.features.len(), 20_000, "every feature is still in the layer");
    }

    #[test]
    fn a_layer_below_the_threshold_sends_every_marker() {
        let s = Symbol::default_for(rustgis_data::GeometryType::Point, 1);
        assert_eq!(sent(&dense_points(LOD_ABOVE, s)), 2 * LOD_ABOVE);
    }

    #[test]
    fn a_see_through_marker_is_never_thinned() {
        let s = Symbol { fill: Rgba::rgb(200, 30, 30).with_alpha(128), ..Symbol::default_for(rustgis_data::GeometryType::Point, 1) };
        assert_eq!(sent(&dense_points(20_000, s)), 40_000, "the overlap of see-through markers is the picture");
    }

    #[test]
    fn cells_tell_symbols_apart_and_draw_what_they_cannot_place() {
        let a = Symbol::default_for(rustgis_data::GeometryType::Point, 1);
        let b = Symbol { size: 9.0, ..a };
        let ext = Extent { min_x: 0.0, min_y: 0.0, max_x: 100.0, max_y: 100.0 };
        let mut cells = Cells::new(&ext, 1.0).expect("a grid");
        let c = Coord::new(10.2, 50.4);
        assert!(!cells.covered(c, &a) && cells.covered(Coord::new(10.7, 50.1), &a), "the second of one symbol in a cell is covered");
        assert!(!cells.covered(c, &b), "another symbol is its own marker");
        assert!(!cells.covered(Coord::new(11.2, 50.4), &a), "the next cell is free");
        for off in [Coord::new(f64::NAN, 1.0), Coord::new(1e300, 1.0), Coord::new(-1e12, 1.0)] {
            assert!(!cells.covered(off, &a) && !cells.covered(off, &a), "a marker off the grid is drawn each time: {off:?}");
        }
        for per_px in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(Cells::new(&ext, per_px).is_none(), "{per_px}");
        }
    }

    #[test]
    fn polygon_with_hole_tessellates_evenodd() {
        let polys = vec![vec![square(0.0, 0.0, 100.0), square(30.0, 30.0, 40.0)]];
        let (v, i) = fill_polygon(&polys, [0.0, 0.0], [1.0, 0.0, 0.0, 1.0], 0.0).expect("tessellate").expect("non-empty");
        assert!(i.len() >= 24, "a hole needs interior triangles: {}", i.len());
        assert!(!point_covered(&v, &i, 50.0, 50.0), "hole center must stay uncovered");
        // (10, 5): inside, and not on a tessellation diagonal (edge points read as uncovered).
        assert!(point_covered(&v, &i, 10.0, 5.0), "corner region must be covered");
    }

    #[test]
    fn decimate_matches_ring_path_rule() {
        // map_per_px = 2: threshold 0.7. Points at 0.5 apart drop, 0.8 apart keep.
        let ring = vec![
            Coord::new(0.0, 0.0),
            Coord::new(0.5, 0.0),  // 0.5 from anchor: drop
            Coord::new(1.0, 0.0),  // 1.0 from anchor (not the dropped 0.5): keep
            Coord::new(1.5, 0.0),  // 0.5: drop
            Coord::new(1.7, 0.0),  // 0.7: not < 0.7, keep
            Coord::new(1.71, 0.0), // last point: always kept
        ];
        let runs = decimate_runs(&ring, 2.0);
        assert_eq!(
            runs,
            vec![vec![Coord::new(0.0, 0.0), Coord::new(1.0, 0.0), Coord::new(1.7, 0.0), Coord::new(1.71, 0.0)]],
            "no NaN: one run, the old rule"
        );
    }

    #[test]
    fn decimate_breaks_at_non_finite_points_like_ring_path() {
        let ring = vec![
            Coord::new(0.0, 0.0),
            Coord::new(10.0, 0.0),
            Coord::new(f64::NAN, 0.0), // the break
            Coord::new(20.0, 0.0),
            Coord::new(20.5, 0.0), // 0.5 < 0.7 from the run's fresh anchor: dropped
            Coord::new(30.0, 0.0),
        ];
        let runs = decimate_runs(&ring, 2.0);
        assert_eq!(runs.len(), 2, "two finite runs, no bridging edge");
        assert_eq!(runs[0], vec![Coord::new(0.0, 0.0), Coord::new(10.0, 0.0)]);
        assert_eq!(runs[1], vec![Coord::new(20.0, 0.0), Coord::new(30.0, 0.0)], "decimation resets across the break");
    }

    #[test]
    fn stroke_vertices_carry_centerline_and_normal() {
        let parts = vec![vec![Coord::new(0.0, 0.0), Coord::new(100.0, 0.0)]];
        let (v, i) = stroke_lines(&parts, [0.0, 0.0], [0.0, 0.0, 1.0, 1.0], 2.0, 0.0).expect("stroke").expect("non-empty");
        assert!(!i.is_empty());
        for vtx in &v {
            assert!(vtx.position[1].abs() < 1e-4, "centerline stays on the path: y={}", vtx.position[1]);
            let len = (vtx.normal[0] * vtx.normal[0] + vtx.normal[1] * vtx.normal[1]).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "unit normal (round caps point along x): {:?}", vtx.normal);
            assert_eq!(vtx.stroke_px, 2.0);
        }
    }

    #[test]
    fn ring_stroke_has_no_width_floor_and_closes() {
        let polys = vec![vec![square(0.0, 0.0, 10.0)]];
        let (v, i) = stroke_rings(&polys, [0.0, 0.0], [0.0; 4], 0.2, 0.0).expect("stroke").expect("non-empty");
        assert!(!i.is_empty());
        assert!(v.iter().all(|x| x.stroke_px == 0.2), "polygon outlines keep their exact width");
    }

    #[test]
    fn stroke_round_joints_are_finely_subdivided() {
        // A 90° arc stroked at width 1.0: with tolerance 0.005 the join is a real curve
        // (~8 vertices), not the ~1–2 of lyon's default 0.1 tolerance (~74° steps).
        let arc: Vec<Coord> = (0..=9)
            .map(|i| {
                let a = std::f64::consts::FRAC_PI_2 * i as f64 / 9.0;
                Coord::new(a.cos() * 100.0, a.sin() * 100.0)
            })
            .collect();
        let (v, _) = stroke_lines(&[arc], [0.0, 0.0], [0.0; 4], 2.0, 0.0).expect("stroke").expect("non-empty");
        assert!(v.len() >= 40, "round join subdivided finely: {} vertices", v.len());
    }

    #[test]
    fn line_width_floors_at_half_pixel() {
        let parts = vec![vec![Coord::new(0.0, 0.0), Coord::new(10.0, 0.0)]];
        let (v, _) = stroke_lines(&parts, [0.0, 0.0], [0.0; 4], 0.2, 0.0).expect("stroke").expect("non-empty");
        assert!(v.iter().all(|x| x.stroke_px == 0.5));
    }

    #[test]
    fn marker_halo_and_stroke_emit_under_instances() {
        let s =
            Symbol { fill: Rgba::rgb(255, 0, 0), stroke: Rgba::rgb(255, 255, 255), stroke_width: 1.0, size: 8.0, marker: Marker::Circle };
        let three = marker_instances(Coord::new(5.0, 5.0), [5.0, 5.0], &s, 2.0, Some(Rgba::rgb(0, 255, 255)));
        assert_eq!(three.len(), 3, "halo + stroke + fill");
        assert_eq!(three[0].radius_px, 8.0 + 3.5, "halo = r + 1.75*ppp"); // r = 8*2/2 = 8
        assert_eq!(three[1].radius_px, 9.0, "stroke at r + w/2 (outer band)");
        assert_eq!(three[2].radius_px, 7.0, "fill at r − w/2 (the stroke's inner half covers its band)");
        // Painter's order: halo, stroke, fill.
        assert_eq!(three[0].color, premul(Rgba::rgb(0, 255, 255)));
        assert_eq!(three[1].color, premul(Rgba::rgb(255, 255, 255)));
        assert_eq!(three[2].color, premul(Rgba::rgb(255, 0, 0)));
        let one = marker_instances(Coord::new(5.0, 5.0), [5.0, 5.0], &Symbol { stroke_width: 0.0, ..s }, 2.0, None);
        assert_eq!(one.len(), 1, "fill only");
    }

    /// The vertex shader's rule in a flat view (`mapgpu.wgsl`, `vs_mesh`): the centerline pushed out along the normal by the
    /// half width, held to the vertex's `reach`.
    fn displaced(v: &MeshVertex, half: f32) -> [f32; 2] {
        let h = half.min(v.reach);
        [v.position[0] + v.normal[0] * h, v.position[1] + v.normal[1] * h]
    }

    fn dist_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
        let (ab, ap) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
        let len2 = ab[0] * ab[0] + ab[1] * ab[1];
        let t = if len2 > 0.0 { ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0) } else { 0.0 };
        (p[0] - (a[0] + t * ab[0])).hypot(p[1] - (a[1] + t * ab[1]))
    }

    fn dist_to_polyline(p: [f32; 2], line: &[[f32; 2]], closed: bool) -> f32 {
        let n = line.len();
        let edges = if closed { n } else { n - 1 };
        (0..edges).map(|i| dist_to_segment(p, line[i], line[(i + 1) % n])).fold(f32::INFINITY, f32::min)
    }

    /// A coastline's worth of jagged points: segments of `0.3 * scale` to `3 * scale`, turns of any angle, from a fixed seed.
    fn jagged(n: usize, scale: f64) -> Vec<Coord> {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let (mut x, mut y, mut heading) = (0.0f64, 0.0f64, 0.0f64);
        (0..n)
            .map(|_| {
                let c = Coord::new(x, y);
                heading += (next() - 0.5) * 2.0 * std::f64::consts::PI * 0.97;
                let step = scale * (0.3 + 2.7 * next());
                x += heading.cos() * step;
                y += heading.sin() * step;
                c
            })
            .collect()
    }

    fn at(pts: &[Coord]) -> Vec<[f32; 2]> {
        pts.iter().map(|c| [c.x as f32, c.y as f32]).collect()
    }

    /// The farthest a displaced vertex lies from the line, in half widths.
    fn worst_overshoot(v: &[MeshVertex], line: &[[f32; 2]], closed: bool, half: f32) -> f32 {
        v.iter().map(|x| dist_to_polyline(displaced(x, half), line, closed) / half).fold(0.0, f32::max)
    }

    // The tessellation is made at width 1.0 (half a unit), the shader draws at the real half width. A coastline is the case
    // where they differ by orders of magnitude: segments of tens of metres (long beside the 1.0 of the tessellation, so lyon
    // keeps the inner corner of a turn) under a stroke of a pixel, thousands of metres. The tests below put the segments at
    // 40 units and the real half width at 10: one pixel of 20 units, a segment two pixels long.

    #[test]
    fn a_hairpin_turn_does_not_throw_a_spike_out_of_the_stroke() {
        // Out forty units, back along almost the same line (a turn of 178 degrees). Lyon's inner corner is 57 half widths
        // out along the line; at the real half width of 10 that is 570 units past the start of a segment of 40.
        let pts = vec![Coord::new(0.0, 0.0), Coord::new(40.0, 0.0), Coord::new(0.0, 1.4)];
        let (v, i) = stroke_lines(std::slice::from_ref(&pts), [0.0, 0.0], [0.0; 4], 1.0, 0.0).expect("stroke").expect("non-empty");
        assert!(!i.is_empty());
        let corner = v.iter().map(|x| x.normal[0].hypot(x.normal[1])).fold(0.0, f32::max);
        assert!(corner > 50.0, "the inner corner is the long normal this test is about: {corner}");
        let held = v.iter().filter(|x| x.reach < f32::MAX).count();
        assert!(held >= 1, "the long normal is held back");
        let over = worst_overshoot(&v, &at(&pts), false, 10.0);
        assert!(over < 1.001, "every vertex stays within the half width of the line: {over} half widths out");
    }

    #[test]
    fn a_jagged_coast_has_no_spikes_at_any_width() {
        let pts = jagged(600, 20.0);
        let line = at(&pts);
        for closed in [false, true] {
            for half in [2.0f32, 10.0, 40.0, 200.0] {
                let parts = vec![pts.clone()];
                let (v, _) = if closed {
                    stroke_rings(&[parts], [0.0, 0.0], [0.0; 4], 1.0, 0.0)
                } else {
                    stroke_lines(&parts, [0.0, 0.0], [0.0; 4], 1.0, 0.0)
                }
                .expect("stroke")
                .expect("non-empty");
                assert!(v.iter().any(|x| x.normal[0].hypot(x.normal[1]) > 3.0), "the coast has sharp inner corners to hold back");
                let over = worst_overshoot(&v, &line, closed, half);
                assert!(over < 1.001, "half width {half}, closed {closed}: a vertex {over} half widths from the line");
            }
        }
    }

    #[test]
    fn a_held_back_corner_leaves_the_stroke_whole() {
        // The corner is pulled in, not cut away: the points of the true stroke (the line's neighbourhood at the real half width
        // of 10, less a margin) are still covered by the triangles the shader would draw. What the pull-in gives up is the
        // sliver between the two legs of the hairpin (1.4 units wide in a stroke 20 wide), which the legs' outer sides cover
        // from the other side anyway.
        let pts = vec![Coord::new(0.0, 0.0), Coord::new(40.0, 0.0), Coord::new(0.0, 1.4), Coord::new(30.0, 6.0)];
        let (v, i) = stroke_lines(std::slice::from_ref(&pts), [0.0, 0.0], [0.0; 4], 1.0, 0.0).expect("stroke").expect("non-empty");
        let line = at(&pts);
        let flat: Vec<MeshVertex> = v.iter().map(|x| MeshVertex { position: displaced(x, 10.0), ..*x }).collect();
        let (mut inside, mut covered) = (0, 0);
        for gx in -40..=120 {
            for gy in -40..=52 {
                let p = [gx as f32 * 0.5 + 0.13, gy as f32 * 0.5 + 0.07];
                if dist_to_polyline(p, &line, false) <= 8.0 {
                    inside += 1;
                    covered += point_covered(&flat, &i, p[0], p[1]) as usize;
                }
            }
        }
        assert!(inside > 2000, "the sample holds the stroke: {inside}");
        assert!(covered as f32 >= inside as f32 * 0.85, "only {covered} of {inside} points of the stroke are covered");
    }

    #[test]
    fn a_bend_with_long_segments_is_left_alone() {
        // A right angle with segments of 100 at half a unit: the offset lines meet 0.7 out, nowhere near the ends.
        let pts = vec![Coord::new(0.0, 0.0), Coord::new(100.0, 0.0), Coord::new(100.0, 100.0), Coord::new(0.0, 100.0)];
        for (v, _) in [
            stroke_lines(std::slice::from_ref(&pts), [0.0, 0.0], [0.0; 4], 1.0, 0.0).expect("stroke").expect("non-empty"),
            stroke_rings(&[vec![pts.clone()]], [0.0, 0.0], [0.0; 4], 1.0, 0.0).expect("stroke").expect("non-empty"),
        ] {
            assert!(v.iter().any(|x| x.normal[0].hypot(x.normal[1]) > 1.3), "the corner's inner vertex is in the mesh");
            for x in &v {
                assert_eq!(x.normal[0].hypot(x.normal[1]) > 1.3, x.reach < f32::MAX, "only a long normal is held: {x:?}");
                assert!(x.reach > 20.0, "held far above the half width of 0.5: {}", x.reach);
            }
        }
    }

    #[test]
    fn inner_reach_is_the_half_width_the_neighbours_can_hold() {
        let line = [[0.0, 0.0], [2.0, 0.0], [0.0, 0.07]];
        assert_eq!(inner_reach(&line, 1, false, 1.0), NO_REACH, "a unit normal (an outer join, a cap) has no limit");
        assert_eq!(inner_reach(&line, 0, false, 9.0), NO_REACH, "the end of an open line has no join");
        assert_eq!(inner_reach(&line, 2, false, 9.0), NO_REACH);
        assert_eq!(inner_reach(&line, 7, false, 9.0), NO_REACH, "an id past the points is not a join");
        // normal 5 (squared 25): tan(turn / 2) = sqrt(24); the shorter segment is 2.0 (the other 2.0006): 0.5 * 2.0 / 4.9.
        let r = inner_reach(&line, 1, false, 25.0);
        assert!((r - 1.0 / 24.0f32.sqrt()).abs() < 1e-3, "{r}");
        // A ring's first point has the last as its previous one.
        let ring = [[0.0, 0.0], [10.0, 0.0], [10.0, 1.0]];
        let r = inner_reach(&ring, 0, true, 2.0);
        assert!((r - 5.0).abs() < 1e-3, "the closing segment (10, 1) to (0, 0) is 10.05 and the first is 10; tan(turn / 2) is 1: {r}");
    }

    #[test]
    fn premultiplies_alpha() {
        assert_eq!(premul(Rgba::rgb(255, 0, 0).with_alpha(128)), [128.0 / 255.0, 0.0, 0.0, 128.0 / 255.0]);
    }
}
