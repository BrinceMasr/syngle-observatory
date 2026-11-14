//! The envelope of every answer, in the contract's key order. A success carries the data version and, for each source the
//! answer used, that source's version, as-of date, licence and attribution; a refusal carries a fixed code.

use crate::error::GovError;
use crate::json::{key, quote};
use crate::snap::{Meta, SourceMeta};
use crate::sources::SOURCES;

/// What a verb produced: the sources it used, the query as JSON, and the results as the inside of a JSON array.
pub struct Answer {
    pub used: &'static [&'static str],
    pub query: String,
    pub count: usize,
    pub results: String,
}

fn source(out: &mut String, s: &SourceMeta) {
    out.push('{');
    let mut first = true;
    key(out, &mut first, "dataVersion");
    quote(out, &s.version);
    key(out, &mut first, "asOf");
    quote(out, &s.as_of);
    if let Some(kind) = &s.as_of_kind {
        key(out, &mut first, "asOfKind");
        quote(out, kind);
    }
    key(out, &mut first, "licence");
    quote(out, &s.licence);
    key(out, &mut first, "attribution");
    quote(out, &s.attribution);
    out.push('}');
}

pub fn success(meta: &Meta, answer: &Answer) -> String {
    let mut out = String::with_capacity(512 + answer.results.len());
    out.push_str("{\"ok\":true,\"dataVersion\":");
    quote(&mut out, &meta.id);
    out.push_str(",\"sources\":{");
    let mut first = true;
    for info in &SOURCES {
        if answer.used.contains(&info.name)
            && let Some(s) = meta.sources.get(info.name)
        {
            key(&mut out, &mut first, info.name);
            source(&mut out, s);
        }
    }
    out.push_str("},\"query\":");
    out.push_str(&answer.query);
    out.push_str(",\"count\":");
    out.push_str(&answer.count.to_string());
    out.push_str(",\"results\":[");
    out.push_str(&answer.results);
    out.push_str("]}");
    out
}

pub fn refusal(e: &GovError) -> String {
    let mut out = String::from("{\"ok\":false,\"error\":{\"code\":");
    quote(&mut out, e.code);
    out.push_str(",\"message\":");
    let message: String = e.message.chars().take(300).collect();
    quote(&mut out, &message);
    out.push('}');
    if let Some(q) = &e.query {
        out.push_str(",\"query\":");
        out.push_str(q);
    }
    out.push('}');
    out
}

/// `gov status`: the snapshot, and each source's version, as-of date, licence and publisher.
pub fn status(meta: &Meta, retained: &[String]) -> String {
    let mut out = String::from("{\"ok\":true,\"dataVersion\":");
    quote(&mut out, &meta.id);
    out.push_str(",\"sources\":{");
    let mut first = true;
    for info in &SOURCES {
        if let Some(s) = meta.sources.get(info.name) {
            key(&mut out, &mut first, info.name);
            out.push('{');
            let mut f = true;
            key(&mut out, &mut f, "dataVersion");
            quote(&mut out, &s.version);
            key(&mut out, &mut f, "asOf");
            quote(&mut out, &s.as_of);
            if let Some(kind) = &s.as_of_kind {
                key(&mut out, &mut f, "asOfKind");
                quote(&mut out, kind);
            }
            key(&mut out, &mut f, "licence");
            quote(&mut out, &s.licence);
            key(&mut out, &mut f, "publisher");
            quote(&mut out, &s.publisher);
            key(&mut out, &mut f, "records");
            out.push_str(&s.records.to_string());
            out.push('}');
        }
    }
    out.push_str("},\"retained\":[");
    for (i, id) in retained.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        quote(&mut out, id);
    }
    out.push_str("]}");
    out
}
