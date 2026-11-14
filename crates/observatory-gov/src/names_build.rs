//! The name tables, built from every name of every entity (the ABN extract's legal, business and trading names, the ASIC
//! companies' current and former names, the business names):
//!
//! - `nm.ent`: 32-byte entries sorted by normalised name, so an exact or prefix match is one contiguous range found by
//!   binary search; each entry holds what ranking and filtering need (status, state, length) and where the entity and
//!   the matched name are stored;
//! - `nm.str`: the normalised names, in that order;
//! - `tok.key`, `tok.str`, `tok.post`: for each word, the sorted list of entries whose name has it, so "all words, any
//!   order" is an intersection of lists.
//!
//! An entry in the sort is `name 0x00 abn(8, big-endian) src(1) rec(8) nidx(1) flags(1) state(4)`: the byte order of the
//! whole is the order of the table, ending on the ABN, so it is total and the same on every machine.

use crate::rd::invalid;
use crate::sort::Sorter;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

pub const ENTRY: usize = 32;
pub const SRC_ABR: u8 = 0;
pub const SRC_ASIC: u8 = 1;
pub const SRC_BN: u8 = 2;
pub const FLAG_ACTIVE: u8 = 1;
pub const FLAG_IN_ABR: u8 = 2;
/// Names are kept to this many bytes of their normalised form.
pub const MAX_NAME: usize = 255;

pub struct NameSink {
    sorter: Sorter,
    scratch: Vec<u8>,
}

pub fn state4(state: &str) -> [u8; 4] {
    let mut out = [0u8; 4];
    for (o, b) in out.iter_mut().zip(state.bytes()) {
        *o = b;
    }
    out
}

impl NameSink {
    pub fn new(dir: &Path, run_bytes: usize) -> NameSink {
        NameSink { sorter: Sorter::new(dir, "nm", run_bytes), scratch: Vec::new() }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add(&mut self, text: &str, abn: u64, src: u8, rec: u64, nidx: u8, flags: u8, state: [u8; 4]) -> io::Result<()> {
        let mut norm = crate::norm::normalise(text);
        norm.truncate(MAX_NAME);
        if norm.is_empty() {
            return Ok(());
        }
        let s = &mut self.scratch;
        s.clear();
        s.extend_from_slice(norm.as_bytes());
        s.push(0);
        s.extend_from_slice(&abn.to_be_bytes());
        s.push(src);
        s.extend_from_slice(&rec.to_be_bytes());
        s.push(nidx);
        s.push(flags);
        s.extend_from_slice(&state);
        self.sorter.push(s)
    }

    /// Writes the three tables into `dir`; the number of names.
    pub fn finish(self, dir: &Path, sort_dir: &Path, run_bytes: usize) -> io::Result<u64> {
        let mut ent = BufWriter::with_capacity(1 << 20, File::create(dir.join("nm.ent"))?);
        let mut strs = BufWriter::with_capacity(1 << 20, File::create(dir.join("nm.str"))?);
        let mut tokens = Sorter::new(sort_dir, "tok", run_bytes);
        let (mut id, mut off) = (0u64, 0u64);
        let mut tok = Vec::new();
        self.sorter.for_each(|r| {
            let nul = r.iter().position(|&b| b == 0).ok_or_else(|| invalid("a name record without its end"))?;
            let (norm, rest) = (&r[..nul], &r[nul + 1..]);
            if rest.len() != 23 || norm.len() > MAX_NAME {
                return Err(invalid("a name record of the wrong size"));
            }
            let abn = u64::from_be_bytes(rest[0..8].try_into().map_err(|_| invalid("abn"))?);
            let rec = u64::from_be_bytes(rest[9..17].try_into().map_err(|_| invalid("rec"))?);
            let mut e = [0u8; ENTRY];
            e[0..8].copy_from_slice(&off.to_le_bytes());
            e[8..16].copy_from_slice(&abn.to_le_bytes());
            e[16..24].copy_from_slice(&rec.to_le_bytes());
            e[24] = norm.len() as u8;
            e[25] = rest[17];
            e[26] = rest[8];
            e[27] = rest[18];
            e[28..32].copy_from_slice(&rest[19..23]);
            ent.write_all(&e)?;
            strs.write_all(norm)?;
            off += norm.len() as u64;
            let id32 = u32::try_from(id).map_err(|_| invalid("more names than the index holds"))?;
            let mut seen: Vec<&[u8]> = Vec::new();
            for word in norm.split(|&b| b == b' ').filter(|w| !w.is_empty()) {
                if seen.contains(&word) {
                    continue;
                }
                seen.push(word);
                tok.clear();
                tok.extend_from_slice(word);
                tok.push(0);
                tok.extend_from_slice(&id32.to_be_bytes());
                tokens.push(&tok)?;
            }
            id += 1;
            Ok(())
        })?;
        ent.flush()?;
        strs.flush()?;
        crate::tokens::write(tokens, dir, "tok")?;
        Ok(id)
    }
}
