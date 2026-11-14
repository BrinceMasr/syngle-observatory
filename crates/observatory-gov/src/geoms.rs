//! The readers of the ASGS boundary files: GeoJSON, Shapefile (loose, or in the ABS's ZIP) and GeoPackage (SQLite, with the
//! geometry as a GeoPackage blob of WKB). Each feature is handed over as its layer, its attributes in the file's order and
//! its polygons (a polygon is its rings, the first the outer one and the rest holes); other kinds of geometry have no polygons.

use crate::rd::invalid;
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

pub type Ring = Vec<[f64; 2]>;
pub type Polygon = Vec<Ring>;

pub struct Feature {
    pub layer: String,
    pub props: Vec<(String, String)>,
    pub polys: Vec<Polygon>,
}

const MAX_FILE: u64 = 4 << 30;
const MAX_RINGS: usize = 1 << 22;

/// `SA2_2021_AUST_GDA2020` as `SA2`; `AREA` when the name has no first word.
pub fn layer_of(name: &str) -> String {
    let stem = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match stem.split_once('_') {
        Some((first, _)) if !first.is_empty() && first.bytes().all(|b| b.is_ascii_alphanumeric()) => first.to_string(),
        _ => "AREA".to_string(),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn point(v: &Value) -> io::Result<[f64; 2]> {
    let xy = v.as_array().filter(|a| a.len() >= 2).and_then(|a| Some([a[0].as_f64()?, a[1].as_f64()?]));
    xy.filter(|p| p.iter().all(|c| c.is_finite())).ok_or_else(|| invalid("a coordinate that is not a pair of numbers"))
}

fn polygon(v: &Value) -> io::Result<Polygon> {
    v.as_array()
        .ok_or_else(|| invalid("polygon coordinates that are not a list of rings"))?
        .iter()
        .map(|r| {
            r.as_array().ok_or_else(|| invalid("a ring that is not a list of points"))?.iter().map(point).collect::<io::Result<Ring>>()
        })
        .collect()
}

fn geojson_feature(f: &Value, layer: &str) -> io::Result<Feature> {
    let props = f
        .get("properties")
        .and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, v)| (k.clone(), text_of(v))).collect())
        .unwrap_or_default();
    let geometry = f.get("geometry").unwrap_or(&Value::Null);
    let coords = geometry.get("coordinates");
    let polys = match (geometry.get("type").and_then(Value::as_str), coords) {
        (Some("Polygon"), Some(c)) => vec![polygon(c)?],
        (Some("MultiPolygon"), Some(c)) => c
            .as_array()
            .ok_or_else(|| invalid("MultiPolygon coordinates that are not a list"))?
            .iter()
            .map(polygon)
            .collect::<io::Result<_>>()?,
        _ => Vec::new(),
    };
    Ok(Feature { layer: layer.to_string(), props, polys })
}

fn geojson(path: &Path, f: &mut dyn FnMut(Feature) -> io::Result<()>) -> io::Result<()> {
    let mut bytes = Vec::new();
    File::open(path)?.take(MAX_FILE).read_to_end(&mut bytes)?;
    let root: Value = serde_json::from_slice(&bytes).map_err(|e| invalid(format!("{}: not GeoJSON: {e}", path.display())))?;
    let layer = layer_of(&path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    match root.get("features").and_then(Value::as_array) {
        Some(list) => list.iter().try_for_each(|ft| f(geojson_feature(ft, &layer)?)),
        None if root.get("type").and_then(Value::as_str) == Some("Feature") => f(geojson_feature(&root, &layer)?),
        None => Err(invalid(format!("{}: GeoJSON with no features", path.display()))),
    }
}

/// The polygons of a shapefile record: each outer ring starts one, each hole goes to the polygon that contains it.
fn shape_polys<P: Copy>(rings: &[shapefile::PolygonRing<P>], xy: impl Fn(&P) -> [f64; 2]) -> Vec<Polygon> {
    let mut polys: Vec<Polygon> = Vec::new();
    for ring in rings {
        let pts: Ring = ring.points().iter().map(&xy).collect();
        match ring {
            shapefile::PolygonRing::Outer(_) => polys.push(vec![pts]),
            shapefile::PolygonRing::Inner(_) => {
                let probe = pts.first().copied();
                let home = probe.and_then(|p| polys.iter().position(|poly| poly.first().is_some_and(|outer| inside(outer, p[1], p[0]))));
                match home.or(polys.len().checked_sub(1)).and_then(|i| polys.get_mut(i)) {
                    Some(poly) => poly.push(pts),
                    None => polys.push(vec![pts]),
                }
            }
        }
    }
    polys
}

/// The reference's point in a ring: the even-odd rule, with its exact arithmetic.
pub fn inside(ring: &[[f64; 2]], lat: f64, lon: f64) -> bool {
    let mut inside = false;
    let mut j = ring.len().wrapping_sub(1);
    for i in 0..ring.len() {
        let ([xi, yi], [xj, yj]) = (ring[i], ring[j]);
        if (yi > lat) != (yj > lat) && lon < ((xj - xi) * (lat - yi)) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn shapefile_features(path: &Path, f: &mut dyn FnMut(Feature) -> io::Result<()>) -> io::Result<()> {
    let layer = layer_of(&path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let mut reader = shapefile::Reader::from_path(path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    for item in reader.iter_shapes_and_records() {
        let (shape, record) = item.map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        let polys = match &shape {
            shapefile::Shape::Polygon(p) => shape_polys(p.rings(), |q| [q.x, q.y]),
            shapefile::Shape::PolygonM(p) => shape_polys(p.rings(), |q| [q.x, q.y]),
            shapefile::Shape::PolygonZ(p) => shape_polys(p.rings(), |q| [q.x, q.y]),
            _ => Vec::new(),
        };
        let props = record
            .into_iter()
            .map(|(k, v)| {
                use shapefile::dbase::FieldValue as V;
                let text = match v {
                    V::Character(s) => s.map(|s| s.trim().to_string()).unwrap_or_default(),
                    V::Numeric(n) => n
                        .map(|n| if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { n.to_string() })
                        .unwrap_or_default(),
                    V::Integer(n) => n.to_string(),
                    V::Double(n) | V::Currency(n) => n.to_string(),
                    V::Float(n) => n.map(|n| n.to_string()).unwrap_or_default(),
                    V::Logical(b) => b.map(|b| b.to_string()).unwrap_or_default(),
                    other => format!("{other:?}"),
                };
                (k, text)
            })
            .collect();
        f(Feature { layer: layer.clone(), props, polys })?;
    }
    Ok(())
}

/// The rings of the WKB geometry in `b` (a polygon or a multipolygon, with or without Z and M); `None` for anything else.
pub fn wkb_polygons(b: &[u8]) -> Option<Vec<Polygon>> {
    struct R<'a> {
        b: &'a [u8],
        at: usize,
    }
    impl R<'_> {
        fn u32(&mut self, little: bool) -> Option<u32> {
            let a: [u8; 4] = self.b.get(self.at..self.at + 4)?.try_into().ok()?;
            self.at += 4;
            Some(if little { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
        }
        fn f64(&mut self, little: bool) -> Option<f64> {
            let a: [u8; 8] = self.b.get(self.at..self.at + 8)?.try_into().ok()?;
            self.at += 8;
            Some(if little { f64::from_le_bytes(a) } else { f64::from_be_bytes(a) })
        }
        fn header(&mut self) -> Option<(bool, u32, usize)> {
            let little = *self.b.get(self.at)? == 1;
            self.at += 1;
            let raw = self.u32(little)?;
            let (kind, dims) = if raw & 0xE000_0000 != 0 {
                (raw & 0xFF, 2 + usize::from(raw & 0x8000_0000 != 0) + usize::from(raw & 0x4000_0000 != 0))
            } else {
                (raw % 1000, 2 + usize::from(matches!(raw / 1000, 1 | 3)) + usize::from(matches!(raw / 1000, 2 | 3)))
            };
            if raw & 0x2000_0000 != 0 {
                self.u32(little)?;
            }
            Some((little, kind, dims))
        }
        fn polygon(&mut self, little: bool, dims: usize) -> Option<Polygon> {
            let rings = self.u32(little)? as usize;
            if rings > MAX_RINGS {
                return None;
            }
            let mut out = Vec::new();
            for _ in 0..rings {
                let n = self.u32(little)? as usize;
                if n.checked_mul(dims * 8).is_none_or(|bytes| self.at + bytes > self.b.len()) {
                    return None;
                }
                let mut ring = Vec::with_capacity(n);
                for _ in 0..n {
                    let (x, y) = (self.f64(little)?, self.f64(little)?);
                    self.at += (dims - 2) * 8;
                    ring.push([x, y]);
                }
                out.push(ring);
            }
            Some(out)
        }
    }
    let mut r = R { b, at: 0 };
    let (little, kind, dims) = r.header()?;
    match kind {
        3 => Some(vec![r.polygon(little, dims)?]),
        6 => {
            let n = r.u32(little)? as usize;
            let mut polys = Vec::new();
            for _ in 0..n.min(MAX_RINGS) {
                let (l, k, d) = r.header()?;
                if k != 3 {
                    return None;
                }
                polys.push(r.polygon(l, d)?);
            }
            Some(polys)
        }
        _ => None,
    }
}

/// The WKB inside a GeoPackage geometry blob (or the blob itself when it is plain WKB).
fn gpkg_wkb(b: &[u8]) -> Option<&[u8]> {
    if !b.starts_with(b"GP") {
        return Some(b);
    }
    let flags = *b.get(3)?;
    if flags & 0b1_0000 != 0 {
        return None;
    }
    let env = match (flags >> 1) & 0b111 {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => return None,
    };
    b.get(8 + env..)
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn gpkg_features(path: &Path, f: &mut dyn FnMut(Feature) -> io::Result<()>) -> io::Result<()> {
    use rusqlite::types::ValueRef;
    let bad = |e: rusqlite::Error| invalid(format!("{}: {e}", path.display()));
    let db = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(bad)?;
    let tables: Vec<(String, String)> = {
        let mut st = db.prepare("SELECT table_name, column_name FROM gpkg_geometry_columns ORDER BY table_name").map_err(bad)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(bad)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(bad)?
    };
    for (table, geom) in tables {
        let mut st = db.prepare(&format!("SELECT * FROM {}", quote_ident(&table))).map_err(bad)?;
        let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        let geom_at = names.iter().position(|n| *n == geom);
        let layer = layer_of(&table);
        let mut rows = st.query([]).map_err(bad)?;
        while let Some(row) = rows.next().map_err(bad)? {
            let (mut props, mut polys) = (Vec::new(), Vec::new());
            for (i, name) in names.iter().enumerate() {
                match row.get_ref(i).map_err(bad)? {
                    ValueRef::Blob(b) if Some(i) == geom_at => polys = gpkg_wkb(b).and_then(wkb_polygons).unwrap_or_default(),
                    ValueRef::Blob(_) | ValueRef::Null => props.push((name.clone(), String::new())),
                    ValueRef::Integer(n) => props.push((name.clone(), n.to_string())),
                    ValueRef::Real(n) => props.push((name.clone(), n.to_string())),
                    ValueRef::Text(t) => props.push((name.clone(), String::from_utf8_lossy(t).trim().to_string())),
                }
            }
            f(Feature { layer: layer.clone(), props, polys })?;
        }
    }
    Ok(())
}

/// Copies the files of a ZIP into `dest` under their own names (folders dropped), and returns the paths in name order.
fn unzip(zip_path: &Path, dest: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dest)?;
    let mut archive =
        zip::ZipArchive::new(BufReader::new(File::open(zip_path)?)).map_err(|e| invalid(format!("{}: {e}", zip_path.display())))?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| invalid(e.to_string()))?;
        let Some(name) = entry.name().rsplit(['/', '\\']).next().filter(|n| !n.is_empty() && *n != "." && *n != "..").map(str::to_string)
        else {
            continue;
        };
        if !entry.is_file() || entry.name().starts_with("__MACOSX") {
            continue;
        }
        let path = dest.join(&name);
        io::copy(&mut (&mut entry).take(MAX_FILE), &mut File::create(&path)?)?;
        out.push(path);
    }
    out.sort();
    Ok(out)
}

/// Calls `f` with every feature of every file: GeoJSON, `.shp` (its `.dbf` beside it), `.gpkg`, and ZIPs of those.
pub fn each_feature(files: &[PathBuf], work: &Path, f: &mut dyn FnMut(Feature) -> io::Result<()>) -> io::Result<()> {
    for (n, file) in files.iter().enumerate() {
        let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "geojson" | "json" => geojson(file, f)?,
            "shp" => shapefile_features(file, f)?,
            "gpkg" => gpkg_features(file, f)?,
            "zip" => {
                let inner = unzip(file, &work.join(format!("z{n}")))?;
                each_feature(&inner, work, f)?;
                std::fs::remove_dir_all(work.join(format!("z{n}")))?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wkb_polygon(rings: &[&[[f64; 2]]], little: bool) -> Vec<u8> {
        let mut b = vec![u8::from(little)];
        let put32 = |b: &mut Vec<u8>, n: u32| b.extend_from_slice(&if little { n.to_le_bytes() } else { n.to_be_bytes() });
        put32(&mut b, 3);
        put32(&mut b, rings.len() as u32);
        for r in rings {
            put32(&mut b, r.len() as u32);
            for p in *r {
                for c in p {
                    b.extend_from_slice(&if little { c.to_le_bytes() } else { c.to_be_bytes() });
                }
            }
        }
        b
    }

    #[test]
    fn layers_from_names() {
        assert_eq!(
            (layer_of("SA2_2021_AUST_GDA2020").as_str(), layer_of("SA2_FIXTURE").as_str(), layer_of("odd").as_str()),
            ("SA2", "SA2", "AREA")
        );
    }

    #[test]
    fn wkb_both_byte_orders_and_garbage() {
        let square: &[[f64; 2]] = &[[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [0.0, 0.0]];
        for little in [true, false] {
            let polys = wkb_polygons(&wkb_polygon(&[square], little)).unwrap();
            assert_eq!(polys.len(), 1);
            assert!(inside(&polys[0][0], 1.0, 1.0) && !inside(&polys[0][0], 3.0, 1.0));
        }
        assert!(
            wkb_polygons(&[]).is_none()
                && wkb_polygons(&[1, 3, 0, 0, 0, 255, 255, 255, 255]).is_none()
                && wkb_polygons(&[1, 1, 0, 0, 0]).is_none()
        );
    }

    #[test]
    fn a_hole_and_a_multipolygon_in_geojson() {
        let f = serde_json::json!({"properties": {"A_CODE": 5}, "geometry": {"type": "MultiPolygon", "coordinates": [[[[0,0],[4,0],[4,4],[0,4],[0,0]],[[1,1],[2,1],[2,2],[1,2],[1,1]]],[[[10,10],[11,10],[11,11],[10,10]]]]}});
        let ft = geojson_feature(&f, "A").unwrap();
        assert_eq!((ft.polys.len(), ft.polys[0].len(), ft.props[0].1.as_str()), (2, 2, "5"));
        assert!(geojson_feature(&serde_json::json!({"geometry": {"type": "Polygon", "coordinates": [[["x", 1]]]}}), "A").is_err());
    }
}
