//! FlatGeobuf files by their web address, read by range: the index a level at a time, then the features a view needs, the ones a
//! sample passes over served as stand-ins (see the module above).

use super::{FgbSource, err};
use crate::source::remote::{self, Remote};
use crate::source::{Batch, Request, clamp_limit, in_request, stride};
use crate::{DataError, Feature, Result};
use flatgeobuf::{FallibleStreamingIterator, FgbReader};
use rustgis_geom::Extent;
use std::sync::Arc;

/// A packed R-tree node's size in bytes.
pub(super) const NODE: u64 = 40;
/// The most index bytes one level of a search fetches (past it, branches are thinned).
const INDEX_BUDGET: u64 = if cfg!(target_arch = "wasm32") { 4 << 20 } else { 8 << 20 };
/// Neighbouring branches followed together when a search thins the tree.
const BRANCH_RUN: usize = 32;
/// The most bytes of features one read fetches (past it, a sample of runs of them).
const FEATURE_BUDGET: u64 = if cfg!(target_arch = "wasm32") { 12 << 20 } else { 24 << 20 };
/// Consecutive features taken together when a read samples (runs of them, evenly spread: their
/// bytes are fetched together, where single features spread out would mean the whole file).
const RUN: usize = 512;
/// A feature's bytes guessed when the index doesn't give its end (more are read if needed).
const FEATURE_GUESS: u64 = 16 << 10;
/// A feature size prefix and an empty flatbuffers table: the stand-in for a feature a sample
/// passes over (the reader needs something to step past; smaller than any real feature).
const STAND_IN: [u8; 16] = [12, 0, 0, 0, 8, 0, 0, 0, 4, 0, 4, 0, 4, 0, 0, 0];
#[cfg(test)]
pub(crate) const STAND_IN_FOR_TESTS: [u8; 16] = STAND_IN;

/// A packed R-tree's levels, leaves first: each one's first node and end (node numbers), as the
/// format lays them out (root first in the file). `None` for a count no index could have.
pub(super) fn level_bounds(items: u64, node_size: u64) -> Option<Vec<(u64, u64)>> {
    if items == 0 || node_size < 2 || items > 1 << 40 {
        return None;
    }
    let mut counts = vec![items];
    let mut n = items;
    let mut total = items;
    while n > 1 {
        n = n.div_ceil(node_size);
        counts.push(n);
        total = total.checked_add(n)?;
    }
    let mut out = Vec::with_capacity(counts.len());
    let mut end = total;
    for c in counts {
        out.push((end - c, end));
        end -= c;
    }
    Some(out)
}

/// What an index search of a remote file found.
struct Hits {
    /// (offset in the feature section, feature number), in file order.
    items: Vec<(u64, u64)>,
    /// How many matches each hit stands for (more than one when the search had to thin the tree).
    thin: u64,
    /// The node runs the thinning passed over: (where in the file, how many nodes). The crate's
    /// own search reads them, so they are served as nodes that meet nothing.
    passed: Vec<(u64, u64)>,
}

impl FgbSource {
    /// The index's start and the feature section's, for a file whose header is held.
    fn sections(&self, f: &Remote) -> Result<(u64, u64)> {
        let head = f.bytes(8, 4).ok_or_else(remote::need)?;
        let size = head
            .get(..4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| err(flatgeobuf::Error::MissingMagicBytes))?;
        let index = 12 + u64::from(size);
        let count = self.info.count.unwrap_or(0);
        let levels = level_bounds(count, u64::from(flatgeobuf::packed_r_tree::PackedRTree::DEFAULT_NODE_SIZE))
            .ok_or_else(|| DataError::Format("FlatGeobuf: an index for an impossible number of features".into()))?;
        let nodes = levels.first().map(|l| l.1).unwrap_or(0);
        Ok((index, index.saturating_add(nodes.saturating_mul(NODE))))
    }

    /// The features a native extent meets, by the index, a level at a time: each level's nodes
    /// are asked for together, so a search takes as many rounds of fetching as the tree has
    /// levels. A level too large to fetch for one read is thinned (every n-th branch followed).
    fn remote_search(&self, f: &Remote, index: u64, e: &Extent) -> Result<Hits> {
        let count = self.info.count.unwrap_or(0);
        let node_size = u64::from(flatgeobuf::packed_r_tree::PackedRTree::DEFAULT_NODE_SIZE);
        let levels = level_bounds(count, node_size).ok_or_else(|| DataError::Format("FlatGeobuf: an impossible index".into()))?;
        let (leaf_start, leaf_end) = levels.first().copied().unwrap_or((0, 0));
        let bounds = flatgeobuf::packed_r_tree::NodeItem::bounds(e.min_x, e.min_y, e.max_x, e.max_y);
        let mut level = levels.len().saturating_sub(1);
        let mut queue: Vec<u64> = vec![levels.get(level).map(|l| l.0).unwrap_or(0)];
        let mut items = Vec::new();
        let mut thin = 1u64;
        let mut passed = Vec::new();
        while let Some(&(start, end)) = levels.get(level) {
            // Too many nodes to fetch for one view: follow an evenly spread part of them.
            let budget = usize::try_from(INDEX_BUDGET / (NODE * node_size)).unwrap_or(usize::MAX).max(1);
            if queue.len() > budget {
                // In runs of neighbouring branches: single ones spread out would be fetched as
                // one range (the gaps between them are smaller than a fetch).
                let step = queue.len().div_ceil(budget);
                let (kept, left): (Vec<(usize, u64)>, Vec<(usize, u64)>) =
                    queue.into_iter().enumerate().partition(|(i, _)| (i / BRANCH_RUN).is_multiple_of(step));
                for (_, n) in left {
                    passed.push((index + n * NODE, n.saturating_add(node_size).min(end).saturating_sub(n)));
                }
                queue = kept.into_iter().map(|(_, n)| n).collect();
                thin = thin.saturating_mul(step as u64);
            }
            let spans: Vec<(u64, u64)> =
                queue.iter().map(|&n| (n, n.saturating_add(node_size).min(end))).filter(|(a, b)| a < b && *a >= start).collect();
            // Every node of the level asked for before any is read.
            let mut held = true;
            for &(a, b) in &spans {
                held &= f.want(index + a * NODE, (b - a) * NODE);
            }
            if !held {
                return Err(remote::need());
            }
            let mut next = Vec::new();
            for (a, b) in spans {
                let bytes = f.bytes(index + a * NODE, (b - a) * NODE).ok_or_else(remote::need)?;
                for (k, raw) in bytes.as_chunks::<{ NODE as usize }>().0.iter().enumerate() {
                    let node = flatgeobuf::packed_r_tree::NodeItem::from_reader(raw.as_slice()).map_err(err)?;
                    if !bounds.intersects(&node) {
                        continue;
                    }
                    if level == 0 {
                        items.push((node.offset, a + k as u64 - leaf_start));
                    } else {
                        // A child must be a node of the level below.
                        let below = levels.get(level - 1).copied().unwrap_or((0, 0));
                        if node.offset < below.0 || node.offset >= below.1 {
                            return Err(DataError::Format("FlatGeobuf: an index node points outside its level".into()));
                        }
                        next.push(node.offset);
                    }
                }
            }
            if level == 0 || next.is_empty() {
                break;
            }
            queue = next;
            level -= 1;
        }
        let _ = leaf_end;
        Ok(Hits { items, thin, passed })
    }

    /// A request on a file by its web address: the index search, then the features the sample
    /// takes, fetched together before any is read; the others are passed over unread.
    pub(super) fn read_remote(&self, f: &Arc<Remote>, req: &Request) -> Result<Batch> {
        let limit = clamp_limit(req.limit);
        let native = match req.extent {
            Some(e) => self.reproject.extent_from_lonlat(&e),
            None => Extent::new(f64::MIN, f64::MIN, f64::MAX, f64::MAX),
        };
        let (index, features) = self.sections(f)?;
        let hits = self.remote_search(f, index, &native)?;
        let n = hits.items.len();
        // Each hit's bytes: to the next feature when that follows it, else a guess.
        let size = |k: usize| -> u64 {
            match (hits.items.get(k), hits.items.get(k + 1)) {
                (Some(&(a, i)), Some(&(b, j))) if j == i + 1 && b > a => (b - a).min(remote::READ_BUDGET),
                _ => FEATURE_GUESS,
            }
        };
        // A sample: runs of consecutive features, every `step`-th run (fewer when their bytes
        // would be more than a read may fetch).
        let mut step = usize::try_from(stride(n as u64, limit)).unwrap_or(usize::MAX).max(1);
        // Runs short enough for the sample to be spread over many places.
        let run = (limit / 256).clamp(1, RUN);
        let taken = |k: usize, step: usize| step == 1 || (k / run).is_multiple_of(step);
        loop {
            let bytes: u64 = (0..n).filter(|&k| taken(k, step)).map(size).sum();
            if bytes <= FEATURE_BUDGET || step >= n.max(1) {
                break;
            }
            step = step.saturating_mul(2);
        }
        let mut held = true;
        for k in (0..n).filter(|&k| taken(k, step)) {
            if let Some(&(off, _)) = hits.items.get(k) {
                held &= f.want(features + off, size(k));
            }
        }
        if !held {
            return Err(remote::need());
        }
        // The features passed over (and the first feature's size, read before any seek, when
        // the sample doesn't start there) are stand-ins.
        let feature = Arc::new(STAND_IN.to_vec());
        let mut at: std::collections::HashMap<u64, Arc<Vec<u8>>> = hits
            .items
            .iter()
            .enumerate()
            .filter(|(k, _)| !taken(*k, step))
            .map(|(_, (off, _))| (features + off, feature.clone()))
            .collect();
        if hits.items.first().is_none_or(|(off, _)| *off != 0) || !taken(0, step) {
            at.insert(features, feature.clone());
        }
        // Index nodes the search passed over: nodes that meet nothing (inverted bounds).
        let mut nothing = Vec::with_capacity(NODE as usize);
        for v in [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY] {
            nothing.extend_from_slice(&v.to_le_bytes());
        }
        nothing.extend_from_slice(&0u64.to_le_bytes());
        for &(pos, count) in &hits.passed {
            let bytes: Vec<u8> = nothing.iter().copied().cycle().take(usize::try_from(count * NODE).unwrap_or(0)).collect();
            at.insert(pos, Arc::new(bytes));
        }
        let stand_ins = Arc::new(remote::StandIns { at });
        let reader = FgbReader::open(remote::Reader::with_stand_ins(f.clone(), stand_ins)).map_err(err)?;
        let mut iter = reader.select_bbox(native.min_x, native.min_y, native.max_x, native.max_y).map_err(err)?;
        // The crate's search must find what this one did (it reads the nodes fetched for it).
        if iter.features_count() != Some(n) {
            return Err(DataError::Format("FlatGeobuf: the index search found a different set of features".into()));
        }
        let mut out = Batch::default();
        let mut k = 0usize;
        while let Some(feat) = iter.next().map_err(err)? {
            let this = k;
            k += 1;
            if !taken(this, step) {
                continue;
            }
            let id = hits.items.get(this).map(|(_, i)| i + 1).unwrap_or(this as u64 + 1);
            let Some(geometry) = self.geometry_in(feat, &mut out.skipped) else { continue };
            if !in_request(&geometry, req) {
                continue;
            }
            let Some(attrs) = self.attrs_in(feat, &mut out.skipped) else { continue };
            out.features.push(Feature { id, geometry: Some(geometry), attrs });
            if out.features.len() >= limit {
                break;
            }
        }
        out.matched = (n as u64).saturating_mul(hits.thin);
        out.complete = step == 1 && hits.thin == 1;
        Ok(out)
    }
}
