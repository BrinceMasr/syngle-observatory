//! ABS statistics: the dataflow catalogue (SDMX-JSON, `dataflows.json`) and the observations of each dataflow fetched
//! (SDMX-CSV, `data/<DATAFLOW>.csv`). The catalogue is written as `abs.flows`; the observations of every dataflow are one
//! block of `abs.dat` (located by `abs.cat`), laid out so a query reads the block once and works on numbers:
//!
//! ```text
//! u32 ncols, u32 nrows, u32 period column, u64 start of the rows
//! per column: u32 length and the name; u64 where its dictionary is and u32 how many values it holds
//! a dictionary: (count + 1) u32 offsets, then the values back to back
//! the rows: nrows * ncols u32 (a value's number in its column's dictionary), in the order the answer is given in:
//!           by TIME_PERIOD, rows with the same period in the order the file has them
//! ```
//!
//! The period dictionary is sorted, so a period's number orders like its text and a range of periods is a binary search.

use crate::error::{GovError, Res};
use crate::rd::invalid;
use crate::table::{le32, le64};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Columns that are not dimensions: the answer gives them as `period`, `value` and `unit`, and leaves the rest out.
pub const NOT_DIMENSIONS: [&str; 6] = ["DATAFLOW", "TIME_PERIOD", "OBS_VALUE", "UNIT_MEASURE", "OBS_STATUS", "DECIMALS"];
/// The ABS gateway answers at most 10 MB a query; a file far above that is not an observation file.
const MAX_CSV: u64 = 256 << 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Flow {
    pub id: String,
    pub version: String,
    pub name: String,
}

#[derive(Deserialize)]
struct Catalogue {
    data: CatalogueData,
}

#[derive(Deserialize)]
struct CatalogueData {
    dataflows: Vec<RawFlow>,
}

#[derive(Deserialize)]
struct RawFlow {
    id: Option<String>,
    version: Option<String>,
    name: Option<String>,
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn u32_of(n: usize) -> io::Result<u32> {
    u32::try_from(n).map_err(|_| invalid("a table of observations is larger than the format allows"))
}

/// Reads one SDMX-CSV file into the block described above.
fn block(r: impl Read) -> io::Result<Vec<u8>> {
    let mut csv = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(r);
    let mut rec = csv::ByteRecord::new();
    if !csv.read_byte_record(&mut rec).map_err(|e| invalid(e.to_string()))? {
        return Err(invalid("an observation file is empty"));
    }
    let names: Vec<String> = rec
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let h = String::from_utf8_lossy(h);
            (if i == 0 { h.trim_start_matches('\u{feff}') } else { &h }).trim().to_string()
        })
        .collect();
    let period = names.iter().position(|n| n == "TIME_PERIOD").ok_or_else(|| invalid("an observation file has no TIME_PERIOD column"))?;
    if !names.iter().any(|n| n == "OBS_VALUE") {
        return Err(invalid("an observation file has no OBS_VALUE column"));
    }
    let ncols = names.len();
    let mut dicts: Vec<(HashMap<Vec<u8>, u32>, Vec<Vec<u8>>)> = (0..ncols).map(|_| (HashMap::new(), Vec::new())).collect();
    let mut cells: Vec<u32> = Vec::new();
    while csv.read_byte_record(&mut rec).map_err(|e| invalid(e.to_string()))? {
        for (c, (map, list)) in dicts.iter_mut().enumerate() {
            let v = rec.get(c).unwrap_or_default();
            let n = match map.get(v) {
                Some(&n) => n,
                None => {
                    let n = u32_of(list.len())?;
                    map.insert(v.to_vec(), n);
                    list.push(v.to_vec());
                    n
                }
            };
            cells.push(n);
        }
    }
    let nrows = cells.len() / ncols.max(1);
    // The period dictionary in text order, the rows stable by period.
    let mut order: Vec<u32> = (0..u32_of(dicts[period].1.len())?).collect();
    order.sort_by(|a, b| dicts[period].1[*a as usize].cmp(&dicts[period].1[*b as usize]));
    let mut rank = vec![0u32; order.len()];
    for (new, old) in order.iter().enumerate() {
        rank[*old as usize] = new as u32;
    }
    let sorted_period: Vec<Vec<u8>> = order.iter().map(|o| dicts[period].1[*o as usize].clone()).collect();
    dicts[period].1 = sorted_period;
    let mut rows: Vec<u32> = (0..u32_of(nrows)?).collect();
    rows.sort_by_key(|r| rank[cells[*r as usize * ncols + period] as usize]);

    let mut out = Vec::new();
    for n in [u32_of(ncols)?, u32_of(nrows)?, u32_of(period)?] {
        out.extend_from_slice(&n.to_le_bytes());
    }
    let rows_at_slot = out.len();
    out.extend_from_slice(&0u64.to_le_bytes());
    for n in &names {
        out.extend_from_slice(&u32_of(n.len())?.to_le_bytes());
        out.extend_from_slice(n.as_bytes());
    }
    let dict_slots = out.len();
    out.resize(out.len() + ncols * 12, 0);
    for (c, (_, list)) in dicts.iter().enumerate() {
        let at = out.len() as u64;
        let slot = dict_slots + c * 12;
        out[slot..slot + 8].copy_from_slice(&at.to_le_bytes());
        out[slot + 8..slot + 12].copy_from_slice(&u32_of(list.len())?.to_le_bytes());
        let mut off = 0u32;
        out.extend_from_slice(&off.to_le_bytes());
        for v in list {
            off = off.checked_add(u32_of(v.len())?).ok_or_else(|| invalid("a dictionary is larger than the format allows"))?;
            out.extend_from_slice(&off.to_le_bytes());
        }
        for v in list {
            out.extend_from_slice(v);
        }
    }
    let rows_at = out.len() as u64;
    out[rows_at_slot..rows_at_slot + 8].copy_from_slice(&rows_at.to_le_bytes());
    for r in rows {
        for c in 0..ncols {
            let cell = cells[r as usize * ncols + c];
            let cell = if c == period { rank[cell as usize] } else { cell };
            out.extend_from_slice(&cell.to_le_bytes());
        }
    }
    Ok(out)
}

/// Writes `abs.flows`, `abs.dat` and `abs.cat` into `dir` from the files of the source; the number of dataflows.
pub fn build(files: &[PathBuf], dir: &Path) -> io::Result<u64> {
    let mut flows: Vec<Flow> = Vec::new();
    let mut have_catalogue = false;
    let mut dat = BufWriter::with_capacity(1 << 20, File::create(dir.join("abs.dat"))?);
    let (mut cat, mut pos): (BTreeMap<String, (u64, u64)>, u64) = (BTreeMap::new(), 0);
    for file in files {
        let name = file.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if name == "dataflows.json" {
            let text = std::fs::read(file)?;
            let parsed: Catalogue =
                serde_json::from_slice(&text).map_err(|e| invalid(format!("dataflows.json is not an SDMX-JSON dataflow list: {e}")))?;
            flows.extend(
                parsed
                    .data
                    .dataflows
                    .into_iter()
                    .filter_map(|f| Some(Flow { id: f.id?, version: f.version.unwrap_or_default(), name: f.name.unwrap_or_default() })),
            );
            have_catalogue = true;
        } else if name.ends_with(".csv") {
            let csv = File::open(file)?;
            if csv.metadata()?.len() > MAX_CSV {
                return Err(invalid(format!("{} is larger than an observation file can be", file.display())));
            }
            let b = block(csv).map_err(|e| invalid(format!("{}: {e}", file.display())))?;
            dat.write_all(&b)?;
            cat.insert(stem(file), (pos, b.len() as u64));
            pos += b.len() as u64;
        }
    }
    dat.flush()?;
    if !have_catalogue {
        return Err(invalid("the ABS source needs its dataflows.json"));
    }
    std::fs::write(dir.join("abs.flows"), serde_json::to_vec(&flows).map_err(|e| invalid(e.to_string()))?)?;
    std::fs::write(dir.join("abs.cat"), serde_json::to_vec(&cat).map_err(|e| invalid(e.to_string()))?)?;
    Ok(flows.len() as u64)
}

/// One dataflow's observations, read from its block.
pub struct Table<'a> {
    b: &'a [u8],
    pub names: Vec<&'a str>,
    dicts: Vec<(usize, usize)>,
    pub nrows: usize,
    pub period: usize,
    rows_at: usize,
}

impl<'a> Table<'a> {
    pub fn open(b: &'a [u8]) -> Res<Table<'a>> {
        let bad = || GovError::damaged("an observation block does not decode");
        let (ncols, nrows, period) = (le32(b, 0) as usize, le32(b, 4) as usize, le32(b, 8) as usize);
        let rows_at = usize::try_from(le64(b, 12)).map_err(|_| bad())?;
        let mut at = 20usize;
        let mut names = Vec::new();
        for _ in 0..ncols {
            let n = le32(b, at) as usize;
            names.push(b.get(at + 4..at + 4 + n).and_then(|s| std::str::from_utf8(s).ok()).ok_or_else(bad)?);
            at += 4 + n;
        }
        let dicts: Vec<(usize, usize)> =
            (0..ncols).map(|c| (usize::try_from(le64(b, at + c * 12)).unwrap_or(usize::MAX), le32(b, at + c * 12 + 8) as usize)).collect();
        if period >= ncols
            || rows_at.checked_add(nrows.checked_mul(ncols).and_then(|n| n.checked_mul(4)).ok_or_else(bad)?).is_none_or(|end| end > b.len())
        {
            return Err(bad());
        }
        Ok(Table { b, names, dicts, nrows, period, rows_at })
    }

    pub fn ncols(&self) -> usize {
        self.names.len()
    }

    /// The number of the value in the cell of a row's column.
    pub fn cell(&self, row: usize, col: usize) -> u32 {
        le32(self.b, self.rows_at + (row * self.ncols() + col) * 4)
    }

    /// The text of a value of a column.
    pub fn text(&self, col: usize, id: u32) -> &'a str {
        let Some(&(at, count)) = self.dicts.get(col) else { return "" };
        let id = id as usize;
        if id >= count {
            return "";
        }
        let strings = at + (count + 1) * 4;
        let (from, to) = (le32(self.b, at + id * 4) as usize, le32(self.b, at + (id + 1) * 4) as usize);
        self.b.get(strings + from..strings + to).and_then(|s| std::str::from_utf8(s).ok()).unwrap_or("")
    }

    /// The number of a value in a column, if the column holds it.
    pub fn find(&self, col: usize, value: &str) -> Option<u32> {
        let count = self.dicts.get(col)?.1;
        (0..count as u32).find(|&i| self.text(col, i) == value)
    }

    /// The first row whose period is not below `from` (when `strictly` is false) or is above it (when it is true).
    pub fn first_row(&self, from: &str, strictly: bool) -> usize {
        let (mut lo, mut hi) = (0, self.nrows);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let p = self.text(self.period, self.cell(mid, self.period));
            if if strictly { p <= from } else { p < from } { lo = mid + 1 } else { hi = mid }
        }
        lo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "DATAFLOW,MEASURE,REGION,FREQ,TIME_PERIOD,OBS_VALUE,UNIT_MEASURE,OBS_STATUS,DECIMALS\r\nABS:X(1.0.0),1,1,A,2024,8500000,NUM,,0\r\nABS:X(1.0.0),1,2,A,2023,6800000,NUM,,0\r\nABS:X(1.0.0),1,1,A,2023,8400000,NUM,,0\r\nABS:X(1.0.0),1,\"2,x\",A,2024,\"1.5\",NUM,,0\r\n";

    #[test]
    fn rows_come_back_by_period_and_in_file_order_within_one() {
        let b = block(io::Cursor::new(CSV.as_bytes().to_vec())).unwrap();
        let t = Table::open(&b).unwrap();
        assert_eq!((t.nrows, t.ncols(), t.names[t.period]), (4, 9, "TIME_PERIOD"));
        let region = t.names.iter().position(|n| *n == "REGION").unwrap();
        let order: Vec<(&str, &str)> =
            (0..t.nrows).map(|r| (t.text(t.period, t.cell(r, t.period)), t.text(region, t.cell(r, region)))).collect();
        assert_eq!(order, [("2023", "2"), ("2023", "1"), ("2024", "1"), ("2024", "2,x")]);
        assert_eq!(
            (t.first_row("2024", false), t.first_row("2023", true), t.first_row("1999", false), t.first_row("9999", true)),
            (2, 2, 0, 4)
        );
        assert_eq!(t.find(region, "2,x").map(|i| t.text(region, i)), Some("2,x"));
        assert_eq!(t.find(region, "nope"), None);
    }

    #[test]
    fn a_file_without_its_columns_is_refused_and_garbage_does_not_open() {
        assert!(block(io::Cursor::new(b"A,B\n1,2\n".to_vec())).is_err());
        assert!(block(io::Cursor::new(Vec::new())).is_err());
        assert!(Table::open(&[1, 2, 3]).is_err());
        assert!(Table::open(&[0xff; 64]).is_err());
    }
}
