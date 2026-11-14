//! The two ASIC datasets from data.gov.au: tab-separated although the file says `.csv`, a UTF-8 byte order mark,
//! CRLF line ends, dates as `dd/mm/yyyy`. One row per company name (the company dataset) and one row per business name.

use crate::codec::{BusinessName, Company};
use crate::norm::{digits_to_u64, from_dmy};
use crate::rd::{invalid, read_until};
use std::collections::HashMap;
use std::io::{self, BufRead};

const MAX_LINE: usize = 1 << 20;

/// The rows of a tab-separated file, each as its cells; the header is read first and names the columns.
struct Rows<R> {
    r: R,
    line: Vec<u8>,
    first: bool,
}

impl<R: BufRead> Rows<R> {
    fn new(r: R) -> Self {
        Rows { r, line: Vec::new(), first: true }
    }

    /// The next non-empty line as cells; `None` at the end.
    fn row(&mut self) -> io::Result<Option<Vec<String>>> {
        loop {
            self.line.clear();
            let found = read_until(&mut self.r, b'\n', &mut self.line, MAX_LINE)?;
            if !found && self.line.is_empty() {
                return Ok(None);
            }
            let mut bytes = self.line.as_slice();
            if self.first {
                bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
                self.first = false;
            }
            while let Some(rest) = bytes.strip_suffix(b"\r") {
                bytes = rest;
            }
            if bytes.is_empty() {
                continue;
            }
            return Ok(Some(String::from_utf8_lossy(bytes).split('\t').map(str::to_string).collect()));
        }
    }
}

fn column(header: &[String], name: &str, file: &str) -> io::Result<usize> {
    header.iter().position(|h| h.trim() == name).ok_or_else(|| invalid(format!("this is not the {file} file: there is no {name} column")))
}

fn cell(row: &[String], at: usize) -> &str {
    row.get(at).map_or("", |s| s.trim())
}

/// The company dataset, one record per ACN: the row marked as the current name sets the name, status, dates and ABN;
/// every other row adds a former name. Sorted by ACN.
pub fn read_companies<R: BufRead>(r: R) -> io::Result<Vec<Company>> {
    let mut rows = Rows::new(r);
    let header = rows.row()?.ok_or_else(|| invalid("the ASIC company file is empty"))?;
    let (name, acn, status, reg, dereg, current, abn) = (
        column(&header, "Company Name", "ASIC company")?,
        column(&header, "ACN", "ASIC company")?,
        column(&header, "Status", "ASIC company")?,
        column(&header, "Date of Registration", "ASIC company")?,
        column(&header, "Date of Deregistration", "ASIC company")?,
        column(&header, "Current Name Indicator", "ASIC company")?,
        column(&header, "ABN", "ASIC company")?,
    );
    let mut by_acn: HashMap<u32, Company> = HashMap::new();
    while let Some(row) = rows.row()? {
        let Some(number) = digits_to_u64(cell(&row, acn), 9).and_then(|n| u32::try_from(n).ok()) else { continue };
        let entry = by_acn.entry(number).or_insert_with(|| Company { acn: number, ..Company::default() });
        if cell(&row, current) == "Y" {
            entry.name = Some(cell(&row, name).to_string());
            entry.status = Some(match cell(&row, status) {
                "REGD" => "Registered".to_string(),
                "DRGD" => "Deregistered".to_string(),
                other => other.to_string(),
            });
            entry.registered = from_dmy(cell(&row, reg));
            entry.deregistered = from_dmy(cell(&row, dereg));
            if let Some(n) = digits_to_u64(cell(&row, abn), 11) {
                entry.abn = n;
            }
        } else if !cell(&row, name).is_empty() {
            entry.former.push(cell(&row, name).to_string());
        }
    }
    let mut out: Vec<Company> = by_acn.into_values().collect();
    for c in &mut out {
        c.former.sort();
        c.former.dedup();
    }
    out.sort_by_key(|c| c.acn);
    Ok(out)
}

/// The business names dataset: every row with a name goes to `sink`; the count of rows read.
pub fn read_business_names<R: BufRead>(r: R, mut sink: impl FnMut(BusinessName) -> io::Result<()>) -> io::Result<u64> {
    let mut rows = Rows::new(r);
    let header = rows.row()?.ok_or_else(|| invalid("the ASIC business names file is empty"))?;
    let (name, status, reg, cancel, abn) = (
        column(&header, "BN_NAME", "ASIC business names")?,
        column(&header, "BN_STATUS", "ASIC business names")?,
        column(&header, "BN_REG_DT", "ASIC business names")?,
        column(&header, "BN_CANCEL_DT", "ASIC business names")?,
        column(&header, "BN_ABN", "ASIC business names")?,
    );
    let mut count = 0;
    while let Some(row) = rows.row()? {
        let n = cell(&row, name);
        if n.is_empty() {
            continue;
        }
        count += 1;
        sink(BusinessName {
            name: n.to_string(),
            status: cell(&row, status).to_string(),
            registered: from_dmy(cell(&row, reg)),
            cancelled: from_dmy(cell(&row, cancel)),
            abn: digits_to_u64(cell(&row, abn), 11).unwrap_or(0),
        })?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPANY: &str = "\u{feff}Company Name\tACN\tType\tClass\tSub Class\tStatus\tDate of Registration\tDate of Deregistration\tPrevious State of Registration\tState Registration number\tModified since last report\tCurrent Name Indicator\tABN\tCurrent Name\tCurrent Name Start Date\r\n\
BLUE GUM ADVISORY GROUP PTY LTD\t100200600\tAPTY\tLMSH\tPROP\tREGD\t08/01/1990\t\tNSW\t1\t\tY\t90100200604\t\t\r\n\
BLUE GUM CONSULTING PTY LTD\t100200600\tAPTY\tLMSH\tPROP\tREGD\t08/01/1990\t\tNSW\t1\t\tN\t90100200604\t\t\r\n\
OLD CO\t100200806\tAPTY\tLMSH\tPROP\tDRGD\t08/01/1990\t15/05/2024\tNSW\t1\t\tY\t\t\t\r\n";

    #[test]
    fn companies_group_by_acn() {
        let c = read_companies(io::Cursor::new(COMPANY.as_bytes().to_vec())).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name.as_deref(), Some("BLUE GUM ADVISORY GROUP PTY LTD"));
        assert_eq!(c[0].former, vec!["BLUE GUM CONSULTING PTY LTD".to_string()]);
        assert_eq!((c[0].abn, c[0].registered), (90100200604, 19900108));
        assert_eq!((c[1].status.as_deref(), c[1].deregistered, c[1].abn), (Some("Deregistered"), 20240515, 0));
    }

    #[test]
    fn a_file_of_another_shape_is_refused() {
        assert!(read_companies(io::Cursor::new(b"a\tb\r\n1\t2\r\n".to_vec())).is_err());
        assert!(read_companies(io::Cursor::new(Vec::new())).is_err());
        assert!(read_business_names(io::Cursor::new(b"x\r\n".to_vec()), |_| Ok(())).is_err());
    }
}
