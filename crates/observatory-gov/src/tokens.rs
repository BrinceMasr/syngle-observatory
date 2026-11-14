//! Word lists: for each word, the sorted ids of the entries that have it. Three files per index, named by a prefix:
//! `<p>.key` (24-byte entries sorted by word: where the word is, where its ids start, how many, how long the word is),
//! `<p>.str` (the words, back to back) and `<p>.post` (the ids, little-endian u32, ascending within a word). "Every word,
//! any order" is an intersection of lists. Names use the prefix `tok`, addresses `atok`.

use crate::error::{GovError, OrDamaged, Res};
use crate::rd::invalid;
use crate::snap::Snap;
use crate::sort::Sorter;
use crate::table::{Blob, le32, le64};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

pub const TOKEN: usize = 24;

fn put_token(key: &mut BufWriter<File>, word: &[u8], start: u64, count: u32, str_off: u64) -> io::Result<()> {
    let mut e = [0u8; TOKEN];
    e[0..8].copy_from_slice(&str_off.to_le_bytes());
    e[8..16].copy_from_slice(&start.to_le_bytes());
    e[16..20].copy_from_slice(&count.to_le_bytes());
    e[20..24].copy_from_slice(&(word.len() as u32).to_le_bytes());
    key.write_all(&e)
}

/// Writes the three files from records `word 0x00 id(u32, big-endian)`, sorted by the sorter.
pub fn write(tokens: Sorter, dir: &Path, prefix: &str) -> io::Result<()> {
    let create = |ext: &str| File::create(dir.join(format!("{prefix}.{ext}"))).map(|f| BufWriter::with_capacity(1 << 20, f));
    let (mut key, mut strs, mut post) = (create("key")?, create("str")?, create("post")?);
    let (mut str_off, mut post_off) = (0u64, 0u64);
    let mut current: Option<(Vec<u8>, u64, u32)> = None;
    tokens.for_each(|r| {
        let nul = r.iter().position(|&b| b == 0).ok_or_else(|| invalid("a word record without its end"))?;
        let (word, id) = (&r[..nul], r.get(nul + 1..nul + 5).ok_or_else(|| invalid("a word record of the wrong size"))?);
        match current.as_mut() {
            Some((w, _, count)) if w.as_slice() == word => *count += 1,
            _ => {
                if let Some((w, start, count)) = current.take() {
                    put_token(&mut key, &w, start, count, str_off - w.len() as u64)?;
                }
                strs.write_all(word)?;
                str_off += word.len() as u64;
                current = Some((word.to_vec(), post_off, 1));
            }
        }
        let le = u32::from_be_bytes(id.try_into().map_err(|_| invalid("id"))?).to_le_bytes();
        post.write_all(&le)?;
        post_off += 1;
        Ok(())
    })?;
    if let Some((w, start, count)) = current.take() {
        put_token(&mut key, &w, start, count, str_off - w.len() as u64)?;
    }
    key.flush()?;
    strs.flush()?;
    post.flush()
}

/// Where a word's ids are in `<prefix>.post`: (first id's position, how many); `None` when no entry has the word.
pub fn find(snap: &Snap, prefix: &str, word: &[u8]) -> Res<Option<(u64, u64)>> {
    let (tok, strs) = (snap.fixed(&format!("{prefix}.key"), TOKEN as u64)?, snap.blob(&format!("{prefix}.str"))?);
    let mut buf = Vec::new();
    let at = tok
        .partition_point(0, |e| {
            buf.resize(le32(e, 20) as usize, 0);
            strs.read_exact_at(le64(e, 0), &mut buf)?;
            Ok(buf.as_slice() < word)
        })
        .damaged()?;
    if at >= tok.count {
        return Ok(None);
    }
    let e = tok.entry(at).damaged()?;
    buf.resize(le32(&e, 20) as usize, 0);
    strs.read_exact_at(le64(&e, 0), &mut buf).damaged()?;
    Ok((buf.as_slice() == word).then(|| (le64(&e, 8), u64::from(le32(&e, 16)))))
}

/// The ids of a list.
pub fn read(post: &Blob, start: u64, count: u64) -> Res<Vec<u32>> {
    let n = usize::try_from(count).map_err(|_| GovError::damaged("a word list is too long"))?;
    let raw = post.read_vec(start * 4, n * 4).damaged()?;
    Ok(raw.chunks_exact(4).map(|c| le32(c, 0)).collect())
}

/// Whether the list holds `id`, looking from `*from` on (ids are asked in ascending order, so the search only moves
/// forward): a gallop and a binary search, a few small reads, for a list far longer than the ids asked.
pub fn contains(post: &Blob, start: u64, count: u64, id: u32, from: &mut u64) -> Res<bool> {
    let at = |i: u64| -> Res<u32> {
        let mut b = [0u8; 4];
        post.read_exact_at((start + i) * 4, &mut b).damaged()?;
        Ok(u32::from_le_bytes(b))
    };
    let (mut lo, mut step) = (*from, 1u64);
    let mut hi = lo;
    while hi < count && at(hi)? < id {
        lo = hi + 1;
        hi = hi.saturating_add(step);
        step = step.saturating_mul(2);
    }
    hi = hi.min(count);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if at(mid)? < id { lo = mid + 1 } else { hi = mid }
    }
    *from = lo;
    Ok(lo < count && at(lo)? == id)
}
