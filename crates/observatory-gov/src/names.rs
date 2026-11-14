//! Searching names. Ranking is total and fixed: class (the whole normalised name equals the query, or starts with it, or has
//! every query word as a word, in any order), then active before cancelled, then the matched name's length, then the
//! matched name, then the ABN. No fuzzy matching. Classes 0 and 1 are one contiguous range of the sorted name table, found
//! by two binary searches; class 2 is an intersection of the words' lists and is only read when the first two do not fill
//! the limit.

use crate::codec::{BusinessName, Company};
use crate::error::{GovError, OrDamaged, Res};
use crate::names_build::{ENTRY, FLAG_ACTIVE, FLAG_IN_ABR, MAX_NAME, SRC_ABR, SRC_ASIC, SRC_BN};
use crate::norm::normalise;
use crate::snap::Snap;
use crate::table::{Blob, Fixed, le64};
use std::collections::HashMap;
use std::io;

pub struct Query<'a> {
    pub text: &'a str,
    pub state: Option<&'a str>,
    pub active: bool,
    pub limit: usize,
}

/// A name matched: the entity it names, which stored name matched, and how.
#[derive(Debug, Clone)]
pub struct Hit {
    pub cls: u8,
    pub len: u8,
    pub active: bool,
    pub abn: u64,
    pub in_abr: bool,
    pub src: u8,
    pub rec: u64,
    pub nidx: u8,
}

impl Hit {
    fn from_entry(e: &[u8], cls: u8) -> Hit {
        let flags = e.get(27).copied().unwrap_or(0);
        Hit {
            cls,
            len: e.get(24).copied().unwrap_or(0),
            active: flags & FLAG_ACTIVE != 0,
            abn: le64(e, 8),
            in_abr: flags & FLAG_IN_ABR != 0,
            src: e.get(26).copied().unwrap_or(0),
            rec: le64(e, 16),
            nidx: e.get(25).copied().unwrap_or(0),
        }
    }

    /// One entity is one result: by ABN, or by its registration when it has none.
    fn entity(&self) -> (bool, u64) {
        if self.abn != 0 { (true, self.abn) } else { (false, self.rec) }
    }

    fn order(&self) -> (u8, u8, u64, u8) {
        (self.cls, self.len, (u64::from(self.src) << 56) | (self.rec & ((1 << 56) - 1)), self.nidx)
    }

    fn abn_key(&self) -> u64 {
        if self.abn == 0 { u64::MAX } else { self.abn }
    }
}

struct Tables {
    ent: Fixed,
    strs: Blob,
}

impl Tables {
    fn norm_at(&self, e: &[u8], buf: &mut Vec<u8>) -> io::Result<()> {
        buf.resize(usize::from(e.get(24).copied().unwrap_or(0)), 0);
        self.strs.read_exact_at(le64(e, 0), buf)
    }

    fn first_not_below(&self, from: u64, key: &[u8]) -> io::Result<u64> {
        let mut buf = Vec::new();
        self.ent.partition_point(from, |e| {
            self.norm_at(e, &mut buf)?;
            Ok(buf.as_slice() < key)
        })
    }
}

/// The key just above every string that starts with `prefix`.
fn successor(prefix: &[u8]) -> Vec<u8> {
    let mut s = prefix.to_vec();
    if let Some(last) = s.last_mut() {
        *last += 1;
    }
    s
}

fn passes(e: &[u8], q: &Query<'_>, want_state: Option<[u8; 4]>) -> bool {
    let flags = e.get(27).copied().unwrap_or(0);
    if q.active && flags & FLAG_ACTIVE == 0 {
        return false;
    }
    want_state.is_none_or(|s| e.get(28..32) == Some(&s[..]))
}

fn keep_better(map: &mut HashMap<(bool, u64), Hit>, hit: Hit) {
    match map.get_mut(&hit.entity()) {
        Some(old) if old.order() <= hit.order() => {}
        Some(old) => *old = hit,
        None => {
            map.insert(hit.entity(), hit);
        }
    }
}

/// The ids of the entries whose name has this word.
fn postings(snap: &Snap, word: &[u8]) -> Res<Vec<u32>> {
    match crate::tokens::find(snap, "tok", word)? {
        Some((start, count)) => crate::tokens::read(&snap.blob("tok.post")?, start, count),
        None => Ok(Vec::new()),
    }
}

fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while let (Some(&x), Some(&y)) = (a.get(i), b.get(j)) {
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

/// The matched name as stored (its text and its kind), from the record the entry points at.
pub fn matched(snap: &Snap, hit: &Hit) -> Res<(String, String)> {
    let bad = || GovError::damaged("a name entry points at a record that is not there");
    match hit.src {
        SRC_ABR => {
            let rec = snap.blob("abn.rec")?.record(hit.rec).damaged()?;
            let r = crate::codec::AbrRecord::decode(hit.abn, &rec, &snap.meta.dicts).ok_or_else(bad)?;
            Ok(match usize::from(hit.nidx).checked_sub(1) {
                None => (r.name, "MN".to_string()),
                Some(i) => r.names.get(i).map(|(k, t)| (t.clone(), k.clone())).ok_or_else(bad)?,
            })
        }
        SRC_ASIC => {
            let c = Company::decode(&snap.blob("co.rec")?.record(hit.rec).damaged()?).ok_or_else(bad)?;
            let text = match usize::from(hit.nidx).checked_sub(1) {
                None => c.name.ok_or_else(bad)?,
                Some(i) => c.former.get(i).cloned().ok_or_else(bad)?,
            };
            Ok((text, "ASIC".to_string()))
        }
        SRC_BN => {
            let b = BusinessName::decode(&snap.blob("bn.rec")?.record(hit.rec).damaged()?).ok_or_else(bad)?;
            Ok((b.name, "BN".to_string()))
        }
        _ => Err(bad()),
    }
}

/// Entries read at a time from the name table.
const BLOCK: u64 = 256;

fn bucket(e: &[u8], cls: u8) -> usize {
    let inactive = usize::from(e.get(27).copied().unwrap_or(0) & FLAG_ACTIVE == 0);
    (usize::from(cls) * 2 + inactive) * 256 + usize::from(e.get(24).copied().unwrap_or(0))
}

/// The first bucket (in rank order) at which `want` entries have been seen, or the last bucket that holds any.
fn threshold(hist: &[u64], want: u64) -> usize {
    let (mut seen, mut last) = (0, 0);
    for (i, n) in hist.iter().enumerate() {
        if *n == 0 {
            continue;
        }
        seen += n;
        last = i;
        if seen >= want {
            break;
        }
    }
    last
}

const BUCKETS: usize = 3 * 2 * 256;

/// One pass over the entries `scan` yields: how many fall in each bucket, and the entries in the buckets that can still
/// matter. The kept ones are those up to the bucket where the count reaches `want`; that bound only ever falls as more
/// entries arrive, so what was dropped before is never needed after.
fn one_pass(scan: &dyn Fn(&mut dyn FnMut(&[u8], u8) -> Res<()>) -> Res<()>, want: u64) -> Res<(Vec<u64>, Vec<(usize, Hit)>)> {
    let mut hist = vec![0u64; BUCKETS];
    let mut kept: Vec<(usize, Hit)> = Vec::new();
    let (mut top, mut cum) = (BUCKETS - 1, 0u64);
    scan(&mut |e, cls| {
        let b = bucket(e, cls);
        if let Some(n) = hist.get_mut(b) {
            *n += 1;
        }
        if b > top {
            return Ok(());
        }
        cum += 1;
        kept.push((b, Hit::from_entry(e, cls)));
        if cum >= want + hist.get(top).copied().unwrap_or(0) {
            top = threshold(&hist, want);
            cum = hist.iter().take(top + 1).sum();
            kept.retain(|(b, _)| *b <= top);
        }
        Ok(())
    })?;
    Ok((hist, kept))
}

/// The best entity of every name entry `scan` yields, for the smallest set of rank buckets (class, status, length) that
/// still holds `limit` distinct entities. A word that is in millions of names costs one sequential read and a handful of
/// kept entries, never one result each. One entity under several names can leave the kept set short of `limit`: the bound is
/// then widened and the scan repeated, which ends when the set holds `limit` entities or every bucket.
fn best(scan: &dyn Fn(&mut dyn FnMut(&[u8], u8) -> Res<()>) -> Res<()>, limit: usize) -> Res<HashMap<(bool, u64), Hit>> {
    let mut want = limit as u64;
    loop {
        let (hist, kept) = one_pass(scan, want)?;
        let top = threshold(&hist, want);
        let mut found: HashMap<(bool, u64), Hit> = HashMap::new();
        for (b, hit) in kept {
            if b <= top {
                keep_better(&mut found, hit);
            }
        }
        let (within, total): (u64, u64) = (hist.iter().take(top + 1).sum(), hist.iter().sum());
        if found.len() >= limit || within >= total {
            return Ok(found);
        }
        want = within.max(1) * 2;
    }
}

pub fn search(snap: &Snap, q: &Query<'_>) -> Res<Vec<Hit>> {
    let norm = normalise(q.text);
    if norm.is_empty() || norm.len() > MAX_NAME || q.limit == 0 {
        return Ok(Vec::new());
    }
    let key = norm.as_bytes();
    let t = Tables { ent: snap.fixed("nm.ent", ENTRY as u64)?, strs: snap.blob("nm.str")? };
    let want_state = q.state.map(crate::names_build::state4);
    if let Some(s) = q.state
        && s.len() > 4
    {
        return Ok(Vec::new());
    }
    // Classes 0 and 1: every name that starts with the query, one range of the table.
    let lo = t.first_not_below(0, key).damaged()?;
    let hi = t.first_not_below(lo, &successor(key)).damaged()?;
    let range = |f: &mut dyn FnMut(&[u8], u8) -> Res<()>| -> Res<()> {
        let mut at = lo;
        while at < hi {
            let block = t.ent.block(at, BLOCK.min(hi - at)).damaged()?;
            for e in block.chunks_exact(ENTRY) {
                if passes(e, q, want_state) {
                    f(e, u8::from(usize::from(e.get(24).copied().unwrap_or(0)) != key.len()))?;
                }
            }
            at += BLOCK;
        }
        Ok(())
    };
    let mut found = best(&range, q.limit)?;

    // Class 2: names with every query word as a word.
    if found.len() < q.limit {
        let mut words: Vec<&[u8]> = key.split(|&b| b == b' ').filter(|w| !w.is_empty()).collect();
        words.sort_unstable();
        words.dedup();
        let mut lists = Vec::new();
        for w in &words {
            lists.push(postings(snap, w)?);
        }
        lists.sort_by_key(Vec::len);
        let mut ids = lists.first().cloned().unwrap_or_default();
        for l in lists.iter().skip(1) {
            if ids.is_empty() {
                break;
            }
            ids = intersect(&ids, l);
        }
        let words = |f: &mut dyn FnMut(&[u8], u8) -> Res<()>| -> Res<()> {
            let (mut start, mut block) = (u64::MAX, Vec::new());
            for &id in &ids {
                let id = u64::from(id);
                if (lo..hi).contains(&id) {
                    continue;
                }
                if id < start || id >= start + (block.len() / ENTRY) as u64 {
                    block = t.ent.block(id, BLOCK).damaged()?;
                    start = id;
                }
                let at = ((id - start) as usize) * ENTRY;
                if let Some(e) = block.get(at..at + ENTRY)
                    && passes(e, q, want_state)
                {
                    f(e, 2)?;
                }
            }
            Ok(())
        };
        // Entities already found by a better class are skipped, so ask for enough to still fill the limit.
        for (_, hit) in best(&words, q.limit + found.len())? {
            keep_better(&mut found, hit);
        }
    }

    let mut hits: Vec<Hit> = found.into_values().collect();
    hits.sort_by_key(|h| (h.cls, !h.active, h.len, h.abn_key(), h.order()));
    // Names of the same class, status and length are ordered by the name itself: read only where that decides the window.
    let mut from = 0;
    while from < hits.len().min(q.limit) {
        let same = |h: &Hit| (h.cls, h.active, h.len) == (hits[from].cls, hits[from].active, hits[from].len);
        let to = from + hits[from..].iter().take_while(|h| same(h)).count();
        if to - from > 1 {
            let mut group: Vec<(String, Hit)> = Vec::with_capacity(to - from);
            for h in &hits[from..to] {
                group.push((matched(snap, h)?.0, h.clone()));
            }
            group.sort_by(|a, b| (&a.0, a.1.abn_key(), a.1.order()).cmp(&(&b.0, b.1.abn_key(), b.1.order())));
            for (slot, (_, h)) in hits[from..to].iter_mut().zip(group) {
                *slot = h;
            }
        }
        from = to;
    }
    hits.truncate(q.limit);
    Ok(hits)
}
