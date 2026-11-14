//! Name normalisation, the ABN check digit and the date forms of the official files.

/// Upper case, `&` read as AND, every other character that is not A to Z or 0 to 9 a space, spaces collapsed and
/// trimmed. The same function reads the data and the query, so a match is a match of this form.
pub fn normalise(text: &str) -> String {
    fn push(out: &mut String, c: char, space: &mut bool) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            *space = false;
        } else if !*space {
            out.push(' ');
            *space = true;
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.chars() {
        if c == '&' {
            push(&mut out, ' ', &mut space);
            for a in "AND".chars() {
                push(&mut out, a, &mut space);
            }
            push(&mut out, ' ', &mut space);
            continue;
        }
        for u in c.to_uppercase() {
            push(&mut out, u, &mut space);
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The ABR's check: 11 digits, the first less one, weighted, the sum divisible by 89.
pub fn abn_valid(digits: &str) -> bool {
    const WEIGHTS: [i64; 11] = [10, 1, 3, 5, 7, 9, 11, 13, 15, 17, 19];
    if digits.len() != 11 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let sum: i64 = digits.bytes().zip(WEIGHTS).enumerate().map(|(i, (b, w))| (i64::from(b - b'0') - i64::from(i == 0)) * w).sum();
    sum % 89 == 0
}

/// The argument of `abn`, `company` and `business-names` with every kind of space taken out.
pub fn strip_spaces(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// `dd/mm/yyyy` (the ASIC files) as `yyyymmdd`; 0 when it is not that form.
pub fn from_dmy(text: &str) -> u32 {
    let b = text.trim().as_bytes();
    if b.len() != 10 || b.get(2) != Some(&b'/') || b.get(5) != Some(&b'/') {
        return 0;
    }
    let digits = |r: std::ops::Range<usize>| {
        b.get(r).filter(|s| s.iter().all(u8::is_ascii_digit)).map(|s| s.iter().fold(0u32, |n, d| n * 10 + u32::from(d - b'0')))
    };
    match (digits(0..2), digits(3..5), digits(6..10)) {
        (Some(d), Some(m), Some(y)) => y * 10_000 + m * 100 + d,
        _ => 0,
    }
}

/// `yyyymmdd` (the ABN extract) as it is; 0 when it is not 8 digits.
pub fn from_ymd8(text: &str) -> u32 {
    let t = text.trim();
    if t.len() == 8 && t.bytes().all(|b| b.is_ascii_digit()) { t.parse().unwrap_or(0) } else { 0 }
}

/// `yyyymmdd` as `yyyy-mm-dd`; `None` for 0.
pub fn iso(date: u32) -> Option<String> {
    (date != 0).then(|| format!("{:04}-{:02}-{:02}", date / 10_000, (date / 100) % 100, date % 100))
}

/// Up to 11 digits of an ABN or 9 of an ACN as a number; `None` for anything else.
pub fn digits_to_u64(text: &str, len: usize) -> Option<u64> {
    let t = text.trim();
    if t.len() == len && t.bytes().all(|b| b.is_ascii_digit()) { t.parse().ok() } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_form() {
        assert_eq!(normalise("  harbour  roofing, pty ltd "), "HARBOUR ROOFING PTY LTD");
        assert_eq!(normalise("HARBOUR & CO PTY LTD"), "HARBOUR AND CO PTY LTD");
        assert_eq!(normalise("harbour and co"), "HARBOUR AND CO");
        assert_eq!(normalise("\u{202e}\u{1}harbour"), "HARBOUR");
        assert_eq!(normalise("é-ß"), "SS");
        assert_eq!(normalise(""), "");
        assert_eq!(normalise("&"), "AND");
    }

    #[test]
    fn check_digit() {
        assert!(abn_valid("14100200301"));
        assert!(!abn_valid("12345678901"));
        assert!(!abn_valid("1410020030"));
        assert!(!abn_valid("hello"));
    }

    #[test]
    fn dates() {
        assert_eq!(iso(from_dmy("08/01/1990")), Some("1990-01-08".to_string()));
        assert_eq!(from_dmy(""), 0);
        assert_eq!(iso(from_ymd8("20120701")), Some("2012-07-01".to_string()));
        assert_eq!(iso(0), None);
    }
}
