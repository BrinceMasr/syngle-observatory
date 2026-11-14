//! `gov abn <ABN or ACN> --live`: one ABN Lookup web services call (the JSON services of abr.business.gov.au), for the entity as
//! the register holds it today. The answer is marked `live:true` and `asOfKind:"fetched"`, comes from no snapshot and is never
//! stored. The GUID ABR e-mails after registration is read from `SYNGLE_ABN_LOOKUP_GUID` only; it travels in the address of the
//! request (the service takes it nowhere else) and so it is never put in a message, an answer, a file or a log: errors name the
//! host, and whatever the server says is scrubbed of the GUID before it is shown.
//!
//! The web services agreement (https://abr.business.gov.au/Tools/WebServicesAgreement, version 9.9.7, read in full on
//! 14 November 2026) binds whoever registers for the GUID, and its licence is non-transferable: each person registers their
//! own, and no GUID is shipped with the program. Terms this code keeps: nothing implies the Commonwealth endorses or is
//! affiliated with the product (the attribution says so); information ABR withdraws and tells you of must be deleted, and a live
//! answer is never stored, so the program holds nothing to delete; the services are "as is", may be suspended and are not
//! warranted correct or current (an answer says it was fetched, and when). The agreement sets no rate limit and has no clause on
//! caching: one call is made per `--live` query and none is repeated.

use crate::dates::today;
use crate::error::{GovError, Res};
use crate::json::{echo, key, opt, quote};
use crate::net::Net;
use crate::norm::{abn_valid, strip_spaces};
use serde_json::Value;
use std::fmt;
use std::time::Duration;

const SOURCE: &str = "abnLookup";

/// A registered GUID: 8-4-4-4-12 hex digits. Printing it shows nothing.
pub struct Guid(String);

impl Guid {
    pub fn parse(text: &str) -> Option<Guid> {
        let t = text.trim();
        let shape = t.len() == 36
            && t.bytes().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() });
        shape.then(|| Guid(t.to_string()))
    }

    /// Removes the GUID from a text the server wrote, in any letter case.
    fn scrub(&self, text: &str) -> String {
        let lower = text.to_ascii_lowercase();
        let mut out = String::new();
        let (mut from, secret) = (0, self.0.to_ascii_lowercase());
        while let Some(at) = lower.get(from..).and_then(|rest| rest.find(&secret)) {
            out.push_str(text.get(from..from + at).unwrap_or_default());
            out.push_str("<guid>");
            from += at + secret.len();
        }
        out.push_str(text.get(from..).unwrap_or_default());
        out
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Guid(<hidden>)")
    }
}

/// `{...}` out of a JSONP answer (`callback({...})`), or the text itself when it is bare JSON.
fn unwrap_jsonp(text: &str) -> Option<&str> {
    let t = text.trim().trim_end_matches(';').trim();
    match (t.find('('), t.rfind(')')) {
        (Some(a), Some(b)) if a < b && !t.starts_with('{') => t.get(a + 1..b),
        _ => Some(t),
    }
}

fn query_json(digits: &str) -> String {
    let mut q = String::from("{\"verb\":\"abn\",\"abn\":");
    quote(&mut q, &echo(digits));
    q.push_str(",\"live\":true}");
    q
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default().trim()
}

/// The day of a date the service writes (`2000-07-01` or `2000-07-01T00:00:00`), or nothing.
fn day(s: &str) -> Option<&str> {
    let d = s.get(..10)?;
    let ok = d.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
    ok.then_some(d)
}

/// The entity in the shape `gov abn` answers with.
fn entity(v: &Value) -> String {
    let mut o = String::from("{");
    let mut f = true;
    key(&mut o, &mut f, "abn");
    quote(&mut o, &text(v, "Abn").chars().filter(char::is_ascii_digit).collect::<String>());
    key(&mut o, &mut f, "status");
    quote(&mut o, text(v, "AbnStatus"));
    key(&mut o, &mut f, "statusFrom");
    opt(&mut o, day(text(v, "AbnStatusEffectiveFrom")));
    key(&mut o, &mut f, "entityType");
    quote(&mut o, text(v, "EntityTypeName"));
    key(&mut o, &mut f, "name");
    quote(&mut o, text(v, "EntityName"));
    key(&mut o, &mut f, "state");
    quote(&mut o, text(v, "AddressState"));
    key(&mut o, &mut f, "postcode");
    quote(&mut o, text(v, "AddressPostcode"));
    key(&mut o, &mut f, "acn");
    let acn = text(v, "Acn");
    opt(&mut o, (!acn.is_empty()).then_some(acn));
    key(&mut o, &mut f, "gst");
    o.push_str(if text(v, "Gst").is_empty() { "false" } else { "true" });
    key(&mut o, &mut f, "names");
    o.push('[');
    let names = v.get("BusinessName").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    for (i, n) in names.iter().filter_map(Value::as_str).filter(|n| !n.trim().is_empty()).enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str("{\"type\":\"Business name\",\"text\":");
        quote(&mut o, n.trim());
        o.push('}');
    }
    o.push_str("]}");
    o
}

fn answer(query: &str, results: &str, count: usize) -> String {
    let today = today();
    let mut o = String::from("{\"ok\":true,\"live\":true,\"dataVersion\":\"live\",\"sources\":{\"");
    o.push_str(SOURCE);
    o.push_str("\":{\"dataVersion\":\"live\",\"asOf\":");
    quote(&mut o, &today);
    o.push_str(",\"asOfKind\":\"fetched\",\"licence\":\"ABN Lookup web services agreement\",\"attribution\":\"Australian Business Register, ABN Lookup web services (fetched live; the Commonwealth does not endorse this product)\"}},\"query\":");
    o.push_str(query);
    o.push_str(&format!(",\"count\":{count},\"results\":[{results}]}}"));
    o
}

/// The one live call. `raw_guid` is `SYNGLE_ABN_LOOKUP_GUID` as the environment holds it.
pub fn lookup(net: &Net, raw_guid: Option<&str>, args: &[String]) -> Res<String> {
    let digits = strip_spaces(&args.concat());
    let query = query_json(&digits);
    let all_digits = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
    if !all_digits || !(digits.len() == 9 || abn_valid(&digits)) {
        return Err(GovError::new("invalid-abn", "not an ABN with a valid check digit, or a 9 digit ACN").with_query(query));
    }
    let Some(raw) = raw_guid.filter(|g| !g.trim().is_empty()) else {
        return Err(GovError::new("no-data", "gov abn --live needs your ABN Lookup GUID in the environment variable SYNGLE_ABN_LOOKUP_GUID (free: https://abr.business.gov.au/Documentation/WebServiceRegistration)").with_query(query));
    };
    let Some(guid) = Guid::parse(raw) else {
        return Err(GovError::new("bad-source", "SYNGLE_ABN_LOOKUP_GUID is not a GUID (8-4-4-4-12 hex digits, as ABR e-mailed it)")
            .with_query(query));
    };
    let (page, param) = if digits.len() == 11 { ("AbnDetails.aspx", "abn") } else { ("AcnDetails.aspx", "acn") };
    let url = format!("{}{page}?{param}={digits}&callback=c&guid={}", net.abn_json, guid.0);
    let fail = |e: GovError| GovError::refresh(guid.scrub(&e.message)).with_query(query.clone());
    let body = net.bytes(&url, &[("Accept", "application/json".to_string())], Duration::from_secs(20), 1 << 20).map_err(fail)?;
    let body = String::from_utf8_lossy(&body);
    let value: Value = unwrap_jsonp(&body)
        .and_then(|j| serde_json::from_str(j).ok())
        .ok_or_else(|| GovError::refresh("ABN Lookup did not answer with JSON").with_query(query.clone()))?;
    let message = guid.scrub(text(&value, "Message"));
    if !message.is_empty() {
        let lower = message.replace("<guid>", "").to_ascii_lowercase();
        if lower.contains("no records found") {
            return Ok(answer(&query, "", 0));
        }
        let why = if lower.contains("guid") {
            "ABN Lookup does not recognise the GUID in SYNGLE_ABN_LOOKUP_GUID (use the one ABR e-mailed after registration)".to_string()
        } else {
            format!("ABN Lookup said: {message}")
        };
        return Err(GovError::refresh(why).with_query(query));
    }
    if text(&value, "Abn").is_empty() {
        return Ok(answer(&query, "", 0));
    }
    Ok(answer(&query, &entity(&value), 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: &str = "0a1b2c3d-4e5f-6789-abcd-ef0123456789";

    #[test]
    fn a_guid_has_its_shape_and_never_prints() {
        assert!(Guid::parse(G).is_some() && Guid::parse(&format!(" {G}\n")).is_some());
        for bad in [
            "",
            "x",
            "0a1b2c3d-4e5f-6789-abcd-ef012345678",
            "0a1b2c3d4e5f-6789-abcd-ef0123456789-",
            "zz1b2c3d-4e5f-6789-abcd-ef0123456789",
            "0a1b2c3d-4e5f-6789-abcd-ef0123456789&x=1",
        ] {
            assert!(Guid::parse(bad).is_none(), "{bad}");
        }
        let g = Guid::parse(G).unwrap();
        assert!(!format!("{g:?}").contains("0a1b"));
        assert_eq!(g.scrub(&format!("bad {G} and {} end", G.to_ascii_uppercase())), "bad <guid> and <guid> end");
        assert_eq!(g.scrub("nothing"), "nothing");
    }

    #[test]
    fn jsonp_and_json() {
        assert_eq!(unwrap_jsonp("c({\"a\":1});"), Some("{\"a\":1}"));
        assert_eq!(unwrap_jsonp("{\"a\":(1)}"), Some("{\"a\":(1)}"));
        assert_eq!(day("2000-07-01T00:00:00"), Some("2000-07-01"));
        assert_eq!(day("1 July 2000"), None);
    }

    #[test]
    fn an_entity_in_the_shape_gov_abn_answers_with() {
        let v: Value = serde_json::from_str(
            r#"{"Abn":"74172177893","AbnStatus":"Active","AbnStatusEffectiveFrom":"2000-07-01","Acn":"172177893","AddressDate":null,"AddressPostcode":"2600","AddressState":"ACT","BusinessName":["Kangaroo Co"," "],"EntityName":"EXAMPLE PTY LTD","EntityTypeCode":"PRV","EntityTypeName":"Australian Private Company","Gst":"2000-07-01","Message":""}"#,
        )
        .unwrap();
        assert_eq!(
            entity(&v),
            r#"{"abn":"74172177893","status":"Active","statusFrom":"2000-07-01","entityType":"Australian Private Company","name":"EXAMPLE PTY LTD","state":"ACT","postcode":"2600","acn":"172177893","gst":true,"names":[{"type":"Business name","text":"Kangaroo Co"}]}"#
        );
    }
}
