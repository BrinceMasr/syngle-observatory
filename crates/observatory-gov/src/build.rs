//! Builds a snapshot's tables from the official files (plain, or inside a ZIP), one source at a time, as a stream: no file
//! is held whole in memory, and every table that has to be sorted goes through the external sorter, so the memory a build
//! needs does not grow with the ABN extract (18 million entities).

use crate::abr;
use crate::asic;
use crate::codec::{AbrRecord, BusinessName, Company, Dicts, Interner};
use crate::names_build::{FLAG_ACTIVE, FLAG_IN_ABR, NameSink, SRC_ABR, SRC_ASIC, SRC_BN, state4};
use crate::rd::invalid;
use crate::sort::Sorter;
use crate::table::{Blob, le64};
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

pub struct SourcePlan {
    pub name: &'static str,
    pub files: Vec<PathBuf>,
}

pub struct Built {
    pub records: BTreeMap<&'static str, u64>,
    pub dicts: Dicts,
}

/// A file of records, appended one after another: a little-endian u32 length and the payload.
struct RecWriter {
    w: BufWriter<File>,
    pos: u64,
}

impl RecWriter {
    fn create(path: &Path) -> io::Result<RecWriter> {
        Ok(RecWriter { w: BufWriter::with_capacity(1 << 20, File::create(path)?), pos: 0 })
    }

    fn append(&mut self, payload: &[u8]) -> io::Result<u64> {
        let len = u32::try_from(payload.len()).map_err(|_| invalid("a record is too long"))?;
        let at = self.pos;
        self.w.write_all(&len.to_le_bytes())?;
        self.w.write_all(payload)?;
        self.pos += 4 + u64::from(len);
        Ok(at)
    }

    fn finish(mut self) -> io::Result<()> {
        self.w.flush()
    }
}

fn zip_error(e: zip::result::ZipError) -> io::Error {
    invalid(format!("the ZIP file cannot be read: {e}"))
}

/// Calls `f` with each text stream a file holds: the file itself, or each file of a ZIP in name order.
pub fn each_stream(path: &Path, f: &mut dyn FnMut(&mut dyn BufRead) -> io::Result<()>) -> io::Result<()> {
    let is_zip = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if !is_zip {
        let mut r = BufReader::with_capacity(1 << 20, File::open(path)?);
        return f(&mut r);
    }
    let mut archive = zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(zip_error)?;
    let mut names: Vec<(String, usize)> = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(zip_error)?;
        if entry.is_file() && !entry.name().starts_with("__MACOSX") {
            names.push((entry.name().to_string(), i));
        }
    }
    names.sort();
    if names.is_empty() {
        return Err(invalid("the ZIP file holds no files"));
    }
    for (_, i) in names {
        let entry = archive.by_index(i).map_err(zip_error)?;
        let mut r = BufReader::with_capacity(1 << 20, entry);
        f(&mut r)?;
    }
    Ok(())
}

fn pair(a: u64, b: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&a.to_be_bytes());
    out[8..].copy_from_slice(&b.to_be_bytes());
    out
}

/// A sorted run of `(a, b)` pairs, written as a table of little-endian pairs.
fn write_pairs(sorter: Sorter, path: &Path) -> io::Result<()> {
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path)?);
    sorter.for_each(|r| {
        let a = u64::from_be_bytes(r.get(0..8).and_then(|s| s.try_into().ok()).ok_or_else(|| invalid("pair"))?);
        let b = u64::from_be_bytes(r.get(8..16).and_then(|s| s.try_into().ok()).ok_or_else(|| invalid("pair"))?);
        w.write_all(&a.to_le_bytes())?;
        w.write_all(&b.to_le_bytes())
    })?;
    w.flush()
}

/// The entities of the ABN extract by number, for the stages that join to them (the ASIC names carry an ABN).
struct AbrLookup {
    idx: Vec<u8>,
    rec: Option<Blob>,
    dicts: Dicts,
}

impl AbrLookup {
    fn none() -> AbrLookup {
        AbrLookup { idx: Vec::new(), rec: None, dicts: Dicts::default() }
    }

    /// Whether the entity is active, and its state, when the ABN is in the extract.
    fn find(&self, abn: u64) -> io::Result<Option<(bool, [u8; 4])>> {
        let (mut lo, mut hi) = (0usize, self.idx.len() / 16);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if le64(&self.idx, mid * 16) < abn { lo = mid + 1 } else { hi = mid }
        }
        let Some(rec) = &self.rec else { return Ok(None) };
        if lo >= self.idx.len() / 16 || le64(&self.idx, lo * 16) != abn {
            return Ok(None);
        }
        let payload = rec.record(le64(&self.idx, lo * 16 + 8))?;
        let r = AbrRecord::decode(abn, &payload, &self.dicts).ok_or_else(|| invalid("a record just written does not decode"))?;
        Ok(Some((r.active, state4(&r.state))))
    }
}

/// Where an entity's flags and state come from: the ABN extract when it holds the ABN, else the record's own status.
fn entity_of(abr: &AbrLookup, abn: u64, own_active: bool) -> io::Result<(u8, [u8; 4])> {
    if abn != 0
        && let Some((active, state)) = abr.find(abn)?
    {
        return Ok((FLAG_IN_ABR | (u8::from(active) * FLAG_ACTIVE), state));
    }
    Ok((u8::from(own_active) * FLAG_ACTIVE, [0; 4]))
}

pub fn build(plan: &[SourcePlan], tmp: &Path, run_bytes: usize) -> io::Result<Built> {
    let sort_dir = tmp.join("sort");
    std::fs::create_dir_all(&sort_dir)?;
    let mut names = NameSink::new(&sort_dir, run_bytes);
    let mut records = BTreeMap::new();
    let (mut entity_types, mut name_types) = (Interner::default(), Interner::default());
    let mut abr_lookup = AbrLookup::none();
    let files_of = |name: &str| plan.iter().find(|p| p.name == name).map(|p| p.files.as_slice());

    if let Some(files) = files_of("abr") {
        let mut rec_w = RecWriter::create(&tmp.join("abn.rec"))?;
        let mut by_abn = Sorter::new(&sort_dir, "abn", run_bytes);
        let mut by_acn = Sorter::new(&sort_dir, "acn", run_bytes);
        let (mut count, mut payload) = (0u64, Vec::new());
        for file in files {
            each_stream(file, &mut |r| {
                abr::parse(r, |rec| {
                    payload.clear();
                    rec.encode(&mut payload, &mut entity_types, &mut name_types);
                    let off = rec_w.append(&payload)?;
                    by_abn.push(&pair(rec.abn, off))?;
                    if rec.acn != 0 {
                        by_acn.push(&pair(u64::from(rec.acn), rec.abn))?;
                    }
                    let (flags, state) = (FLAG_IN_ABR | (u8::from(rec.active) * FLAG_ACTIVE), state4(&rec.state));
                    names.add(&rec.name, rec.abn, SRC_ABR, off, 0, flags, state)?;
                    for (i, (_, text)) in rec.names.iter().take(254).enumerate() {
                        names.add(text, rec.abn, SRC_ABR, off, (i + 1) as u8, flags, state)?;
                    }
                    count += 1;
                    Ok(())
                })
                .map(|_| ())
            })?;
        }
        rec_w.finish()?;
        write_pairs(by_abn, &tmp.join("abn.idx"))?;
        write_pairs(by_acn, &tmp.join("acn.idx"))?;
        records.insert("abr", count);
        let dicts = Dicts { entity: entity_types.items.clone(), name_type: name_types.items.clone() };
        abr_lookup = AbrLookup { idx: std::fs::read(tmp.join("abn.idx"))?, rec: Some(Blob::open(&tmp.join("abn.rec"))?), dicts };
    }

    if let Some(files) = files_of("asic") {
        let mut companies: Vec<Company> = Vec::new();
        for file in files {
            each_stream(file, &mut |r| {
                companies.extend(asic::read_companies(r)?);
                Ok(())
            })?;
        }
        companies.sort_by_key(|c| c.acn);
        let mut rec_w = RecWriter::create(&tmp.join("co.rec"))?;
        let mut idx = BufWriter::with_capacity(1 << 20, File::create(tmp.join("co.idx"))?);
        let mut payload = Vec::new();
        for c in &companies {
            payload.clear();
            c.encode(&mut payload);
            let off = rec_w.append(&payload)?;
            idx.write_all(&u64::from(c.acn).to_le_bytes())?;
            idx.write_all(&off.to_le_bytes())?;
            if let (true, Some(name)) = (c.abn != 0, c.name.as_deref()) {
                let (flags, state) = entity_of(&abr_lookup, c.abn, c.status.as_deref() == Some("Registered"))?;
                names.add(name, c.abn, SRC_ASIC, off, 0, flags, state)?;
                for (i, former) in c.former.iter().take(254).enumerate() {
                    names.add(former, c.abn, SRC_ASIC, off, (i + 1) as u8, flags, state)?;
                }
            }
        }
        rec_w.finish()?;
        idx.flush()?;
        records.insert("asic", companies.len() as u64);
    }

    if let Some(files) = files_of("asicBn") {
        let mut rec_w = RecWriter::create(&tmp.join("bn.rec"))?;
        let mut by_abn = Sorter::new(&sort_dir, "bn", run_bytes);
        let (mut count, mut payload) = (0u64, Vec::new());
        // A registration with no ABN is one entity by its name, however many rows say it.
        let mut nameless: HashSet<String> = HashSet::new();
        for file in files {
            each_stream(file, &mut |r| {
                count += asic::read_business_names(r, |b: BusinessName| {
                    payload.clear();
                    b.encode(&mut payload);
                    let off = rec_w.append(&payload)?;
                    if b.abn != 0 {
                        by_abn.push(&pair(b.abn, off))?;
                    } else if !nameless.insert(b.name.clone()) {
                        return Ok(());
                    }
                    let (flags, state) = entity_of(&abr_lookup, b.abn, b.status == "Registered")?;
                    names.add(&b.name, b.abn, SRC_BN, off, 0, flags, state)
                })?;
                Ok(())
            })?;
        }
        rec_w.finish()?;
        write_pairs(by_abn, &tmp.join("bn.idx"))?;
        records.insert("asicBn", count);
    }

    names.finish(tmp, &sort_dir, run_bytes)?;
    std::fs::remove_dir_all(&sort_dir)?;
    Ok(Built { records, dicts: Dicts { entity: entity_types.items, name_type: name_types.items } })
}
