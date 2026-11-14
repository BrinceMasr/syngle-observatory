//! ASGS boundaries as an index of areas by point. Every feature is one record (`area.rec`, offsets in `area.idx`):
//!
//! ```text
//! layer, code, name (strings), a flag byte and the state name when it is set,
//! varint polygons; per polygon: varint rings; per ring: varint points, then the points as little-endian f64 x, y
//! ```
//!
//! and one entry per grid cell its bounding box touches in `area.cell` (40 bytes: cell, feature, minx, miny, maxx, maxy; sorted
//! by cell then feature), so a point is found by two binary searches, a bounding box test on the entries of its cell, and the
//! even-odd test on the rings of the few features left. A feature whose box touches more than `MAX_CELLS` cells (a state, a
//! country) is entered once under `BIG`, which every query reads.

use crate::codec::{put_str, put_varint};
use crate::geoms::{Feature, each_feature};
use crate::rd::invalid;
use crate::sort::Sorter;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Grid cell size in degrees.
pub const CELL: f64 = 0.1;
const COLUMNS: u32 = 3600;
const ROWS: u32 = 1800;
const MAX_CELLS: u64 = 4096;
pub const BIG: u32 = u32::MAX;
pub const ENTRY: u64 = 40;

pub fn cell_x(lon: f64) -> u32 {
    (((lon + 180.0) / CELL).floor().max(0.0) as u32).min(COLUMNS - 1)
}

pub fn cell_y(lat: f64) -> u32 {
    (((lat + 90.0) / CELL).floor().max(0.0) as u32).min(ROWS - 1)
}

pub fn cell_id(x: u32, y: u32) -> u32 {
    y * COLUMNS + x
}

/// The code, name and state of a feature. A layer's own keys are `<LAYER>_CODE...` and `<LAYER>_NAME...` (`STATE_` for `STE`,
/// as the ABS names it; a boundary file's keys may be in any case). A layer with no own code key (a file with other names)
/// takes the first key with `_CODE` in it, and the first with `_NAME` that is not a state's, as the reference reads them;
/// a layer with a code but no name (a mesh block) has no name. The state is the first `STE_NAME` or `STATE_NAME` key.
pub fn pick(layer: &str, props: &[(String, String)]) -> (String, String, Option<String>) {
    let has = |k: &str, part: &str| k.to_ascii_uppercase().contains(part);
    let own = |what: &str| -> Option<&(String, String)> {
        let groups: &[&str] = if layer.eq_ignore_ascii_case("STE") { &["STE", "STATE"] } else { &[layer] };
        groups
            .iter()
            .find_map(|g| props.iter().find(|(k, _)| k.to_ascii_uppercase().starts_with(&format!("{}_{what}", g.to_ascii_uppercase()))))
    };
    let value = |p: Option<&(String, String)>| p.map(|(_, v)| v.clone()).unwrap_or_default();
    let (code, name) = match own("CODE") {
        Some(code) => (code.1.clone(), value(own("NAME"))),
        None => (
            value(props.iter().find(|(k, _)| has(k, "_CODE"))),
            value(props.iter().find(|(k, _)| has(k, "_NAME") && !has(k, "STE_") && !has(k, "STATE_"))),
        ),
    };
    let state = props.iter().find(|(k, _)| has(k, "STE_NAME") || has(k, "STATE_NAME")).map(|(_, v)| v.clone());
    (code, name, state)
}

fn encode(ft: &Feature, code: &str, name: &str, state: Option<&str>, out: &mut Vec<u8>) {
    put_str(out, &ft.layer);
    put_str(out, code);
    put_str(out, name);
    out.push(u8::from(state.is_some()));
    put_str(out, state.unwrap_or(""));
    put_varint(out, ft.polys.len() as u64);
    for poly in &ft.polys {
        put_varint(out, poly.len() as u64);
        for ring in poly {
            put_varint(out, ring.len() as u64);
            for [x, y] in ring {
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
            }
        }
    }
}

/// Writes `area.rec`, `area.idx` and `area.cell` into `dir`; the number of features.
pub fn build(files: &[PathBuf], dir: &Path, sort_dir: &Path, run_bytes: usize) -> io::Result<u64> {
    let open = |name: &str| File::create(dir.join(name)).map(|f| BufWriter::with_capacity(1 << 20, f));
    let (mut rec, mut idx) = (open("area.rec")?, open("area.idx")?);
    let mut cells = Sorter::new(sort_dir, "cell", run_bytes);
    let (mut count, mut pos) = (0u32, 0u64);
    let (mut payload, mut entry) = (Vec::new(), Vec::new());
    each_feature(files, &sort_dir.join("asgs"), &mut |ft| {
        let (code, name, state) = pick(&ft.layer, &ft.props);
        payload.clear();
        encode(&ft, &code, &name, state.as_deref(), &mut payload);
        idx.write_all(&pos.to_le_bytes())?;
        rec.write_all(&u32::try_from(payload.len()).map_err(|_| invalid("an area is larger than the format allows"))?.to_le_bytes())?;
        rec.write_all(&payload)?;
        pos += 4 + payload.len() as u64;
        let points = ft.polys.iter().flatten().flatten();
        let bbox = points.fold(None, |b: Option<[f64; 4]>, [x, y]| {
            Some(b.map_or([*x, *y, *x, *y], |b| [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)]))
        });
        if let Some(b) = bbox {
            let (x0, x1, y0, y1) = (cell_x(b[0]), cell_x(b[2]), cell_y(b[1]), cell_y(b[3]));
            let wanted = u64::from(x1 - x0 + 1) * u64::from(y1 - y0 + 1);
            let mut put = |cell: u32| -> io::Result<()> {
                entry.clear();
                entry.extend_from_slice(&cell.to_be_bytes());
                entry.extend_from_slice(&count.to_be_bytes());
                for v in b {
                    entry.extend_from_slice(&v.to_le_bytes());
                }
                cells.push(&entry)
            };
            if wanted > MAX_CELLS {
                put(BIG)?;
            } else {
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        put(cell_id(x, y))?;
                    }
                }
            }
        }
        count = count.checked_add(1).ok_or_else(|| invalid("more areas than the index holds"))?;
        Ok(())
    })?;
    rec.flush()?;
    idx.flush()?;
    let mut out = open("area.cell")?;
    cells.for_each(|r| {
        let (cell, feature, bbox) = (r.get(0..4).unwrap_or_default(), r.get(4..8).unwrap_or_default(), r.get(8..40).unwrap_or_default());
        let le = |b: &[u8]| {
            <[u8; 4]>::try_from(b).map(|a| u32::from_be_bytes(a).to_le_bytes()).map_err(|_| invalid("a cell entry of the wrong size"))
        };
        out.write_all(&le(cell)?)?;
        out.write_all(&le(feature)?)?;
        out.write_all(bbox)
    })?;
    out.flush()?;
    Ok(u64::from(count))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn code_name_and_state_follow_the_layer() {
        let sa2 = props(&[("SA2_CODE21", "1"), ("SA2_NAME21", "Sydney"), ("SA3_CODE21", "9"), ("STE_NAME21", "New South Wales")]);
        assert_eq!(pick("SA2", &sa2), ("1".into(), "Sydney".into(), Some("New South Wales".into())));
        let mb = props(&[("MB_CODE_2021", "5"), ("SA2_NAME_2021", "Elsewhere")]);
        assert_eq!(pick("MB", &mb), ("5".into(), String::new(), None));
        let ste = props(&[("STE_CODE21", "1"), ("STE_NAME21", "Victoria")]);
        assert_eq!(pick("STE", &ste), ("1".into(), "Victoria".into(), Some("Victoria".into())));
        let state =
            props(&[("STATE_CODE_2021", "2"), ("STATE_NAME_2021", "Victoria"), ("AUS_CODE_2021", "AUS"), ("AUS_NAME_2021", "Australia")]);
        assert_eq!(pick("STE", &state), ("2".into(), "Victoria".into(), Some("Victoria".into())));
        let aus = props(&[("aus_code_2021", "AUS"), ("aus_name_2021", "Australia")]);
        assert_eq!(pick("AUS", &aus), ("AUS".into(), "Australia".into(), None));
        let loose = props(&[("CODE_X_CODE", "7"), ("ZONE_NAME", "Z")]);
        assert_eq!(pick("SA2", &loose), ("7".into(), "Z".into(), None));
        assert_eq!(pick("X", &[]), (String::new(), String::new(), None));
    }

    #[test]
    fn cells_are_monotone_and_clamped() {
        assert!(
            cell_x(151.2) >= cell_x(151.1)
                && cell_x(151.2) > cell_x(150.0)
                && cell_x(-1000.0) == 0
                && cell_x(1000.0) == COLUMNS - 1
                && cell_y(95.0) == ROWS - 1
        );
    }
}
