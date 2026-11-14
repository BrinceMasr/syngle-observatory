//! An external sort of byte records, compared as bytes. A record is whatever the caller makes it (a key in big-endian
//! form followed by its payload), so the order is the order of the bytes: deterministic and the same on every machine.
//! Records collect in a run of bounded size; a full run is sorted and written to a temporary file; `for_each` merges
//! the runs. A build that fits one run never touches the disk.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

pub struct Sorter {
    dir: PathBuf,
    tag: String,
    run_bytes: usize,
    buf: Vec<u8>,
    index: Vec<(u32, u32)>,
    runs: Vec<PathBuf>,
}

impl Sorter {
    pub fn new(dir: &Path, tag: &str, run_bytes: usize) -> Sorter {
        Sorter {
            dir: dir.to_path_buf(),
            tag: tag.to_string(),
            run_bytes: run_bytes.clamp(1 << 10, 1 << 30),
            buf: Vec::new(),
            index: Vec::new(),
            runs: Vec::new(),
        }
    }

    pub fn push(&mut self, record: &[u8]) -> io::Result<()> {
        if !self.index.is_empty() && self.buf.len() + record.len() > self.run_bytes {
            self.spill()?;
        }
        let start = u32::try_from(self.buf.len()).map_err(|_| crate::rd::invalid("a sort run outgrew its limit"))?;
        let len = u32::try_from(record.len()).map_err(|_| crate::rd::invalid("a sort record is too long"))?;
        self.buf.extend_from_slice(record);
        self.index.push((start, len));
        Ok(())
    }

    fn sort_run(&mut self) {
        let buf = &self.buf;
        let slice = |&(s, l): &(u32, u32)| buf.get(s as usize..(s + l) as usize).unwrap_or_default();
        self.index.sort_unstable_by(|a, b| slice(a).cmp(slice(b)));
    }

    fn spill(&mut self) -> io::Result<()> {
        self.sort_run();
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{}-run-{}", self.tag, self.runs.len()));
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
        for &(s, l) in &self.index {
            w.write_all(&l.to_le_bytes())?;
            w.write_all(self.buf.get(s as usize..(s + l) as usize).unwrap_or_default())?;
        }
        w.flush()?;
        self.runs.push(path);
        self.buf.clear();
        self.index.clear();
        Ok(())
    }

    /// Calls `f` with every record in byte order.
    pub fn for_each(mut self, mut f: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        if self.runs.is_empty() {
            self.sort_run();
            for &(s, l) in &self.index {
                f(self.buf.get(s as usize..(s + l) as usize).unwrap_or_default())?;
            }
            return Ok(());
        }
        if !self.index.is_empty() {
            self.spill()?;
        }
        self.buf = Vec::new();
        let mut readers = Vec::new();
        let mut heap = BinaryHeap::new();
        for path in &self.runs {
            let mut r = BufReader::with_capacity(1 << 18, File::open(path)?);
            if let Some(rec) = next_record(&mut r)? {
                heap.push(Reverse((rec, readers.len())));
            }
            readers.push(r);
        }
        while let Some(Reverse((rec, from))) = heap.pop() {
            f(&rec)?;
            if let Some(reader) = readers.get_mut(from)
                && let Some(next) = next_record(reader)?
            {
                heap.push(Reverse((next, from)));
            }
        }
        Ok(())
    }
}

fn next_record(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut rec = vec![0u8; u32::from_le_bytes(len) as usize];
    r.read_exact(&mut rec)?;
    Ok(Some(rec))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(run_bytes: usize) -> Vec<Vec<u8>> {
        let dir = std::env::temp_dir().join(format!("obs-gov-sort-{}-{run_bytes}", std::process::id()));
        let mut s = Sorter::new(&dir, "t", run_bytes);
        for i in 0..5000u32 {
            let k = i.wrapping_mul(2_654_435_761) % 1000;
            s.push(&[&k.to_be_bytes()[..], &i.to_be_bytes()[..]].concat()).unwrap();
        }
        let mut out = Vec::new();
        s.for_each(|r| {
            out.push(r.to_vec());
            Ok(())
        })
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn many_runs_merge_to_the_same_order_as_one() {
        let one = sorted(1 << 30);
        let many = sorted(2000);
        assert_eq!(one.len(), 5000);
        assert!(one.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(one, many);
    }
}
