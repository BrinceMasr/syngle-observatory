//! Reading the index by position. `unsafe` is forbidden here, so no memory map: a file is read at an offset
//! (`pread` on Unix, `seek_read` on Windows) and the operating system's page cache is the cache. Tables are sorted
//! runs of fixed-width entries, found by binary search, so a lookup is a few small reads and opens nothing it does not need.

use crate::rd::invalid;
use std::fs::File;
use std::io;
use std::path::Path;

pub struct Blob {
    file: File,
    pub len: u64,
}

#[cfg(unix)]
fn read_some(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(windows)]
fn read_some(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

impl Blob {
    pub fn open(path: &Path) -> io::Result<Blob> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Blob { file, len })
    }

    /// Reads as much as is there, up to `buf.len()`; the count read.
    pub fn read_up_to(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            let Some(rest) = buf.get_mut(done..) else { break };
            match read_some(&self.file, rest, offset + done as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }

    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        if self.read_up_to(offset, buf)? == buf.len() {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the index file is shorter than its table says"))
        }
    }

    pub fn read_vec(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut v = vec![0u8; len];
        self.read_exact_at(offset, &mut v)?;
        Ok(v)
    }

    /// One record of a record file: a little-endian u32 length, then the payload.
    pub fn record(&self, offset: u64) -> io::Result<Vec<u8>> {
        const MAX: usize = 64 << 20;
        let mut first = [0u8; 512];
        let got = self.read_up_to(offset, &mut first)?;
        let head = first.get(..4).filter(|_| got >= 4).ok_or_else(|| invalid("a record offset is past the end of its file"))?;
        let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if len > MAX {
            return Err(invalid("a record is larger than the format allows"));
        }
        if 4 + len <= got {
            return Ok(first.get(4..4 + len).unwrap_or_default().to_vec());
        }
        self.read_vec(offset + 4, len)
    }
}

pub fn le64(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8).and_then(|s| <[u8; 8]>::try_from(s).ok()).map_or(0, u64::from_le_bytes)
}

pub fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()).map_or(0, u32::from_le_bytes)
}

/// A sorted table of fixed-width entries.
pub struct Fixed {
    blob: Blob,
    width: u64,
    pub count: u64,
}

impl Fixed {
    pub fn open(path: &Path, width: u64) -> io::Result<Fixed> {
        let blob = Blob::open(path)?;
        if width == 0 || blob.len % width != 0 {
            return Err(invalid("an index table is not a whole number of entries"));
        }
        let count = blob.len / width;
        Ok(Fixed { blob, width, count })
    }

    pub fn get(&self, index: u64, buf: &mut [u8]) -> io::Result<()> {
        self.blob.read_exact_at(index * self.width, buf)
    }

    pub fn entry(&self, index: u64) -> io::Result<Vec<u8>> {
        self.blob.read_vec(index * self.width, self.width as usize)
    }

    /// Entries `start..start + count` (cut at the end of the table), back to back.
    pub fn block(&self, start: u64, count: u64) -> io::Result<Vec<u8>> {
        let end = (start + count).min(self.count);
        if start >= end {
            return Ok(Vec::new());
        }
        self.blob.read_vec(start * self.width, ((end - start) * self.width) as usize)
    }

    /// The first index at or after `from` where `before` is false, `before` being true for a prefix of the table.
    pub fn partition_point(&self, from: u64, mut before: impl FnMut(&[u8]) -> io::Result<bool>) -> io::Result<u64> {
        let (mut lo, mut hi) = (from, self.count);
        let mut buf = vec![0u8; self.width as usize];
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            self.get(mid, &mut buf)?;
            if before(&buf)? { lo = mid + 1 } else { hi = mid }
        }
        Ok(lo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn binary_search_and_records() {
        let dir = std::env::temp_dir().join(format!("obs-gov-table-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.idx");
        let mut f = File::create(&path).unwrap();
        for k in [1u64, 3, 3, 7, 9] {
            f.write_all(&k.to_le_bytes()).unwrap();
        }
        drop(f);
        let t = Fixed::open(&path, 8).unwrap();
        assert_eq!(t.count, 5);
        assert_eq!(t.partition_point(0, |e| Ok(le64(e, 0) < 3)).unwrap(), 1);
        assert_eq!(t.partition_point(0, |e| Ok(le64(e, 0) < 4)).unwrap(), 3);
        assert_eq!(t.partition_point(0, |e| Ok(le64(e, 0) < 100)).unwrap(), 5);
        assert_eq!(t.block(3, 10).unwrap().len(), 16);
        let rec = dir.join("t.rec");
        let mut f = File::create(&rec).unwrap();
        f.write_all(&5u32.to_le_bytes()).unwrap();
        f.write_all(b"hello").unwrap();
        drop(f);
        let b = Blob::open(&rec).unwrap();
        assert_eq!(b.record(0).unwrap(), b"hello");
        assert!(b.record(100).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
