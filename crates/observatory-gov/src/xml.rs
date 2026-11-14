//! A pull tokenizer for the ABN Bulk Extract (500 MB of XML per file): start, end and text events over a `BufRead`,
//! one reused buffer, nothing held per record. It reads the XML the extract uses (elements, attributes, the five entities
//! and numeric references, comments, a declaration); a DOCTYPE with an internal subset is refused, and a tag or a text
//! node longer than a megabyte is refused, so hostile input costs bounded memory.

use crate::rd::{invalid, read_until};
use std::borrow::Cow;
use std::io::{self, BufRead};

const MAX_TAG: usize = 1 << 20;
const MAX_TEXT: usize = 8 << 20;

pub enum Ev<'a> {
    Start { name: &'a [u8], attrs: &'a [u8] },
    End { name: &'a [u8] },
    Text(&'a [u8]),
    Eof,
}

#[derive(Clone, Copy)]
enum Mode {
    Text,
    Tag,
    EmptyEnd,
}

#[derive(Clone, Copy)]
enum Kind {
    Start { name_end: usize, attrs_end: usize },
    End { from: usize, to: usize },
    Text,
    Eof,
}

pub struct Xml<R> {
    r: R,
    buf: Vec<u8>,
    mode: Mode,
    name_end: usize,
}

impl<R: BufRead> Xml<R> {
    pub fn new(r: R) -> Self {
        Xml { r, buf: Vec::new(), mode: Mode::Text, name_end: 0 }
    }

    pub fn event(&mut self) -> io::Result<Ev<'_>> {
        let kind = self.advance()?;
        Ok(match kind {
            Kind::Eof => Ev::Eof,
            Kind::Text => Ev::Text(&self.buf),
            Kind::End { from, to } => Ev::End { name: self.buf.get(from..to).unwrap_or_default() },
            Kind::Start { name_end, attrs_end } => Ev::Start {
                name: self.buf.get(..name_end).unwrap_or_default(),
                attrs: self.buf.get(name_end..attrs_end).unwrap_or_default(),
            },
        })
    }

    fn advance(&mut self) -> io::Result<Kind> {
        loop {
            match self.mode {
                Mode::EmptyEnd => {
                    self.mode = Mode::Text;
                    return Ok(Kind::End { from: 0, to: self.name_end });
                }
                Mode::Text => {
                    self.buf.clear();
                    if !read_until(&mut self.r, b'<', &mut self.buf, MAX_TEXT)? {
                        return Ok(Kind::Eof);
                    }
                    self.mode = Mode::Tag;
                    if !self.buf.iter().all(u8::is_ascii_whitespace) {
                        return Ok(Kind::Text);
                    }
                }
                Mode::Tag => {
                    self.buf.clear();
                    self.mode = Mode::Text;
                    if !read_until(&mut self.r, b'>', &mut self.buf, MAX_TAG)? {
                        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the file ends inside a tag"));
                    }
                    if let Some(kind) = self.classify()? {
                        return Ok(kind);
                    }
                }
            }
        }
    }

    /// The tag in `buf` (without `<` and `>`): `None` for what carries no event (declaration, comment, CDATA, DOCTYPE).
    fn classify(&mut self) -> io::Result<Option<Kind>> {
        match self.buf.first().copied() {
            Some(b'?') => Ok(None),
            Some(b'!') => {
                let (open, close): (&[u8], &[u8]) = if self.buf.starts_with(b"!--") {
                    (b"!--", b"--")
                } else if self.buf.starts_with(b"![CDATA[") {
                    (b"![CDATA[", b"]]")
                } else {
                    (b"", b"")
                };
                if open.is_empty() {
                    if self.buf.contains(&b'[') {
                        return Err(invalid("a DOCTYPE with an internal subset is not part of the extract"));
                    }
                    return Ok(None);
                }
                // A comment or CDATA section may hold `>`: read on until its own end.
                while !(self.buf.len() >= open.len() + close.len() && self.buf.ends_with(close)) {
                    self.buf.push(b'>');
                    if !read_until(&mut self.r, b'>', &mut self.buf, MAX_TAG)? {
                        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the file ends inside a comment"));
                    }
                }
                Ok(None)
            }
            Some(b'/') => {
                let mut to = self.buf.len();
                while to > 1 && self.buf.get(to - 1).is_some_and(u8::is_ascii_whitespace) {
                    to -= 1;
                }
                Ok(Some(Kind::End { from: 1, to }))
            }
            Some(_) => {
                let closing = self.buf.last() == Some(&b'/');
                let attrs_end = self.buf.len() - usize::from(closing);
                let name_end = self.buf.iter().position(|b| b.is_ascii_whitespace() || *b == b'/').unwrap_or(self.buf.len()).min(attrs_end);
                self.name_end = name_end;
                self.mode = if closing { Mode::EmptyEnd } else { Mode::Text };
                Ok(Some(Kind::Start { name_end, attrs_end }))
            }
            None => Err(invalid("an empty tag")),
        }
    }
}

/// The text of an element or attribute with its entities read: the five named ones and `&#N;` / `&#xH;`; anything else
/// is left as it is.
pub fn unescape(raw: &[u8]) -> Cow<'_, str> {
    if !raw.contains(&b'&') {
        return String::from_utf8_lossy(raw);
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while let Some(&b) = raw.get(i) {
        if b == b'&'
            && let Some(len) = raw.get(i + 1..(i + 12).min(raw.len())).and_then(|w| w.iter().position(|&c| c == b';'))
            && let Some(name) = raw.get(i + 1..i + 1 + len)
            && let Some(c) = entity(name)
        {
            let mut tmp = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
            i += len + 2;
            continue;
        }
        out.push(b);
        i += 1;
    }
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

fn entity(name: &[u8]) -> Option<char> {
    match name {
        b"amp" => Some('&'),
        b"lt" => Some('<'),
        b"gt" => Some('>'),
        b"quot" => Some('"'),
        b"apos" => Some('\''),
        [b'#', b'x' | b'X', hex @ ..] => u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok().and_then(char::from_u32),
        [b'#', dec @ ..] => std::str::from_utf8(dec).ok()?.parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

/// The value of attribute `key` in the attribute text of a start tag.
pub fn attr<'a>(attrs: &'a [u8], key: &[u8]) -> Option<Cow<'a, str>> {
    let mut i = 0;
    while i < attrs.len() {
        while attrs.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let name_start = i;
        while attrs.get(i).is_some_and(|b| *b != b'=' && !b.is_ascii_whitespace()) {
            i += 1;
        }
        let name = attrs.get(name_start..i)?;
        while attrs.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if attrs.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while attrs.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let quote = *attrs.get(i)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        i += 1;
        let value_start = i;
        while attrs.get(i).is_some_and(|b| *b != quote) {
            i += 1;
        }
        let value = attrs.get(value_start..i)?;
        i += 1;
        if name == key {
            return Some(unescape(value));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(xml: &str) -> Vec<String> {
        let mut x = Xml::new(io::Cursor::new(xml.as_bytes().to_vec()));
        let mut out = Vec::new();
        loop {
            match x.event() {
                Ok(Ev::Eof) => break,
                Ok(Ev::Start { name, attrs }) => {
                    out.push(format!("<{} {}>", String::from_utf8_lossy(name), String::from_utf8_lossy(attrs).trim()))
                }
                Ok(Ev::End { name }) => out.push(format!("</{}>", String::from_utf8_lossy(name))),
                Ok(Ev::Text(t)) => out.push(format!("'{}'", unescape(t))),
                Err(e) => {
                    out.push(format!("ERR {e}"));
                    break;
                }
            }
        }
        out
    }

    #[test]
    fn tokens_entities_and_empty_elements() {
        let e = events("<?xml version=\"1.0\"?>\n<a x=\"1\">\n <!-- c > d --><b/>t &amp; u &#65;&#x42; &bogus;</a>\n");
        assert_eq!(e, ["<a x=\"1\">", "<b >", "</b>", "'t & u AB &bogus;'", "</a>"]);
    }

    #[test]
    fn attributes() {
        let a = b" status=\"ACT\" ABNStatusFromDate='20120701' q=\"a&amp;b\"";
        assert_eq!(attr(a, b"status").as_deref(), Some("ACT"));
        assert_eq!(attr(a, b"ABNStatusFromDate").as_deref(), Some("20120701"));
        assert_eq!(attr(a, b"q").as_deref(), Some("a&b"));
        assert_eq!(attr(a, b"nope"), None);
    }

    #[test]
    fn hostile_input_is_an_error_not_a_panic() {
        assert!(events("<a").last().is_some_and(|l| l.starts_with("ERR")));
        assert!(events("<!DOCTYPE x [<!ENTITY y \"z\">]><x/>").last().is_some_and(|l| l.starts_with("ERR")));
        assert!(events("<a><!-- never closed").last().is_some_and(|l| l.starts_with("ERR")));
        let _ = events("<a b=></a b>< >");
    }
}
