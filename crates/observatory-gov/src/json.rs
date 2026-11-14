//! Answers are written by hand, so the key order is the contract's and no map order can leak in.

use std::fmt::Write;

/// A JSON string: `"` and `\` escaped, control characters as `\u00XX`, everything else as it is.
pub fn quote(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn opt(out: &mut String, value: Option<&str>) {
    match value {
        Some(v) => quote(out, v),
        None => out.push_str("null"),
    }
}

/// `"key":` after a comma when it is not the first.
pub fn key(out: &mut String, first: &mut bool, name: &str) {
    if !*first {
        out.push(',');
    }
    *first = false;
    quote(out, name);
    out.push(':');
}

/// A value copied for the query echo: long input is cut, so a hostile argument cannot make a large answer.
pub fn echo(value: &str) -> String {
    const MAX: usize = 120;
    if value.chars().count() <= MAX { value.to_string() } else { value.chars().take(MAX).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_escapes_and_controls() {
        let mut s = String::new();
        quote(&mut s, "a\"b\\c\n\u{1}é");
        assert_eq!(s, "\"a\\\"b\\\\c\\n\\u0001é\"");
    }

    #[test]
    fn echo_cuts_long_input() {
        assert_eq!(echo(&"x".repeat(500)).len(), 120);
    }
}
