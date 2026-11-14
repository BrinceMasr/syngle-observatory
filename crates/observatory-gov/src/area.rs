//! `gov area --lat Y --lon X`: the ASGS areas that contain a point.

use crate::asgs::{BIG, ENTRY, cell_id, cell_x, cell_y};
use crate::codec::Cur;
use crate::envelope::Answer;
use crate::error::{GovError, OrDamaged, Res};
use crate::geoms::inside;
use crate::json::{echo, js_number, key, opt, quote};
use crate::snap::Snap;
use crate::table::{Fixed, le32, le64};

struct Hit {
    code: String,
    number: u32,
    layer: String,
    name: String,
    state: Option<String>,
}

/// Whether the feature's record holds a polygon with the point in its outer ring and in none of its holes.
fn contains(payload: &[u8], number: u32, lat: f64, lon: f64) -> Option<Hit> {
    let mut c = Cur::new(payload);
    let (layer, code, name) = (c.str()?, c.str()?, c.str()?);
    let has_state = c.u8()? == 1;
    let state = c.str().filter(|_| has_state);
    let mut hit = false;
    for _ in 0..c.varint()? {
        let mut in_outer = false;
        let mut in_hole = false;
        for ring in 0..c.varint()? {
            let n = usize::try_from(c.varint()?).ok()?;
            let bytes = c.take(n.checked_mul(16)?)?;
            if hit || (ring > 0 && !in_outer) {
                continue;
            }
            let pts: Vec<[f64; 2]> =
                (0..n).map(|i| [f64::from_bits(le64(bytes, i * 16)), f64::from_bits(le64(bytes, i * 16 + 8))]).collect();
            if ring == 0 {
                in_outer = inside(&pts, lat, lon);
            } else if inside(&pts, lat, lon) {
                in_hole = true;
            }
        }
        hit = hit || (in_outer && !in_hole);
    }
    hit.then_some(Hit { code, number, layer, name, state })
}

pub fn area(snap: &Snap, lat_text: Option<&str>, lon_text: Option<&str>) -> Res<Answer> {
    let mut query = String::from("{\"verb\":\"area\"");
    for (name, text) in [("lat", lat_text), ("lon", lon_text)] {
        if let Some(t) = text {
            query.push(',');
            quote(&mut query, name);
            query.push(':');
            quote(&mut query, &echo(t));
        }
    }
    query.push('}');
    let number = |t: Option<&str>| t.and_then(js_number).filter(|n| n.is_finite());
    let (Some(lat), Some(lon)) = (number(lat_text), number(lon_text)) else {
        return Err(GovError::new("invalid-coordinates", "latitude within 90 and longitude within 180").with_query(query));
    };
    if lat.abs() > 90.0 || lon.abs() > 180.0 {
        return Err(GovError::new("invalid-coordinates", "latitude within 90 and longitude within 180").with_query(query));
    }
    snap.need("asgs", "gov area")?;
    let cells: Fixed = snap.fixed("area.cell", ENTRY)?;
    let (idx, rec) = (snap.fixed("area.idx", 8)?, snap.blob("area.rec")?);
    let mut found: Vec<Hit> = Vec::new();
    for cell in [cell_id(cell_x(lon), cell_y(lat)), BIG] {
        let lo = cells.partition_point(0, |e| Ok(le32(e, 0) < cell)).damaged()?;
        let hi = cells.partition_point(lo, |e| Ok(le32(e, 0) <= cell)).damaged()?;
        let block = cells.block(lo, hi - lo).damaged()?;
        for e in block.chunks_exact(ENTRY as usize) {
            let b = |i: usize| f64::from_bits(le64(e, 8 + i * 8));
            if lon < b(0) || lat < b(1) || lon > b(2) || lat > b(3) {
                continue;
            }
            let number = le32(e, 4);
            let offset = le64(&idx.entry(u64::from(number)).damaged()?, 0);
            let payload = rec.record(offset).damaged()?;
            if let Some(h) = contains(&payload, number, lat, lon) {
                found.push(h);
            }
        }
    }
    found.sort_by(|a, b| (&a.code, a.number).cmp(&(&b.code, b.number)));
    let mut results = String::new();
    for (i, h) in found.iter().enumerate() {
        if i > 0 {
            results.push(',');
        }
        results.push('{');
        let mut f = true;
        key(&mut results, &mut f, "layer");
        quote(&mut results, &h.layer);
        key(&mut results, &mut f, "code");
        quote(&mut results, &h.code);
        key(&mut results, &mut f, "name");
        quote(&mut results, &h.name);
        key(&mut results, &mut f, "state");
        opt(&mut results, h.state.as_deref());
        results.push('}');
    }
    Ok(Answer { used: &["asgs"], query, count: found.len(), results })
}
