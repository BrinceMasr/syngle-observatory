//! `gov stats find <words>` and `gov stats get <dataflow> [--filter DIM=VAL]... [--from P] [--to P] [--last N]`.

use crate::abs::{Flow, NOT_DIMENSIONS, Table};
use crate::envelope::Answer;
use crate::error::{GovError, OrDamaged, Res};
use crate::json::{echo, js_number, key, number, quote};
use crate::norm::normalise;
use crate::snap::Snap;
use std::collections::BTreeMap;

pub struct Get<'a> {
    pub flow: &'a str,
    pub filters: Vec<&'a str>,
    pub from: Option<&'a str>,
    pub to: Option<&'a str>,
    pub last: Option<&'a str>,
}

fn flows(snap: &Snap) -> Res<Vec<Flow>> {
    let bytes = std::fs::read(snap.dir.join("abs.flows")).damaged()?;
    serde_json::from_slice(&bytes).map_err(GovError::damaged)
}

fn flow_json(out: &mut String, f: &Flow) {
    out.push('{');
    let mut first = true;
    for (name, value) in [("id", &f.id), ("version", &f.version), ("name", &f.name)] {
        key(out, &mut first, name);
        quote(out, value);
    }
    out.push('}');
}

/// `stats find`: the dataflows in which every word is the start of a word of the id or the name; `count` is all of them and
/// `results` the first 50, in the catalogue's order.
pub fn find(snap: &Snap, words: &str) -> Res<Answer> {
    let mut query = String::from("{\"verb\":\"stats\",\"text\":");
    quote(&mut query, &echo(words));
    query.push('}');
    let tokens: Vec<String> = normalise(words).split(' ').filter(|t| !t.is_empty()).map(str::to_string).collect();
    if tokens.is_empty() {
        return Err(GovError::new("empty-query", "give words to search the dataflows for").with_query(query));
    }
    snap.need("abs", "gov stats")?;
    let hits: Vec<Flow> = flows(snap)?
        .into_iter()
        .filter(|f| {
            let text = normalise(&format!("{} {}", f.id, f.name));
            tokens.iter().all(|t| text.split(' ').any(|w| w.starts_with(t.as_str())))
        })
        .collect();
    let mut results = String::new();
    for (i, f) in hits.iter().take(50).enumerate() {
        if i > 0 {
            results.push(',');
        }
        flow_json(&mut results, f);
    }
    Ok(Answer { used: &["abs"], query, count: hits.len(), results })
}

/// `--last` as the contract reads it: a number of at least 1.
fn last_of(text: &str) -> Option<f64> {
    js_number(text).filter(|n| *n >= 1.0)
}

pub fn get(snap: &Snap, a: &Get<'_>) -> Res<Answer> {
    let mut query = String::from("{\"verb\":\"stats\",\"dataflow\":");
    quote(&mut query, &echo(a.flow));
    query.push('}');
    snap.need("abs", "gov stats")?;
    let cat: BTreeMap<String, (u64, u64)> =
        serde_json::from_slice(&std::fs::read(snap.dir.join("abs.cat")).damaged()?).map_err(GovError::damaged)?;
    let Some(&(offset, len)) = cat.get(a.flow) else {
        return Err(GovError::new("unknown-dataflow", format!("no data for {} in this index", echo(a.flow))).with_query(query));
    };
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for f in &a.filters {
        let mut parts = f.split('=');
        match (parts.next().filter(|k| !k.is_empty()), parts.next()) {
            (Some(k), Some(v)) => pairs.push((k, v)),
            _ => return Err(GovError::new("invalid-filter", "--filter takes DIMENSION=VALUE").with_query(query)),
        }
    }
    let last = match a.last {
        None => None,
        Some(text) => match last_of(text) {
            Some(n) => Some(n.floor()),
            None => return Err(GovError::new("invalid-limit", "--last must be a positive number").with_query(query)),
        },
    };
    let blob = snap.blob("abs.dat")?;
    if offset.checked_add(len).is_none_or(|end| end > blob.len) {
        return Err(GovError::damaged("an observation block is outside its file"));
    }
    let bytes = blob.read_vec(offset, usize::try_from(len).map_err(|_| GovError::damaged("a block is too long"))?).damaged()?;
    let t = Table::open(&bytes)?;

    // A filter names a column and a value; a column the table lacks, or a value it never holds, matches nothing.
    let mut wanted: Vec<(usize, u32)> = Vec::new();
    let mut possible = true;
    for (k, v) in pairs {
        match t.names.iter().rposition(|n| *n == k).and_then(|c| t.find(c, v).map(|id| (c, id))) {
            Some(w) => wanted.push(w),
            None => possible = false,
        }
    }
    let lo = a.from.filter(|f| !f.is_empty()).map_or(0, |f| t.first_row(f, false));
    let hi = a.to.filter(|f| !f.is_empty()).map_or(t.nrows, |f| t.first_row(f, true));
    let mut rows: Vec<usize> =
        if possible { (lo..hi).filter(|&r| wanted.iter().all(|&(c, id)| t.cell(r, c) == id)).collect() } else { Vec::new() };
    if let Some(n) = last {
        let keep = if n >= rows.len() as f64 { rows.len() } else { n as usize };
        rows.drain(..rows.len() - keep);
    }

    let value_col = t.names.iter().rposition(|n| *n == "OBS_VALUE");
    let unit_col = t.names.iter().rposition(|n| *n == "UNIT_MEASURE");
    let dims: Vec<usize> = (0..t.ncols()).filter(|&c| !NOT_DIMENSIONS.contains(&t.names[c])).collect();
    let mut results = String::new();
    for (i, &r) in rows.iter().enumerate() {
        if i > 0 {
            results.push(',');
        }
        results.push_str("{\"period\":");
        quote(&mut results, t.text(t.period, t.cell(r, t.period)));
        results.push_str(",\"value\":");
        // An observation with no value is `null`, never 0 (the reference reads a blank cell as 0).
        let raw = value_col.map_or("", |c| t.text(c, t.cell(r, c))).trim();
        match js_number(raw).filter(|_| !raw.is_empty()) {
            Some(v) => number(&mut results, v),
            None => results.push_str("null"),
        }
        if let Some(c) = unit_col {
            results.push_str(",\"unit\":");
            quote(&mut results, t.text(c, t.cell(r, c)));
        }
        results.push_str(",\"dims\":{");
        for (j, &c) in dims.iter().enumerate() {
            if j > 0 {
                results.push(',');
            }
            quote(&mut results, t.names[c]);
            results.push(':');
            quote(&mut results, t.text(c, t.cell(r, c)));
        }
        results.push_str("}}");
    }
    Ok(Answer { used: &["abs"], query, count: rows.len(), results })
}
