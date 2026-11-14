//! G-NAF (pipe-separated tables, one set per state: `<STATE>_ADDRESS_DETAIL_psv.psv`, `STREET_LOCALITY`, `LOCALITY`,
//! `ADDRESS_DEFAULT_GEOCODE`), loose in a folder or inside the official ZIP. The small tables (streets, localities) are held
//! in memory; the two big ones (every address, every geocode) are joined on the address PID by the external sort: both
//! kinds of row go into one sorter, a geocode row sorting just before its address row, so the merged stream is read once, in
//! PID order, and an address's number is its place in that order (which is the order ties are broken in).
//!
//! Retired addresses and alias rows (`ALIAS_PRINCIPAL` = `A`, about 5 percent of the file) are left out.
//!
//! Files written: `addr.rec` (one record per address), `addr.ent` (12 bytes each: record offset, word count), `atok.*` (the
//! word lists), `gn.street.*` and `gn.loc.*` (the street and locality records with their offset tables).

use crate::build::each_named;
use crate::codec::{Cur, Interner, put_str, put_varint};
use crate::norm::normalise;
use crate::rd::{invalid, read_until};
use crate::sort::Sorter;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};

const MAX_LINE: usize = 1 << 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Table {
    Locality,
    Street,
    Geocode,
    Detail,
}

/// `NSW_ADDRESS_DETAIL_psv.psv` as (`NSW`, detail); `None` for every other file of the release.
fn table_of(name: &str) -> Option<(&str, Table)> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let rest = base.strip_suffix("_psv.psv")?;
    for (suffix, table) in [
        ("ADDRESS_DEFAULT_GEOCODE", Table::Geocode),
        ("ADDRESS_DETAIL", Table::Detail),
        ("STREET_LOCALITY", Table::Street),
        ("LOCALITY", Table::Locality),
    ] {
        if let Some(state) = rest.strip_suffix(suffix).and_then(|s| s.strip_suffix('_'))
            && !state.is_empty()
            && state.bytes().all(|b| b.is_ascii_uppercase())
        {
            return Some((state, table));
        }
    }
    None
}

/// One table: its header names the columns; rows are read one at a time into a reused line.
struct Psv<R> {
    r: R,
    header: Vec<String>,
    line: Vec<u8>,
    cells: Vec<(usize, usize)>,
}

impl<R: BufRead> Psv<R> {
    fn new(mut r: R) -> io::Result<Self> {
        let mut line = Vec::new();
        if !read_until(&mut r, b'\n', &mut line, MAX_LINE)? && line.is_empty() {
            return Err(invalid("a G-NAF table is empty"));
        }
        let text = String::from_utf8_lossy(line.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&line)).trim_end_matches('\r').to_string();
        Ok(Psv { r, header: text.split('|').map(|h| h.trim().to_string()).collect(), line: Vec::new(), cells: Vec::new() })
    }

    fn col(&self, name: &str) -> Option<usize> {
        self.header.iter().position(|h| h == name)
    }

    fn need(&self, name: &str) -> io::Result<usize> {
        self.col(name).ok_or_else(|| invalid(format!("this is not a G-NAF table: there is no {name} column")))
    }

    fn row(&mut self) -> io::Result<bool> {
        loop {
            self.line.clear();
            let found = read_until(&mut self.r, b'\n', &mut self.line, MAX_LINE)?;
            while self.line.last() == Some(&b'\r') {
                self.line.pop();
            }
            if !found && self.line.is_empty() {
                return Ok(false);
            }
            if self.line.is_empty() {
                continue;
            }
            self.cells.clear();
            let mut from = 0;
            for (i, b) in self.line.iter().enumerate() {
                if *b == b'|' {
                    self.cells.push((from, i));
                    from = i + 1;
                }
            }
            self.cells.push((from, self.line.len()));
            return Ok(true);
        }
    }

    fn cell(&self, at: Option<usize>) -> &str {
        at.and_then(|i| self.cells.get(i))
            .and_then(|&(a, b)| self.line.get(a..b))
            .and_then(|s| std::str::from_utf8(s).ok())
            .map_or("", str::trim)
    }
}

/// Calls `f` with every wanted table of every file (loose or inside a ZIP), in name order.
fn each_table(files: &[PathBuf], want: &[Table], f: &mut dyn FnMut(&str, Table, &mut dyn BufRead) -> io::Result<()>) -> io::Result<()> {
    for file in files {
        let wanted = |name: &str| table_of(name).is_some_and(|(_, t)| want.contains(&t));
        each_named(file, &wanted, &mut |name, r| match table_of(name) {
            Some((state, table)) => f(state, table, r),
            None => Ok(()),
        })?;
    }
    Ok(())
}

fn join(parts: &[&str]) -> String {
    parts.concat()
}

/// A table of records by number: `<name>.rec` and `<name>.idx` (little-endian u64 offsets).
struct RecTable {
    rec: BufWriter<File>,
    idx: BufWriter<File>,
    pos: u64,
}

impl RecTable {
    fn create(dir: &Path, name: &str) -> io::Result<RecTable> {
        let open = |ext: &str| File::create(dir.join(format!("{name}.{ext}"))).map(|f| BufWriter::with_capacity(1 << 20, f));
        Ok(RecTable { rec: open("rec")?, idx: open("idx")?, pos: 0 })
    }

    fn add(&mut self, payload: &[u8]) -> io::Result<()> {
        self.idx.write_all(&self.pos.to_le_bytes())?;
        self.rec.write_all(&(payload.len() as u32).to_le_bytes())?;
        self.rec.write_all(payload)?;
        self.pos += 4 + payload.len() as u64;
        Ok(())
    }

    fn finish(mut self) -> io::Result<()> {
        self.rec.flush()?;
        self.idx.flush()
    }
}

/// Builds the address tables into `dir` and returns the number of addresses (retired ones, aliases, and those with no street,
/// locality or geocode, are left out). The state names go into `states`, in the order the merged stream meets them.
pub fn build(files: &[PathBuf], dir: &Path, sort_dir: &Path, run_bytes: usize, states: &mut Interner) -> io::Result<u64> {
    // Streets and localities: held whole, numbered in the order they are read.
    let (mut streets, mut street_ix): (Vec<(String, String)>, HashMap<String, u32>) = (Vec::new(), HashMap::new());
    let (mut locs, mut loc_ix): (Vec<(String, String)>, HashMap<String, u32>) = (Vec::new(), HashMap::new());
    each_table(files, &[Table::Locality, Table::Street], &mut |_, table, r| {
        let mut t = Psv::new(r)?;
        if table == Table::Locality {
            // A retired locality or street is kept: an address that is not retired may still stand on it.
            let (pid, name, post) = (t.need("LOCALITY_PID")?, t.need("LOCALITY_NAME")?, t.col("PRIMARY_POSTCODE"));
            while t.row()? {
                loc_ix.insert(t.cell(Some(pid)).to_string(), locs.len() as u32);
                locs.push((t.cell(Some(name)).to_string(), t.cell(post).to_string()));
            }
        } else {
            let (pid, name, kind, suffix) =
                (t.need("STREET_LOCALITY_PID")?, t.need("STREET_NAME")?, t.col("STREET_TYPE_CODE"), t.col("STREET_SUFFIX_CODE"));
            while t.row()? {
                let kind = [t.cell(kind), t.cell(suffix)].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
                street_ix.insert(t.cell(Some(pid)).to_string(), streets.len() as u32);
                streets.push((t.cell(Some(name)).to_string(), kind));
            }
        }
        Ok(())
    })?;

    // Addresses and geocodes into one sorter, keyed by the address PID.
    let mut joined = Sorter::new(sort_dir, "gn", run_bytes);
    let mut rec = Vec::new();
    each_table(files, &[Table::Geocode, Table::Detail], &mut |state, table, r| {
        let mut t = Psv::new(r)?;
        if table == Table::Geocode {
            let (pid, lat, lon, retired) =
                (t.need("ADDRESS_DETAIL_PID")?, t.need("LATITUDE")?, t.need("LONGITUDE")?, t.col("DATE_RETIRED"));
            while t.row()? {
                let (Ok(la), Ok(lo)) = (t.cell(Some(lat)).parse::<f64>(), t.cell(Some(lon)).parse::<f64>()) else { continue };
                if !t.cell(retired).is_empty() || !la.is_finite() || !lo.is_finite() {
                    continue;
                }
                rec.clear();
                rec.extend_from_slice(t.cell(Some(pid)).as_bytes());
                rec.extend_from_slice(&[0, 0]);
                rec.extend_from_slice(&la.to_le_bytes());
                rec.extend_from_slice(&lo.to_le_bytes());
                joined.push(&rec)?;
            }
            return Ok(());
        }
        let c = |name: &str| t.col(name);
        let (pid, street, loc, post) =
            (t.need("ADDRESS_DETAIL_PID")?, t.need("STREET_LOCALITY_PID")?, t.need("LOCALITY_PID")?, c("POSTCODE"));
        let (retired, flat_type, flat, flat_pre, flat_suf) =
            (c("DATE_RETIRED"), c("FLAT_TYPE_CODE"), c("FLAT_NUMBER"), c("FLAT_NUMBER_PREFIX"), c("FLAT_NUMBER_SUFFIX"));
        let (n1, n1_pre, n1_suf, n2, n2_pre, n2_suf) = (
            c("NUMBER_FIRST"),
            c("NUMBER_FIRST_PREFIX"),
            c("NUMBER_FIRST_SUFFIX"),
            c("NUMBER_LAST"),
            c("NUMBER_LAST_PREFIX"),
            c("NUMBER_LAST_SUFFIX"),
        );
        let (lot, lot_pre, lot_suf, alias) = (c("LOT_NUMBER"), c("LOT_NUMBER_PREFIX"), c("LOT_NUMBER_SUFFIX"), c("ALIAS_PRINCIPAL"));
        while t.row()? {
            // An alias row (`A`) is another name for a principal address that is in the index: it would repeat the address.
            if !t.cell(retired).is_empty() || t.cell(alias) == "A" {
                continue;
            }
            let (Some(&s), Some(&l)) = (street_ix.get(t.cell(Some(street))), loc_ix.get(t.cell(Some(loc)))) else { continue };
            let mut number = join(&[t.cell(n1_pre), t.cell(n1), t.cell(n1_suf)]);
            if !t.cell(n2).is_empty() {
                number = format!("{number}-{}", join(&[t.cell(n2_pre), t.cell(n2), t.cell(n2_suf)]));
            }
            if number.is_empty() && !t.cell(lot).is_empty() {
                number = format!("LOT {}", join(&[t.cell(lot_pre), t.cell(lot), t.cell(lot_suf)]));
            }
            let flat_no = join(&[t.cell(flat_pre), t.cell(flat), t.cell(flat_suf)]);
            let postcode =
                Some(t.cell(post)).filter(|p| !p.is_empty()).or_else(|| locs.get(l as usize).map(|x| x.1.as_str())).unwrap_or("");
            rec.clear();
            rec.extend_from_slice(t.cell(Some(pid)).as_bytes());
            rec.extend_from_slice(&[0, 1]);
            put_str(&mut rec, state);
            put_str(&mut rec, if flat_no.is_empty() { "" } else { t.cell(flat_type) });
            put_str(&mut rec, &flat_no);
            put_str(&mut rec, &number);
            put_str(&mut rec, postcode);
            put_varint(&mut rec, u64::from(s));
            put_varint(&mut rec, u64::from(l));
            joined.push(&rec)?;
        }
        Ok(())
    })?;

    // The merged stream: a geocode, then the address it belongs to.
    let (mut addr, mut ent) = (
        BufWriter::with_capacity(1 << 20, File::create(dir.join("addr.rec"))?),
        BufWriter::with_capacity(1 << 20, File::create(dir.join("addr.ent"))?),
    );
    let mut tokens = Sorter::new(sort_dir, "atok", run_bytes);
    let (mut count, mut pos) = (0u64, 0u64);
    let mut geo: Option<(Vec<u8>, [u8; 16])> = None;
    let (mut payload, mut key) = (Vec::new(), Vec::new());
    joined.for_each(|r| {
        let nul = r.iter().position(|&b| b == 0).ok_or_else(|| invalid("an address record without its end"))?;
        let (pid, tag, body) = (&r[..nul], r.get(nul + 1).copied(), r.get(nul + 2..).unwrap_or_default());
        if tag == Some(0) {
            geo = body.try_into().ok().map(|b: [u8; 16]| (pid.to_vec(), b));
            return Ok(());
        }
        let Some((_, coords)) = geo.as_ref().filter(|(p, _)| p.as_slice() == pid) else { return Ok(()) };
        let bad = || invalid("an address record that does not decode");
        let mut c = Cur::new(body);
        let (state, flat_type, flat_no, number, postcode) = (
            c.str().ok_or_else(bad)?,
            c.str().ok_or_else(bad)?,
            c.str().ok_or_else(bad)?,
            c.str().ok_or_else(bad)?,
            c.str().ok_or_else(bad)?,
        );
        let (s, l) = (c.varint().ok_or_else(bad)? as usize, c.varint().ok_or_else(bad)? as usize);
        let (Some((street, kind)), Some((locality, _))) = (streets.get(s), locs.get(l)) else { return Err(bad()) };
        payload.clear();
        put_str(&mut payload, &String::from_utf8_lossy(pid));
        put_str(&mut payload, &flat_type);
        put_str(&mut payload, &flat_no);
        put_str(&mut payload, &number);
        put_varint(&mut payload, s as u64);
        put_varint(&mut payload, l as u64);
        put_varint(&mut payload, u64::from(states.intern(&state)));
        put_str(&mut payload, &postcode);
        payload.extend_from_slice(coords);
        addr.write_all(&(payload.len() as u32).to_le_bytes())?;
        addr.write_all(&payload)?;
        // The words an address is found by: the reference's `flat number, number, street, type, locality, state, postcode`.
        let words = normalise(&format!("{flat_no} {number} {street} {kind} {locality} {state} {postcode}"));
        let all: Vec<&str> = words.split(' ').filter(|w| !w.is_empty()).collect();
        let id = u32::try_from(count).map_err(|_| invalid("more addresses than the index holds"))?;
        let mut seen: Vec<&str> = Vec::new();
        for w in &all {
            if !seen.contains(w) {
                seen.push(w);
                key.clear();
                key.extend_from_slice(w.as_bytes());
                key.push(0);
                key.extend_from_slice(&id.to_be_bytes());
                tokens.push(&key)?;
            }
        }
        ent.write_all(&pos.to_le_bytes())?;
        ent.write_all(&(all.len() as u32).to_le_bytes())?;
        pos += 4 + payload.len() as u64;
        count += 1;
        Ok(())
    })?;
    addr.flush()?;
    ent.flush()?;
    crate::tokens::write(tokens, dir, "atok")?;
    for (name, items) in [("gn.street", &streets), ("gn.loc", &locs)] {
        let mut t = RecTable::create(dir, name)?;
        let mut p = Vec::new();
        for (a, b) in items {
            p.clear();
            put_str(&mut p, a);
            put_str(&mut p, b);
            t.add(&p)?;
        }
        t.finish()?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_tables_of_the_address_are_read() {
        assert_eq!(table_of("NSW_ADDRESS_DETAIL_psv.psv"), Some(("NSW", Table::Detail)));
        assert_eq!(table_of("G-NAF/G-NAF AUGUST 2026/Standard/VIC_STREET_LOCALITY_psv.psv"), Some(("VIC", Table::Street)));
        assert_eq!(table_of("OT_LOCALITY_psv.psv"), Some(("OT", Table::Locality)));
        assert_eq!(table_of("NSW_ADDRESS_DEFAULT_GEOCODE_psv.psv"), Some(("NSW", Table::Geocode)));
        assert_eq!(table_of("NSW_LOCALITY_ALIAS_psv.psv"), None);
        assert_eq!(table_of("Authority_Code_STREET_TYPE_AUT_psv.psv"), None);
        assert_eq!(table_of("NSW_STATE_psv.psv"), None);
    }

    #[test]
    fn a_table_without_the_columns_is_refused() {
        assert!(Psv::new(io::Cursor::new(b"A|B\n1|2\n".to_vec())).and_then(|t| t.need("ADDRESS_DETAIL_PID").map(|_| ())).is_err());
        assert!(Psv::new(io::Cursor::new(Vec::new())).is_err());
    }
}
