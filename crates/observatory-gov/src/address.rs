//! `gov address <text> [--limit N]` and `gov address --id <G-NAF PID>`. A query's words (a unit word dropped, `/` a space) must
//! each be a word of the address, or its long form (`ST` for `STREET`); the closest match, the address with the fewest words
//! beyond the query's, comes first, and the PID breaks ties. The words are looked up in `atok.*`; the shortest lists are
//! read, the long ones (`STREET`, `NSW`) are only probed for the few ids left, and the ranking keeps no more than the limit.

use crate::codec::Cur;
use crate::envelope::Answer;
use crate::error::{GovError, OrDamaged, Res};
use crate::json::{echo, key, number, quote};
use crate::norm::normalise;
use crate::snap::Snap;
use crate::table::{Blob, Fixed, le64};
use crate::tokens;

const ABBR: [(&str, &str); 15] = [
    ("ST", "STREET"),
    ("RD", "ROAD"),
    ("AVE", "AVENUE"),
    ("AV", "AVENUE"),
    ("DR", "DRIVE"),
    ("CT", "COURT"),
    ("PDE", "PARADE"),
    ("HWY", "HIGHWAY"),
    ("LN", "LANE"),
    ("PL", "PLACE"),
    ("CL", "CLOSE"),
    ("CRES", "CRESCENT"),
    ("TCE", "TERRACE"),
    ("BLVD", "BOULEVARD"),
    ("SQ", "SQUARE"),
];
const DROP: [&str; 6] = ["UNIT", "FLAT", "APT", "APARTMENT", "SUITE", "SHOP"];
/// A query with more distinct words than this matches no address.
const MAX_WORDS: usize = 40;
const BLOCK: u64 = 256;

struct Tables {
    ent: Fixed,
    rec: Blob,
    street: (Fixed, Blob),
    loc: (Fixed, Blob),
}

fn open(snap: &Snap) -> Res<Tables> {
    Ok(Tables {
        ent: snap.fixed("addr.ent", 12)?,
        rec: snap.blob("addr.rec")?,
        street: (snap.fixed("gn.street.idx", 8)?, snap.blob("gn.street.rec")?),
        loc: (snap.fixed("gn.loc.idx", 8)?, snap.blob("gn.loc.rec")?),
    })
}

fn pair(t: &(Fixed, Blob), i: usize) -> Res<(String, String)> {
    let bad = || GovError::damaged("an address points at a street or locality that is not there");
    let at = le64(&t.0.entry(i as u64).damaged()?, 0);
    let payload = t.1.record(at).damaged()?;
    let mut c = Cur::new(&payload);
    Ok((c.str().ok_or_else(bad)?, c.str().ok_or_else(bad)?))
}

fn join(parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" ")
}

/// One address as the answer gives it.
fn view(snap: &Snap, t: &Tables, record: &[u8]) -> Res<String> {
    let bad = || GovError::damaged("an address record does not decode");
    let mut c = Cur::new(record);
    let (pid, flat_type, flat_no, number_text) =
        (c.str().ok_or_else(bad)?, c.str().ok_or_else(bad)?, c.str().ok_or_else(bad)?, c.str().ok_or_else(bad)?);
    let (street_at, loc_at, state_at) = (c.varint().ok_or_else(bad)?, c.varint().ok_or_else(bad)?, c.varint().ok_or_else(bad)?);
    let postcode = c.str().ok_or_else(bad)?;
    let coords = c.take(16).ok_or_else(bad)?;
    let ((street, kind), (locality, _)) = (pair(&t.street, street_at as usize)?, pair(&t.loc, loc_at as usize)?);
    let state = snap.meta.dicts.states.get(state_at as usize).map_or("", String::as_str);
    let flat = if flat_no.is_empty() {
        String::new()
    } else {
        format!("{}{flat_no}/", if flat_type.is_empty() { String::new() } else { format!("{flat_type} ") })
    };
    let address = format!("{flat}{}, {}", join(&[&number_text, &street, &kind]), join(&[&locality, state, &postcode]));
    let mut o = String::with_capacity(220);
    o.push('{');
    let mut f = true;
    key(&mut o, &mut f, "addressId");
    quote(&mut o, &pid);
    key(&mut o, &mut f, "address");
    quote(&mut o, &address);
    key(&mut o, &mut f, "lat");
    number(&mut o, f64::from_bits(le64(coords, 0)));
    key(&mut o, &mut f, "lon");
    number(&mut o, f64::from_bits(le64(coords, 8)));
    key(&mut o, &mut f, "locality");
    quote(&mut o, &locality);
    key(&mut o, &mut f, "state");
    quote(&mut o, state);
    key(&mut o, &mut f, "postcode");
    quote(&mut o, &postcode);
    o.push('}');
    Ok(o)
}

fn record_of(t: &Tables, id: u64) -> Res<Vec<u8>> {
    t.rec.record(le64(&t.ent.entry(id).damaged()?, 0)).damaged()
}

fn pid_of(record: &[u8]) -> Option<String> {
    Cur::new(record).str()
}

fn union(a: Vec<u32>, b: Vec<u32>) -> Vec<u32> {
    if b.is_empty() {
        return a;
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::with_capacity(a.len() + b.len()));
    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(&x), Some(&y)) if x == y => {
                out.push(x);
                i += 1;
                j += 1;
            }
            (Some(&x), Some(&y)) if x < y => {
                out.push(x);
                i += 1;
            }
            (Some(&x), None) => {
                out.push(x);
                i += 1;
            }
            (_, Some(&y)) => {
                out.push(y);
                j += 1;
            }
            (None, None) => break,
        }
    }
    out
}

/// The addresses that have every word (or its long form): ids, ascending.
fn matching(snap: &Snap, words: &[String]) -> Res<Vec<u32>> {
    let post = snap.blob("atok.post")?;
    // For each word, where its lists are: the word's own and its long form's.
    let mut wanted: Vec<(u64, Vec<(u64, u64)>)> = Vec::new();
    for w in words {
        let mut lists = Vec::new();
        for form in std::iter::once(w.as_str()).chain(ABBR.iter().find(|(a, _)| a == w).map(|(_, long)| *long)) {
            if let Some(l) = tokens::find(snap, "atok", form.as_bytes())? {
                lists.push(l);
            }
        }
        if lists.is_empty() {
            return Ok(Vec::new());
        }
        wanted.push((lists.iter().map(|l| l.1).sum(), lists));
    }
    wanted.sort_by_key(|w| w.0);
    let mut ids: Option<Vec<u32>> = None;
    for (total, lists) in wanted {
        ids = Some(match ids {
            Some(have) if (have.len() as u64) * 64 < total => {
                let mut from = vec![0u64; lists.len()];
                let mut kept = Vec::new();
                for id in have {
                    for (l, f) in lists.iter().zip(from.iter_mut()) {
                        if tokens::contains(&post, l.0, l.1, id, f)? {
                            kept.push(id);
                            break;
                        }
                    }
                }
                kept
            }
            have => {
                let mut all = Vec::new();
                for l in &lists {
                    all = union(all, tokens::read(&post, l.0, l.1)?);
                }
                match have {
                    Some(have) => {
                        let (mut i, mut j, mut out) = (0, 0, Vec::new());
                        while let (Some(&x), Some(&y)) = (have.get(i), all.get(j)) {
                            match x.cmp(&y) {
                                std::cmp::Ordering::Less => i += 1,
                                std::cmp::Ordering::Greater => j += 1,
                                std::cmp::Ordering::Equal => {
                                    out.push(x);
                                    i += 1;
                                    j += 1;
                                }
                            }
                        }
                        out
                    }
                    None => all,
                }
            }
        });
        if ids.as_ref().is_some_and(Vec::is_empty) {
            break;
        }
    }
    Ok(ids.unwrap_or_default())
}

/// The ids with the fewest words, `limit` of them, fewest first and by id within a count.
fn closest(t: &Tables, ids: &[u32], limit: usize) -> Res<Vec<u32>> {
    let (mut counts, mut start, mut block) = (Vec::with_capacity(ids.len()), u64::MAX, Vec::new());
    for &id in ids {
        let id = u64::from(id);
        if id < start || id >= start + (block.len() / 12) as u64 {
            block = t.ent.block(id, BLOCK).damaged()?;
            start = id;
        }
        let at = (id - start) as usize * 12 + 8;
        counts.push(block.get(at..at + 4).map_or(255, |b| crate::table::le32(b, 0).min(255)) as u8);
    }
    let mut hist = [0u64; 256];
    for &c in &counts {
        hist[usize::from(c)] += 1;
    }
    let (mut seen, mut cutoff) = (0u64, 0usize);
    for (i, n) in hist.iter().enumerate() {
        if *n > 0 {
            cutoff = i;
            seen += n;
            if seen >= limit as u64 {
                break;
            }
        }
    }
    let below: u64 = hist.iter().take(cutoff).sum();
    let mut room_at_cutoff = (limit as u64).saturating_sub(below);
    let mut picked: Vec<(u8, u32)> = Vec::new();
    for (&id, &c) in ids.iter().zip(&counts) {
        if usize::from(c) < cutoff {
            picked.push((c, id));
        } else if usize::from(c) == cutoff && room_at_cutoff > 0 {
            room_at_cutoff -= 1;
            picked.push((c, id));
        }
    }
    picked.sort_unstable();
    Ok(picked.into_iter().map(|(_, id)| id).collect())
}

pub fn address(snap: &Snap, text: &str, id: Option<&str>, limit: usize) -> Res<Answer> {
    let mut query = String::from("{\"verb\":\"address\",");
    quote(&mut query, if id.is_some() { "id" } else { "address" });
    query.push(':');
    quote(&mut query, &echo(id.unwrap_or(text)));
    query.push('}');
    let tokens_of_query = |s: &str| -> Vec<String> {
        normalise(&s.replace('/', " ")).split(' ').filter(|t| !t.is_empty() && !DROP.contains(t) && *t != "U").map(str::to_string).collect()
    };
    let words = if id.is_some() { Vec::new() } else { tokens_of_query(text) };
    if id.is_none() && words.is_empty() {
        return Err(GovError::new("empty-query", "give an address to search for").with_query(query));
    }
    snap.need("gnaf", "gov address")?;
    let t = open(snap)?;
    let mut results = String::new();
    let mut count = 0;
    if let Some(pid) = id {
        let at = t.ent.partition_point(0, |e| Ok(pid_of(&t.rec.record(le64(e, 0))?).is_none_or(|p| p.as_str() < pid))).damaged()?;
        if at < t.ent.count {
            let record = record_of(&t, at)?;
            if pid_of(&record).as_deref() == Some(pid) {
                results = view(snap, &t, &record)?;
                count = 1;
            }
        }
    } else {
        let mut distinct: Vec<String> = Vec::new();
        for w in &words {
            if !distinct.contains(w) {
                distinct.push(w.clone());
            }
        }
        if distinct.len() <= MAX_WORDS {
            let ids = closest(&t, &matching(snap, &distinct)?, limit)?;
            for (i, id) in ids.iter().enumerate() {
                if i > 0 {
                    results.push(',');
                }
                results.push_str(&view(snap, &t, &record_of(&t, u64::from(*id))?)?);
            }
            count = ids.len();
        }
    }
    Ok(Answer { used: &["gnaf"], query, count, results })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_merges_and_drops_repeats() {
        assert_eq!(union(vec![1, 3, 5], vec![2, 3, 6]), [1, 2, 3, 5, 6]);
        assert_eq!(union(vec![], vec![1]), [1]);
        assert_eq!(union(vec![4], vec![]), [4]);
    }
}
