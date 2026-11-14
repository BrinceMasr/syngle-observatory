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

/// `Number(text)` as JavaScript reads it: blanks around are dropped, empty text is 0, and only decimal numbers and
/// `Infinity` are numbers (not `inf` or `nan`). `None` where JavaScript gives NaN.
pub fn js_number(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    if body == "Infinity" {
        return Some(if t.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY });
    }
    if !body.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return None;
    }
    t.parse::<f64>().ok()
}

/// A number as JavaScript prints it (what the reference's `JSON.stringify` gives): the shortest text that reads back, no
/// exponent from 1e-6 up to 1e21, `null` when it is not finite, and `0` for minus zero.
pub fn number(out: &mut String, v: f64) {
    if !v.is_finite() {
        out.push_str("null");
    } else if v == 0.0 {
        out.push('0');
    } else if (1e-6..1e21).contains(&v.abs()) {
        let _ = write!(out, "{v}");
    } else {
        let text = format!("{v:e}");
        match text.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => {
                let _ = write!(out, "{m}e+{e}");
            }
            _ => out.push_str(&text),
        }
    }
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
    fn numbers_print_as_javascript_prints_them() {
        let text = |v: f64| {
            let mut s = String::new();
            number(&mut s, v);
            s
        };
        assert_eq!(
            [
                text(8500000.0),
                text(-33.8688),
                text(0.1),
                text(-0.0),
                text(1e21),
                text(1.5e-7),
                text(f64::NAN),
                text(123456789012345680000.0)
            ],
            ["8500000", "-33.8688", "0.1", "0", "1e+21", "1.5e-7", "null", "123456789012345680000"]
        );
    }

    #[test]
    fn echo_cuts_long_input() {
        assert_eq!(echo(&"x".repeat(500)).len(), 120);
    }
}
