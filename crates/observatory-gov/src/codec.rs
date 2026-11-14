//! The records of the index, as bytes. Little-endian u32 length, then the payload: a flag byte, varints, strings with a
//! varint length. A reader never trusts a length: every read is bounds-checked and a record that does not decode is
//! reported as damage, never a panic.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

pub fn put_str(out: &mut Vec<u8>, s: &str) {
    put_varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

pub struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Cur { b, p: 0 }
    }

    pub fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.p)?;
        self.p += 1;
        Some(v)
    }

    pub fn varint(&mut self) -> Option<u64> {
        let (mut v, mut shift) = (0u64, 0u32);
        loop {
            let b = self.u8()?;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }

    pub fn str(&mut self) -> Option<String> {
        let n = usize::try_from(self.varint()?).ok()?;
        let end = self.p.checked_add(n)?;
        let s = self.b.get(self.p..end)?;
        self.p = end;
        Some(String::from_utf8_lossy(s).into_owned())
    }
}

/// The short lists that repeat in every record (entity types, kinds of name) are stored once, in the snapshot's
/// metadata, and a record holds an index into them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dicts {
    #[serde(default)]
    pub entity: Vec<String>,
    #[serde(default)]
    pub name_type: Vec<String>,
}

impl Dicts {
    fn get(list: &[String], i: u8) -> &str {
        list.get(usize::from(i)).map_or("", String::as_str)
    }
}

#[derive(Default)]
pub struct Interner {
    map: HashMap<String, u8>,
    pub items: Vec<String>,
}

impl Interner {
    /// The index of `s`; 255 (read back as empty) when the list is full.
    pub fn intern(&mut self, s: &str) -> u8 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        if self.items.len() >= 255 {
            return 255;
        }
        let i = self.items.len() as u8;
        self.items.push(s.to_string());
        self.map.insert(s.to_string(), i);
        i
    }
}

/// An ABN Bulk Extract entity (the key, the ABN, is held by the index and not repeated in the record).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AbrRecord {
    pub abn: u64,
    pub active: bool,
    pub gst: bool,
    pub status_from: u32,
    pub acn: u32,
    pub entity_type: String,
    pub name: String,
    pub state: String,
    pub postcode: String,
    pub names: Vec<(String, String)>,
}

impl AbrRecord {
    pub fn encode(&self, out: &mut Vec<u8>, entity: &mut Interner, name_type: &mut Interner) {
        let flags =
            u8::from(self.active) | (u8::from(self.gst) << 1) | (u8::from(self.acn != 0) << 2) | (u8::from(self.status_from != 0) << 3);
        out.push(flags);
        if self.status_from != 0 {
            put_varint(out, u64::from(self.status_from));
        }
        if self.acn != 0 {
            put_varint(out, u64::from(self.acn));
        }
        out.push(entity.intern(&self.entity_type));
        put_str(out, &self.name);
        put_str(out, &self.state);
        put_str(out, &self.postcode);
        let shown = self.names.len().min(254);
        put_varint(out, shown as u64);
        for (kind, text) in self.names.iter().take(shown) {
            out.push(name_type.intern(kind));
            put_str(out, text);
        }
    }

    pub fn decode(abn: u64, payload: &[u8], dicts: &Dicts) -> Option<AbrRecord> {
        let mut c = Cur::new(payload);
        let flags = c.u8()?;
        let status_from = if flags & 8 != 0 { u32::try_from(c.varint()?).ok()? } else { 0 };
        let acn = if flags & 4 != 0 { u32::try_from(c.varint()?).ok()? } else { 0 };
        let entity_type = Dicts::get(&dicts.entity, c.u8()?).to_string();
        let name = c.str()?;
        let state = c.str()?;
        let postcode = c.str()?;
        let n = usize::try_from(c.varint()?).ok()?;
        let mut names = Vec::with_capacity(n.min(255));
        for _ in 0..n {
            let kind = Dicts::get(&dicts.name_type, c.u8()?).to_string();
            names.push((kind, c.str()?));
        }
        Some(AbrRecord { abn, active: flags & 1 != 0, gst: flags & 2 != 0, status_from, acn, entity_type, name, state, postcode, names })
    }
}

/// One ASIC company, current and former names together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Company {
    pub acn: u32,
    pub abn: u64,
    pub name: Option<String>,
    pub status: Option<String>,
    pub registered: u32,
    pub deregistered: u32,
    pub former: Vec<String>,
}

impl Company {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(self.name.is_some()) | (u8::from(self.status.is_some()) << 1));
        put_varint(out, u64::from(self.acn));
        put_varint(out, self.abn);
        put_varint(out, u64::from(self.registered));
        put_varint(out, u64::from(self.deregistered));
        put_str(out, self.name.as_deref().unwrap_or(""));
        put_str(out, self.status.as_deref().unwrap_or(""));
        let shown = self.former.len().min(254);
        put_varint(out, shown as u64);
        for f in self.former.iter().take(shown) {
            put_str(out, f);
        }
    }

    pub fn decode(payload: &[u8]) -> Option<Company> {
        let mut c = Cur::new(payload);
        let flags = c.u8()?;
        let acn = u32::try_from(c.varint()?).ok()?;
        let abn = c.varint()?;
        let registered = u32::try_from(c.varint()?).ok()?;
        let deregistered = u32::try_from(c.varint()?).ok()?;
        let name = c.str()?;
        let status = c.str()?;
        let n = usize::try_from(c.varint()?).ok()?;
        let mut former = Vec::with_capacity(n.min(255));
        for _ in 0..n {
            former.push(c.str()?);
        }
        Some(Company {
            acn,
            abn,
            name: (flags & 1 != 0).then_some(name),
            status: (flags & 2 != 0).then_some(status),
            registered,
            deregistered,
            former,
        })
    }
}

/// One ASIC business name registration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BusinessName {
    pub name: String,
    pub status: String,
    pub registered: u32,
    pub cancelled: u32,
    pub abn: u64,
}

impl BusinessName {
    pub fn encode(&self, out: &mut Vec<u8>) {
        put_str(out, &self.name);
        put_str(out, &self.status);
        put_varint(out, u64::from(self.registered));
        put_varint(out, u64::from(self.cancelled));
        put_varint(out, self.abn);
    }

    pub fn decode(payload: &[u8]) -> Option<BusinessName> {
        let mut c = Cur::new(payload);
        let name = c.str()?;
        let status = c.str()?;
        let registered = u32::try_from(c.varint()?).ok()?;
        let cancelled = u32::try_from(c.varint()?).ok()?;
        Some(BusinessName { name, status, registered, cancelled, abn: c.varint()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_garbage_does_not_decode() {
        let (mut et, mut nt) = (Interner::default(), Interner::default());
        let r = AbrRecord {
            abn: 14100200301,
            active: true,
            gst: true,
            status_from: 20120701,
            acn: 100200306,
            entity_type: "Australian Private Company".into(),
            name: "HARBOUR ROOFING PTY LTD".into(),
            state: "NSW".into(),
            postcode: "2000".into(),
            names: vec![("TRD".into(), "HARBOUR ROOFING".into())],
        };
        let mut b = Vec::new();
        r.encode(&mut b, &mut et, &mut nt);
        let dicts = Dicts { entity: et.items, name_type: nt.items };
        assert_eq!(AbrRecord::decode(r.abn, &b, &dicts), Some(r));
        assert_eq!(AbrRecord::decode(1, &b[..b.len() - 3], &dicts), None);
        assert_eq!(AbrRecord::decode(1, &[0xff; 20], &dicts), None);
        let c = Company {
            acn: 1,
            abn: 2,
            name: Some("A".into()),
            status: Some("Registered".into()),
            registered: 19900108,
            deregistered: 0,
            former: vec!["B".into()],
        };
        let mut b = Vec::new();
        c.encode(&mut b);
        assert_eq!(Company::decode(&b), Some(c));
        let n = BusinessName { name: "X".into(), status: "Registered".into(), registered: 1, cancelled: 0, abn: 5 };
        let mut b = Vec::new();
        n.encode(&mut b);
        assert_eq!(BusinessName::decode(&b), Some(n));
    }
}
