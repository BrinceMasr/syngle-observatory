//! Dates without a calendar crate: today in UTC, and the dates an HTTP server writes.

use std::time::{SystemTime, UNIX_EPOCH};

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Today's date as `yyyy-mm-dd`, in UTC (the clock before 1970 reads as the epoch).
pub fn today() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, m, d) = civil(i64::try_from(secs / 86_400).unwrap_or(0));
    format!("{y:04}-{m:02}-{d:02}")
}

/// The year of today's date.
pub fn this_year() -> i64 {
    today().get(..4).and_then(|y| y.parse().ok()).unwrap_or(2026)
}

/// `Thu, 24 Jun 2021 22:47:40 GMT` as `2021-06-24T22:47:40`, the form CKAN gives its own dates in.
pub fn http_date(text: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    let _weekday = words.next()?;
    let day: u32 = words.next()?.parse().ok()?;
    let name = words.next()?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|m| *m == name)? + 1;
    let year: u32 = words.next()?.parse().ok()?;
    let time = words.next()?;
    let ok = time.len() == 8 && time.bytes().enumerate().all(|(i, b)| if i == 2 || i == 5 { b == b':' } else { b.is_ascii_digit() });
    (ok && (1..=31).contains(&day) && (1970..=9999).contains(&year)).then(|| format!("{year:04}-{month:02}-{day:02}T{time}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(20_528), (2026, 3, 16));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(today().len(), 10);
    }

    #[test]
    fn http_dates() {
        assert_eq!(http_date("Thu, 24 Jun 2021 22:47:40 GMT").as_deref(), Some("2021-06-24T22:47:40"));
        assert_eq!(http_date("Mon, 09 Nov 2026 13:53:06 GMT").as_deref(), Some("2026-11-09T13:53:06"));
        assert_eq!(http_date("yesterday"), None);
        assert_eq!(http_date("Thu, 24 Jun 2021 22:47 GMT"), None);
        assert_eq!(http_date("Thu, 24 Foo 2021 22:47:40 GMT"), None);
    }
}
