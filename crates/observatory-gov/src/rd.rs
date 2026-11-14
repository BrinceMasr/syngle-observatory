//! Bounded reading: a hostile file with no delimiter must not be read into memory whole.

use std::io::{self, BufRead};

pub fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Appends the bytes up to (not including) `delim` to `out` and consumes the delimiter. `Ok(false)` at the end of the
/// input without a delimiter (what was read is in `out`); an error when `out` outgrows `limit`.
pub fn read_until(r: &mut impl BufRead, delim: u8, out: &mut Vec<u8>, limit: usize) -> io::Result<bool> {
    loop {
        let (found, used) = {
            let chunk = r.fill_buf()?;
            if chunk.is_empty() {
                return Ok(false);
            }
            match memchr::memchr(delim, chunk) {
                Some(i) => {
                    out.extend_from_slice(chunk.get(..i).unwrap_or_default());
                    (true, i + 1)
                }
                None => {
                    out.extend_from_slice(chunk);
                    (false, chunk.len())
                }
            }
        };
        r.consume(used);
        if out.len() > limit {
            return Err(invalid("a line or element is longer than the format allows"));
        }
        if found {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_to_the_delimiter_and_bounds_the_rest() {
        let mut r = io::Cursor::new(b"abc<def".to_vec());
        let mut out = Vec::new();
        assert!(read_until(&mut r, b'<', &mut out, 10).unwrap());
        assert_eq!(out, b"abc");
        out.clear();
        assert!(!read_until(&mut r, b'<', &mut out, 10).unwrap());
        assert_eq!(out, b"def");
        let mut long = io::Cursor::new(vec![b'x'; 100]);
        assert!(read_until(&mut long, b'<', &mut Vec::new(), 10).is_err());
    }
}
