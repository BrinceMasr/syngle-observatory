//! A large FlatGeobuf file on disk, read through its index's leaf level held in memory.
//!
//! Read the crate's way, a view of a million-point file visits every feature record (a seek, a read and a flatbuffers check
//! each, about 100 ns) to keep one in four of them, and a zoom out and back does it again. The leaf level of the file's own
//! packed R-tree has, for each feature in file order, its bounds and where it starts: 40 bytes a feature, read once. A request is
//! then answered from memory. The leaves its extent meets are counted and sampled as the index search would (the same numbers
//! and ids, the same sample, the same `matched` and `complete`), and only the features the sample keeps are read from the file
//! and decoded by the crate's own reader, in runs on as many threads as the machine has.
//!
//! This is an optimisation with a way out: anything unexpected (a file that changed, offsets that do not climb, a record past
//! the end of the file, a thread that could not start) answers `None` and the caller reads the crate's way. Tables are held
//! within a budget for the whole process, so many large files cannot hold more than that.

use super::FgbSource;
use crate::Feature;
use crate::source::{Batch, Request, clamp_limit, stride};
use flatgeobuf::packed_r_tree::{NodeItem, PackedRTree};
use flatgeobuf::{FallibleStreamingIterator, FgbReader};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

/// Files with fewer features are read the crate's way: it is quick enough, and a small file is not worth a table.
const MIN_FEATURES: u64 = 100_000;
/// The most features one table holds.
const MAX_FEATURES: u64 = 2_000_000;
/// The most bytes of tables held at once by every file open in the process.
const BUDGET: usize = 256 << 20;
static HELD: AtomicUsize = AtomicUsize::new(0);

/// The largest header the crate reads.
const MAX_HEADER: u64 = 10 << 20;
/// The largest feature record taken as real: a size prefix past it is not a feature.
const MAX_RECORD: u64 = 64 << 20;
/// Leaves read and parsed at a time when a table is made.
const LEAVES_AT_A_TIME: usize = 32 << 10;

/// What a read does with the machine: how many threads decode, how many features a thread gathers and decodes at a time, and
/// how it reads the file. Fixed in a build; a test sets its own to reach every path with a small file.
struct Plan {
    threads: usize,
    run: usize,
    /// Kept features further apart than this on average (bytes between them) are fetched one by one, `near` bytes at a read.
    dense_gap: u64,
    span: usize,
    near: usize,
}

impl Plan {
    fn standard() -> Plan {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
        Plan { threads, run: 4096, dense_gap: 32 << 10, span: 256 << 10, near: 4 << 10 }
    }
}

/// The index's leaves and what is needed to read the features they point to.
pub(super) struct Table {
    nodes: Vec<NodeItem>,
    /// The file's bytes before the index: magic, header size and header.
    head: Vec<u8>,
    /// The index's length, and where the feature section starts.
    index_len: u64,
    features_at: u64,
    /// The file as it was when the table was made.
    len: u64,
    modified: Option<SystemTime>,
    /// The bytes of the process's budget this table holds (none for one made outside `load`).
    held: usize,
}

impl Drop for Table {
    fn drop(&mut self) {
        HELD.fetch_sub(self.held, Ordering::Relaxed);
    }
}

const NODE_BYTES: usize = 40;

impl Table {
    /// The table of a file of `count` features, if it is one worth holding and within the budget.
    fn load(path: &Path, count: u64) -> Option<Table> {
        if !(MIN_FEATURES..=MAX_FEATURES).contains(&count) {
            return None;
        }
        let bytes = usize::try_from(count).ok()?.checked_mul(NODE_BYTES)?;
        if HELD.fetch_add(bytes, Ordering::Relaxed).saturating_add(bytes) > BUDGET {
            HELD.fetch_sub(bytes, Ordering::Relaxed);
            return None;
        }
        // A table that is made keeps the bytes (its `Drop` gives them back); one that is not gives them back now.
        match Table::build(path, count) {
            Some(mut table) => {
                table.held = bytes;
                Some(table)
            }
            None => {
                HELD.fetch_sub(bytes, Ordering::Relaxed);
                None
            }
        }
    }

    /// Reads the leaves. `None` for a file that is not what its header says: a table that does not fit, offsets that do not
    /// climb, a record that would start past the end.
    fn build(path: &Path, count: u64) -> Option<Table> {
        let mut file = File::open(path).ok()?;
        let meta = file.metadata().ok()?;
        let len = meta.len();
        let mut start = [0u8; 12];
        file.read_exact(&mut start).ok()?;
        if start[..3] != *b"fgb" || start[4..7] != *b"fgb" {
            return None;
        }
        let header = u64::from(u32::from_le_bytes([start[8], start[9], start[10], start[11]]));
        if header > MAX_HEADER {
            return None;
        }
        let index_at = 12 + header;
        let levels = super::ranges::level_bounds(count, u64::from(PackedRTree::DEFAULT_NODE_SIZE))?;
        let (leaf_start, total) = *levels.first()?;
        let index_len = total.checked_mul(super::ranges::NODE)?;
        let features_at = index_at.checked_add(index_len)?;
        if features_at > len {
            return None;
        }
        let mut head = vec![0u8; usize::try_from(index_at).ok()?];
        file.seek(SeekFrom::Start(0)).ok()?;
        file.read_exact(&mut head).ok()?;
        file.seek(SeekFrom::Start(index_at.checked_add(leaf_start.checked_mul(super::ranges::NODE)?)?)).ok()?;
        let (mut nodes, mut left, mut last) = (Vec::with_capacity(usize::try_from(count).ok()?), count, None::<u64>);
        let mut raw = vec![0u8; LEAVES_AT_A_TIME * NODE_BYTES];
        while left > 0 {
            let n = usize::try_from(left).ok()?.min(LEAVES_AT_A_TIME);
            let block = raw.get_mut(..n * NODE_BYTES)?;
            file.read_exact(block).ok()?;
            for chunk in block.as_chunks::<NODE_BYTES>().0 {
                let node = NodeItem::from_reader(chunk.as_slice()).ok()?;
                // Feature records climb through the section, and each starts inside it.
                if last.is_some_and(|l| node.offset <= l) || node.offset >= len - features_at {
                    return None;
                }
                last = Some(node.offset);
                nodes.push(node);
            }
            left -= n as u64;
        }
        Some(Table { nodes, head, index_len, features_at, len, modified: meta.modified().ok(), held: 0 })
    }

    /// Whether the file is still the one the table was made from.
    fn still_current(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.len() == self.len && m.modified().ok() == self.modified)
    }

    /// The features a request keeps: how many leaves it meets, and the (id, offset in the feature section) of every
    /// `step`-th one, in file order. No extent meets all of them.
    fn pick(&self, bounds: Option<&NodeItem>, limit: usize) -> (u64, u64, Vec<(u64, u64)>) {
        let meets = |n: &NodeItem| bounds.is_none_or(|b| b.intersects(n));
        let matched = self.nodes.iter().filter(|n| meets(n)).count() as u64;
        let step = stride(matched, limit);
        let mut picks = Vec::with_capacity(usize::try_from(matched.div_ceil(step)).unwrap_or(0));
        let mut seen = 0u64;
        for (i, node) in self.nodes.iter().enumerate() {
            if !meets(node) {
                continue;
            }
            if seen.is_multiple_of(step) {
                picks.push((i as u64 + 1, node.offset));
            }
            seen += 1;
        }
        (matched, step, picks)
    }
}

/// A file read in pieces: `at` serves the bytes from a position out of the piece it holds, fetching another when it holds
/// none that cover them.
struct Spans<'a> {
    file: &'a mut File,
    buf: Vec<u8>,
    from: u64,
    len: u64,
    size: usize,
}

impl Spans<'_> {
    /// The file's bytes from `at` to the end of the piece, at least `min` of them.
    fn at(&mut self, at: u64, min: usize) -> Option<&[u8]> {
        let held =
            at.checked_sub(self.from).and_then(|off| usize::try_from(off).ok()).filter(|off| self.buf.len().saturating_sub(*off) >= min);
        let off = match held {
            Some(off) => off,
            None => {
                let room = self.len.checked_sub(at)?;
                let want = usize::try_from(room).unwrap_or(usize::MAX).min(self.size.max(min));
                if want < min {
                    return None;
                }
                self.buf.clear();
                self.buf.resize(want, 0);
                self.from = at;
                self.file.seek(SeekFrom::Start(at)).ok()?;
                self.file.read_exact(&mut self.buf).ok()?;
                0
            }
        };
        self.buf.get(off..)
    }
}

/// What the crate's reader sees: the file's head, a gap where the index is (it only ever seeks over it), then the records
/// that were gathered, one after the other.
struct Spliced<'a> {
    head: &'a [u8],
    gap: u64,
    body: Vec<u8>,
    pos: u64,
}

impl Spliced<'_> {
    fn end(&self) -> u64 {
        self.head.len() as u64 + self.gap + self.body.len() as u64
    }
}

impl Read for Spliced<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let (head, body_at) = (self.head.len() as u64, self.head.len() as u64 + self.gap);
        let n = if self.pos < head {
            let from = usize::try_from(self.pos).unwrap_or(usize::MAX);
            let part = self.head.get(from..).unwrap_or_default();
            let n = part.len().min(out.len());
            out[..n].copy_from_slice(&part[..n]);
            n
        } else if self.pos < body_at {
            let n = usize::try_from(body_at - self.pos).unwrap_or(usize::MAX).min(out.len());
            out[..n].fill(0);
            n
        } else {
            let from = usize::try_from(self.pos - body_at).unwrap_or(usize::MAX);
            let part = self.body.get(from..).unwrap_or_default();
            let n = part.len().min(out.len());
            out[..n].copy_from_slice(&part[..n]);
            n
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Spliced<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.end().checked_add_signed(d),
        };
        self.pos = target.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start"))?;
        Ok(self.pos)
    }
}

impl FgbSource {
    /// A request on a large file on disk, answered from its table; `None` to read it the crate's way.
    pub(super) fn read_resident(&self, path: &Path, req: &Request) -> Option<Batch> {
        let count = self.info.count?;
        if !self.info.indexed {
            return None;
        }
        let table = self.resident.get_or_init(|| Table::load(path, count)).as_ref()?;
        if table.nodes.len() as u64 != count || !table.still_current(path) {
            return None;
        }
        self.answer(table, path, req, &Plan::standard())
    }

    fn answer(&self, table: &Table, path: &Path, req: &Request, plan: &Plan) -> Option<Batch> {
        let limit = clamp_limit(req.limit);
        let native = req.extent.map(|e| self.reproject.extent_from_lonlat(&e)).filter(|_| !self.covers(req));
        let bounds = native.map(|e| NodeItem::bounds(e.min_x, e.min_y, e.max_x, e.max_y));
        let (matched, step, picks) = table.pick(bounds.as_ref(), limit);
        let mean = (table.len - table.features_at) / table.nodes.len().max(1) as u64;
        let dense = mean.saturating_mul(step) <= plan.dense_gap;
        let (features, skipped) = self.decode(table, path, &picks, req, plan, if dense { plan.span } else { plan.near })?;
        Some(Batch { features, matched, complete: step == 1, skipped })
    }

    /// Decodes the picked features, in the picks' order, on as many threads as the plan has and the picks are worth.
    fn decode(
        &self,
        table: &Table,
        path: &Path,
        picks: &[(u64, u64)],
        req: &Request,
        plan: &Plan,
        piece: usize,
    ) -> Option<(Vec<Feature>, usize)> {
        let parts = plan.threads.min(picks.len().div_ceil(plan.run.max(1))).max(1);
        if parts == 1 {
            return self.decode_picks(table, path, picks, req, plan, piece);
        }
        let chunks: Vec<&[(u64, u64)]> = picks.chunks(picks.len().div_ceil(parts)).collect();
        let mut done = Vec::with_capacity(chunks.len());
        std::thread::scope(|scope| {
            let started: Vec<_> = chunks
                .iter()
                .map(|chunk| {
                    std::thread::Builder::new()
                        .name("rustgis-fgb".into())
                        .spawn_scoped(scope, move || self.decode_picks(table, path, chunk, req, plan, piece))
                })
                .collect();
            for (chunk, handle) in chunks.iter().zip(started) {
                done.push(match handle {
                    Ok(handle) => handle.join().ok().flatten(),
                    // No thread to run it on: here.
                    Err(_) => self.decode_picks(table, path, chunk, req, plan, piece),
                });
            }
        });
        let mut features = Vec::with_capacity(picks.len());
        let mut skipped = 0;
        for part in done {
            let (f, s) = part?;
            features.extend(f);
            skipped += s;
        }
        Some((features, skipped))
    }

    /// Reads and decodes picks in runs: a run's records are copied one after the other into a buffer, which the crate's reader
    /// then walks as it would the feature section of a file.
    fn decode_picks(
        &self,
        table: &Table,
        path: &Path,
        picks: &[(u64, u64)],
        req: &Request,
        plan: &Plan,
        piece: usize,
    ) -> Option<(Vec<Feature>, usize)> {
        let mut file = File::open(path).ok()?;
        let mut spans = Spans { file: &mut file, buf: Vec::new(), from: 0, len: table.len, size: piece };
        let mut features = Vec::with_capacity(picks.len());
        let mut skipped = 0;
        for run in picks.chunks(plan.run.max(1)) {
            let mut body = Vec::new();
            for &(_, offset) in run {
                let at = table.features_at.checked_add(offset)?;
                let size = u64::from(u32::from_le_bytes(spans.at(at, 4)?.get(..4)?.try_into().ok()?));
                if size > MAX_RECORD || at.checked_add(4 + size)? > table.len {
                    return None;
                }
                let total = usize::try_from(4 + size).ok()?;
                body.extend_from_slice(spans.at(at, total)?.get(..total)?);
            }
            let reader = FgbReader::open(Spliced { head: &table.head, gap: table.index_len, body, pos: 0 }).ok()?;
            let mut iter = reader.select_all().ok()?;
            let mut taken = 0;
            while let Some(f) = iter.next().ok()? {
                let (id, _) = *run.get(taken)?;
                taken += 1;
                let Some(geometry) = self.geometry_in(f, &mut skipped) else { continue };
                if !crate::source::in_request(&geometry, req) {
                    continue;
                }
                let Some(attrs) = self.attrs_in(f, &mut skipped) else { continue };
                features.push(Feature { id, geometry: Some(geometry), attrs });
            }
            if taken != run.len() {
                return None;
            }
        }
        Some((features, skipped))
    }
}

#[cfg(test)]
mod tests;
