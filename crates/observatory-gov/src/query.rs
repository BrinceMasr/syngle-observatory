//! The verbs that read the ABN extract and the ASIC datasets: `abn`, `company`, `business-names` and `name`.

use crate::codec::{AbrRecord, BusinessName, Company};
use crate::envelope::Answer;
use crate::error::{GovError, OrDamaged, Res};
use crate::json::{echo, key, opt, quote};
use crate::names::{self, Hit, Query};
use crate::norm::{abn_valid, iso, strip_spaces};
use crate::snap::Snap;
use crate::table::le64;

const NAME_SOURCES: &[&str] = &["abr", "asic", "asicBn"];

fn query_json(verb: &str, field: &str, value: &str) -> String {
    let mut q = String::from("{\"verb\":");
    quote(&mut q, verb);
    q.push(',');
    quote(&mut q, field);
    q.push(':');
    quote(&mut q, &echo(value));
    q.push('}');
    q
}

fn bad_damage() -> GovError {
    GovError::damaged("a record does not decode")
}

/// Every entity the extract holds under this ABN (one, in a clean extract).
fn find_abn(snap: &Snap, abn: u64) -> Res<Vec<AbrRecord>> {
    let idx = snap.fixed("abn.idx", 16)?;
    let mut at = idx.partition_point(0, |e| Ok(le64(e, 0) < abn)).damaged()?;
    let mut out = Vec::new();
    let rec = snap.blob("abn.rec")?;
    while at < idx.count {
        let e = idx.entry(at).damaged()?;
        if le64(&e, 0) != abn {
            break;
        }
        let payload = rec.record(le64(&e, 8)).damaged()?;
        out.push(AbrRecord::decode(abn, &payload, &snap.meta.dicts).ok_or_else(bad_damage)?);
        at += 1;
    }
    Ok(out)
}

fn status_word(active: bool) -> &'static str {
    if active { "Active" } else { "Cancelled" }
}

fn abr_json(r: &AbrRecord) -> String {
    let mut o = String::with_capacity(300);
    o.push('{');
    let mut f = true;
    key(&mut o, &mut f, "abn");
    quote(&mut o, &format!("{:011}", r.abn));
    key(&mut o, &mut f, "status");
    quote(&mut o, status_word(r.active));
    key(&mut o, &mut f, "statusFrom");
    opt(&mut o, iso(r.status_from).as_deref());
    key(&mut o, &mut f, "entityType");
    quote(&mut o, &r.entity_type);
    key(&mut o, &mut f, "name");
    quote(&mut o, &r.name);
    key(&mut o, &mut f, "state");
    quote(&mut o, &r.state);
    key(&mut o, &mut f, "postcode");
    quote(&mut o, &r.postcode);
    key(&mut o, &mut f, "acn");
    opt(&mut o, (r.acn != 0).then(|| format!("{:09}", r.acn)).as_deref());
    key(&mut o, &mut f, "gst");
    o.push_str(if r.gst { "true" } else { "false" });
    key(&mut o, &mut f, "names");
    o.push('[');
    for (i, (kind, text)) in r.names.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str("{\"type\":");
        quote(&mut o, kind);
        o.push_str(",\"text\":");
        quote(&mut o, text);
        o.push('}');
    }
    o.push_str("]}");
    o
}

/// `gov abn <ABN or ACN>`: the check digit of an ABN is validated; an ACN is 9 digits.
pub fn abn(snap: &Snap, args: &[String]) -> Res<Answer> {
    let digits = strip_spaces(&args.concat());
    let query = query_json("abn", "abn", &digits);
    let all_digits = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
    if !all_digits || !(digits.len() == 9 || abn_valid(&digits)) {
        return Err(GovError::new("invalid-abn", "not an ABN with a valid check digit, or a 9 digit ACN").with_query(query));
    }
    snap.need("abr", "gov abn")?;
    let number: u64 = digits.parse().map_err(|_| GovError::new("invalid-abn", "not a number").with_query(query.clone()))?;
    let mut records = Vec::new();
    if digits.len() == 11 {
        records = find_abn(snap, number)?;
    } else {
        let idx = snap.fixed("acn.idx", 16)?;
        let mut at = idx.partition_point(0, |e| Ok(le64(e, 0) < number)).damaged()?;
        while at < idx.count {
            let e = idx.entry(at).damaged()?;
            if le64(&e, 0) != number {
                break;
            }
            records.extend(find_abn(snap, le64(&e, 8))?);
            at += 1;
        }
    }
    let results: Vec<String> = records.iter().map(abr_json).collect();
    Ok(Answer { used: &["abr"], query, count: records.len(), results: results.join(",") })
}

fn company_json(c: &Company) -> String {
    let mut o = String::from("{");
    let mut f = true;
    key(&mut o, &mut f, "acn");
    quote(&mut o, &format!("{:09}", c.acn));
    key(&mut o, &mut f, "name");
    opt(&mut o, c.name.as_deref());
    key(&mut o, &mut f, "status");
    opt(&mut o, c.status.as_deref());
    key(&mut o, &mut f, "registered");
    opt(&mut o, iso(c.registered).as_deref());
    key(&mut o, &mut f, "deregistered");
    opt(&mut o, iso(c.deregistered).as_deref());
    key(&mut o, &mut f, "abn");
    opt(&mut o, (c.abn != 0).then(|| format!("{:011}", c.abn)).as_deref());
    key(&mut o, &mut f, "formerNames");
    o.push('[');
    for (i, n) in c.former.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        quote(&mut o, n);
    }
    o.push_str("]}");
    o
}

/// `gov company <ACN>`.
pub fn company(snap: &Snap, args: &[String]) -> Res<Answer> {
    let digits = strip_spaces(&args.concat());
    let query = query_json("company", "acn", &digits);
    if digits.len() != 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(GovError::new("invalid-acn", "an ACN is 9 digits").with_query(query));
    }
    snap.need("asic", "gov company")?;
    let acn: u64 = digits.parse().map_err(|_| GovError::new("invalid-acn", "not a number").with_query(query.clone()))?;
    let idx = snap.fixed("co.idx", 16)?;
    let at = idx.partition_point(0, |e| Ok(le64(e, 0) < acn)).damaged()?;
    let mut results = String::new();
    let mut count = 0;
    if at < idx.count {
        let e = idx.entry(at).damaged()?;
        if le64(&e, 0) == acn {
            let c = Company::decode(&snap.blob("co.rec")?.record(le64(&e, 8)).damaged()?).ok_or_else(bad_damage)?;
            results = company_json(&c);
            count = 1;
        }
    }
    Ok(Answer { used: &["asic"], query, count, results })
}

/// `gov business-names <ABN>`: the ASIC business names of an ABN, by name.
pub fn business_names(snap: &Snap, args: &[String]) -> Res<Answer> {
    let digits = strip_spaces(&args.concat());
    let query = query_json("business-names", "abn", &digits);
    if digits.len() != 11 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(GovError::new("invalid-abn", "an ABN is 11 digits").with_query(query));
    }
    snap.need("asicBn", "gov business-names")?;
    let abn: u64 = digits.parse().map_err(|_| GovError::new("invalid-abn", "not a number").with_query(query.clone()))?;
    let idx = snap.fixed("bn.idx", 16)?;
    let rec = snap.blob("bn.rec")?;
    let mut at = idx.partition_point(0, |e| Ok(le64(e, 0) < abn)).damaged()?;
    let mut found: Vec<BusinessName> = Vec::new();
    while at < idx.count {
        let e = idx.entry(at).damaged()?;
        if le64(&e, 0) != abn {
            break;
        }
        found.push(BusinessName::decode(&rec.record(le64(&e, 8)).damaged()?).ok_or_else(bad_damage)?);
        at += 1;
    }
    found.sort_by(|a, b| (&a.name, a.registered, a.cancelled, &a.status).cmp(&(&b.name, b.registered, b.cancelled, &b.status)));
    let mut results = String::new();
    for (i, b) in found.iter().enumerate() {
        if i > 0 {
            results.push(',');
        }
        results.push('{');
        let mut f = true;
        key(&mut results, &mut f, "name");
        quote(&mut results, &b.name);
        key(&mut results, &mut f, "status");
        quote(&mut results, &b.status);
        key(&mut results, &mut f, "registered");
        opt(&mut results, iso(b.registered).as_deref());
        key(&mut results, &mut f, "cancelled");
        opt(&mut results, iso(b.cancelled).as_deref());
        key(&mut results, &mut f, "abn");
        quote(&mut results, &format!("{:011}", b.abn));
        results.push('}');
    }
    Ok(Answer { used: &["asicBn"], query, count: found.len(), results })
}

/// One search hit as a result: the entity (from the ABN extract when it holds the ABN, else from the record that matched),
/// and the name that matched.
fn hit_json(snap: &Snap, hit: &Hit) -> Res<String> {
    let (text, kind) = names::matched(snap, hit)?;
    let abr = if hit.in_abr { find_abn(snap, hit.abn)?.into_iter().next() } else { None };
    let (name, state, postcode, acn, active) = match abr {
        Some(r) => (r.name, r.state, r.postcode, (r.acn != 0).then(|| format!("{:09}", r.acn)), r.active),
        None => match hit.src {
            crate::names_build::SRC_ASIC => {
                let c = Company::decode(&snap.blob("co.rec")?.record(hit.rec).damaged()?).ok_or_else(bad_damage)?;
                (c.name.unwrap_or_default(), String::new(), String::new(), Some(format!("{:09}", c.acn)), hit.active)
            }
            _ => {
                let b = BusinessName::decode(&snap.blob("bn.rec")?.record(hit.rec).damaged()?).ok_or_else(bad_damage)?;
                (b.name, String::new(), String::new(), None, hit.active)
            }
        },
    };
    let mut o = String::with_capacity(260);
    o.push('{');
    let mut f = true;
    key(&mut o, &mut f, "kind");
    quote(&mut o, if hit.abn != 0 { "abn" } else { "business-name" });
    key(&mut o, &mut f, "abn");
    opt(&mut o, (hit.abn != 0).then(|| format!("{:011}", hit.abn)).as_deref());
    key(&mut o, &mut f, "name");
    quote(&mut o, &name);
    key(&mut o, &mut f, "matched");
    quote(&mut o, &text);
    key(&mut o, &mut f, "matchedType");
    quote(&mut o, &kind);
    key(&mut o, &mut f, "status");
    quote(&mut o, status_word(active));
    key(&mut o, &mut f, "state");
    quote(&mut o, &state);
    key(&mut o, &mut f, "postcode");
    quote(&mut o, &postcode);
    key(&mut o, &mut f, "acn");
    opt(&mut o, acn.as_deref());
    o.push('}');
    Ok(o)
}

/// `gov name <text> [--state S] [--active] [--limit N]`.
pub fn name(snap: &Snap, text: &str, state: Option<&str>, active: bool, limit: usize) -> Res<Answer> {
    let mut query = String::from("{\"verb\":\"name\",\"name\":");
    quote(&mut query, &echo(text));
    if let Some(s) = state {
        query.push_str(",\"state\":");
        quote(&mut query, &echo(s));
    }
    if active {
        query.push_str(",\"active\":true");
    }
    query.push_str(&format!(",\"limit\":{limit}}}"));
    if crate::norm::normalise(text).is_empty() {
        return Err(GovError::new("empty-query", "give a name to search for").with_query(query));
    }
    if !NAME_SOURCES.iter().any(|s| snap.has(s)) {
        return Err(GovError::new("no-data", "no name source is indexed: run syngle-map gov refresh --source abr,asic,asicBn"));
    }
    let hits = names::search(snap, &Query { text, state, active, limit })?;
    let mut results = String::new();
    for (i, h) in hits.iter().enumerate() {
        if i > 0 {
            results.push(',');
        }
        results.push_str(&hit_json(snap, h)?);
    }
    Ok(Answer { used: NAME_SOURCES, query, count: hits.len(), results })
}
