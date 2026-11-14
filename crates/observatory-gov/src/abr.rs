//! The ABN Bulk Extract (`bulkextract.xsd`): root `Transfer`, a `TransferInfo` header (the record count and the extract's
//! own time), then one `ABR` element per entity. Read as a stream; a record is built, handed to the caller and dropped.

use crate::codec::AbrRecord;
use crate::norm::{digits_to_u64, from_ymd8};
use crate::rd::invalid;
use crate::xml::{Ev, Xml, attr, unescape};
use std::io::{self, BufRead};

const MAX_DEPTH: usize = 64;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub declared: Option<u64>,
    pub records: u64,
    pub extract_time: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum El {
    Transfer,
    TransferInfo,
    RecordCount,
    ExtractTime,
    Abr,
    Abn,
    EntityTypeText,
    MainEntity,
    OtherEntity,
    IndividualName,
    NameText,
    NameType,
    GivenName,
    FamilyName,
    State,
    Postcode,
    AsicNumber,
    Other,
}

fn classify(name: &[u8]) -> El {
    match name {
        b"Transfer" => El::Transfer,
        b"TransferInfo" => El::TransferInfo,
        b"RecordCount" => El::RecordCount,
        b"ExtractTime" => El::ExtractTime,
        b"ABR" => El::Abr,
        b"ABN" => El::Abn,
        b"EntityTypeText" => El::EntityTypeText,
        b"MainEntity" => El::MainEntity,
        b"OtherEntity" => El::OtherEntity,
        b"IndividualName" => El::IndividualName,
        b"NonIndividualNameText" => El::NameText,
        b"NonIndividualName" => El::NameType,
        b"GivenName" => El::GivenName,
        b"FamilyName" => El::FamilyName,
        b"State" => El::State,
        b"Postcode" => El::Postcode,
        b"ASICNumber" => El::AsicNumber,
        _ => El::Other,
    }
}

#[derive(Default)]
struct Draft {
    rec: AbrRecord,
    has_abn: bool,
    main_name: Option<String>,
    given: Vec<String>,
    family: Option<String>,
    individual: bool,
    kind: String,
    type_seen: bool,
    state_seen: bool,
    postcode_seen: bool,
}

impl Draft {
    fn finish(mut self) -> Option<AbrRecord> {
        if !self.has_abn {
            return None;
        }
        self.rec.name = if self.individual {
            format!("{}, {}", self.family.unwrap_or_default(), self.given.join(" ")).trim().to_string()
        } else {
            self.main_name.unwrap_or_default()
        };
        Some(self.rec)
    }
}

/// Reads one extract. `sink` gets every entity that has an ABN. Fails when the file is cut short (no closing
/// `Transfer`), is not XML, or holds a number of `ABR` elements other than its header declares.
pub fn parse<R: BufRead>(r: R, mut sink: impl FnMut(AbrRecord) -> io::Result<()>) -> io::Result<Header> {
    let mut xml = Xml::new(r);
    let mut stack: Vec<El> = Vec::new();
    let mut header = Header::default();
    let mut draft: Option<Draft> = None;
    let mut closed = false;
    let mut saw_root = false;
    loop {
        match xml.event()? {
            Ev::Eof => break,
            Ev::Start { name, attrs } => {
                let el = classify(name);
                if stack.len() >= MAX_DEPTH {
                    return Err(invalid("the XML is nested deeper than the extract is"));
                }
                if stack.is_empty() {
                    if el != El::Transfer {
                        return Err(invalid("this is not an ABN Bulk Extract (the root element is not Transfer)"));
                    }
                    saw_root = true;
                }
                match el {
                    El::Abr => draft = Some(Draft::default()),
                    El::Abn => {
                        if let Some(d) = draft.as_mut() {
                            d.rec.active = attr(attrs, b"status").is_some_and(|s| s == "ACT");
                            d.rec.status_from = attr(attrs, b"ABNStatusFromDate").map_or(0, |s| from_ymd8(&s));
                        }
                    }
                    El::NameType => {
                        if let Some(d) = draft.as_mut() {
                            d.kind = attr(attrs, b"type").map(|s| s.into_owned()).unwrap_or_default();
                        }
                    }
                    El::IndividualName => {
                        if let Some(d) = draft.as_mut() {
                            d.individual = true;
                        }
                    }
                    El::Other if name == b"GST" => {
                        if let Some(d) = draft.as_mut() {
                            d.rec.gst = attr(attrs, b"status").is_some_and(|s| s == "ACT");
                        }
                    }
                    _ => {}
                }
                stack.push(el);
            }
            Ev::End { name } => {
                let popped = stack.pop();
                if popped.is_some_and(|p| p != classify(name)) {
                    return Err(invalid("an end tag does not match its start tag"));
                }
                match popped {
                    Some(El::Abr) => {
                        header.records += 1;
                        if let Some(rec) = draft.take().and_then(Draft::finish) {
                            sink(rec)?;
                        }
                    }
                    Some(El::Transfer) => closed = true,
                    _ => {}
                }
            }
            Ev::Text(raw) => {
                let text = unescape(raw);
                match stack.last() {
                    Some(El::RecordCount) => header.declared = text.trim().parse().ok(),
                    Some(El::ExtractTime) => header.extract_time = Some(text.trim().to_string()),
                    Some(top) => {
                        if let Some(d) = draft.as_mut() {
                            let in_other = stack.contains(&El::OtherEntity);
                            let in_main = stack.contains(&El::MainEntity);
                            match top {
                                El::Abn => {
                                    if let Some(abn) = digits_to_u64(&text, 11) {
                                        d.rec.abn = abn;
                                        d.has_abn = true;
                                    }
                                }
                                El::EntityTypeText if !d.type_seen => {
                                    d.type_seen = true;
                                    d.rec.entity_type = text.into_owned();
                                }
                                El::NameText if in_other => {
                                    if !text.is_empty() {
                                        d.rec.names.push((d.kind.clone(), text.into_owned()));
                                    }
                                }
                                El::NameText if in_main && d.main_name.is_none() => d.main_name = Some(text.into_owned()),
                                El::GivenName if d.individual => d.given.push(text.into_owned()),
                                El::FamilyName if d.family.is_none() => d.family = Some(text.into_owned()),
                                El::State if !d.state_seen => {
                                    d.state_seen = true;
                                    d.rec.state = text.trim().to_string();
                                }
                                El::Postcode if !d.postcode_seen => {
                                    d.postcode_seen = true;
                                    d.rec.postcode = text.trim().to_string();
                                }
                                El::AsicNumber => d.rec.acn = digits_to_u64(&text, 9).and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
                                _ => {}
                            }
                        }
                    }
                    None => {}
                }
            }
        }
    }
    if !saw_root || !closed {
        return Err(invalid("the extract is cut short: its closing Transfer element is missing"));
    }
    if let Some(declared) = header.declared
        && declared != header.records
    {
        return Err(invalid(format!("the extract holds {} records but its header says {declared}", header.records)));
    }
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Transfer error="">
  <TransferInfo><FileSequenceNumber>1</FileSequenceNumber><RecordCount>3</RecordCount><ExtractTime>2026-11-05T21:34:30</ExtractTime></TransferInfo>
  <ABR recordLastUpdatedDate="20261105" replaced="N">
    <ABN status="ACT" ABNStatusFromDate="20120701">14100200301</ABN>
    <EntityType><EntityTypeInd>PRV</EntityTypeInd><EntityTypeText>Australian Private Company</EntityTypeText></EntityType>
    <MainEntity><NonIndividualName type="MN"><NonIndividualNameText>HARBOUR &amp; CO</NonIndividualNameText></NonIndividualName><BusinessAddress><AddressDetails><State>NSW</State><Postcode>2000</Postcode></AddressDetails></BusinessAddress></MainEntity>
    <ASICNumber ASICNumberType="undetermined">100200306</ASICNumber>
    <GST status="ACT" GSTStatusFromDate="20120701"/>
    <OtherEntity><NonIndividualName type="TRD"><NonIndividualNameText>HARBOUR TRADING</NonIndividualNameText></NonIndividualName></OtherEntity>
  </ABR>
  <ABR><ABN status="CAN" ABNStatusFromDate="20240515">90100201301</ABN><EntityType><EntityTypeText>Individual/Sole Trader</EntityTypeText></EntityType>
    <LegalEntity><IndividualName type="LGL"><NameTitle>MR</NameTitle><GivenName>ALEX</GivenName><GivenName>J</GivenName><FamilyName>TESTPERSON</FamilyName></IndividualName>
    <BusinessAddress><AddressDetails><State>NSW</State><Postcode>2000</Postcode></AddressDetails></BusinessAddress></LegalEntity></ABR>
  <ABR><NoAbnHere/></ABR>
</Transfer>
"#;

    #[test]
    fn reads_entities_names_and_the_header() {
        let mut got = Vec::new();
        let h = parse(io::Cursor::new(XML.as_bytes().to_vec()), |r| {
            got.push(r);
            Ok(())
        })
        .unwrap();
        assert_eq!(h, Header { declared: Some(3), records: 3, extract_time: Some("2026-11-05T21:34:30".into()) });
        assert_eq!(got.len(), 2);
        let a = &got[0];
        assert_eq!((a.abn, a.active, a.gst, a.status_from, a.acn), (14100200301, true, true, 20120701, 100200306));
        assert_eq!((a.name.as_str(), a.state.as_str(), a.postcode.as_str()), ("HARBOUR & CO", "NSW", "2000"));
        assert_eq!(a.names, vec![("TRD".to_string(), "HARBOUR TRADING".to_string())]);
        let b = &got[1];
        assert_eq!(
            (b.name.as_str(), b.active, b.gst, b.acn, b.entity_type.as_str()),
            ("TESTPERSON, ALEX J", false, false, 0, "Individual/Sole Trader")
        );
    }

    #[test]
    fn a_short_or_miscounted_or_foreign_file_is_refused() {
        let cut = &XML[..XML.len() - 30];
        assert!(parse(io::Cursor::new(cut.as_bytes().to_vec()), |_| Ok(())).is_err());
        let wrong = XML.replace("<RecordCount>3<", "<RecordCount>4<");
        assert!(parse(io::Cursor::new(wrong.as_bytes().to_vec()), |_| Ok(())).is_err());
        assert!(parse(io::Cursor::new(b"<html></html>".to_vec()), |_| Ok(())).is_err());
        assert!(parse(io::Cursor::new(Vec::new()), |_| Ok(())).is_err());
    }
}
