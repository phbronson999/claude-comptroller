//! Dates the way this computer shows them: Windows' short date format, as in the taskbar
//! clock (11/27/2026 in the US, 27/11/2026 in the UK, 2026-11-27 in Sweden). Typing accepts
//! that format and yyyy-mm-dd. Files always store yyyy-mm-dd, which means the same anywhere.

use std::sync::OnceLock;

/// The short date pattern, such as `M/d/yyyy`. Falls back to `yyyy-MM-dd`.
pub fn pattern() -> &'static str {
    static PATTERN: OnceLock<String> = OnceLock::new();
    PATTERN.get_or_init(|| windows_pattern().unwrap_or_else(|| "yyyy-MM-dd".into()))
}

fn windows_pattern() -> Option<String> {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SSHORTDATE};
    let mut buffer = [0u16; 80];
    // SAFETY: a null locale name means the user's default; the buffer is ours.
    let written = unsafe { GetLocaleInfoEx(windows::core::PCWSTR::null(), LOCALE_SSHORTDATE, Some(&mut buffer)) };
    if written <= 1 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..written as usize - 1]);
    // Only day, month and year numbers are supported; a pattern with names falls back.
    (text.contains('d') && text.contains('M') && text.contains('y') && !text.contains("MMM") && !text.contains("ddd")).then_some(text)
}

/// Formats a date with `pattern`.
pub fn format_with(pattern: &str, y: i64, m: u32, d: u32) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let run = chars[i..].iter().take_while(|&&x| x == c).count();
        match c {
            'd' => out += &if run >= 2 { format!("{d:02}") } else { d.to_string() },
            'M' => out += &if run >= 2 { format!("{m:02}") } else { m.to_string() },
            'y' => out += &if run >= 3 { format!("{y:04}") } else { format!("{:02}", y.rem_euclid(100)) },
            // Quoted literal text.
            '\'' => {}
            other => out.extend(std::iter::repeat_n(other, run)),
        }
        i += run;
    }
    out
}

pub fn format(y: i64, m: u32, d: u32) -> String {
    format_with(pattern(), y, m, d)
}

/// A stored `yyyy-mm-dd` date, shown the local way. Anything else is shown as it is.
pub fn show_iso(iso: &str) -> String {
    parse_with("yyyy-MM-dd", iso).map(|(y, m, d)| format(y, m, d)).unwrap_or_else(|| iso.to_string())
}

/// Reads a typed date in `pattern`'s order, or as yyyy-mm-dd. Two-digit years are 20xx.
pub fn parse_with(pattern: &str, text: &str) -> Option<(i64, u32, u32)> {
    let numbers: Vec<&str> = text.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).collect();
    if numbers.len() != 3 {
        return None;
    }
    // A four-digit first number can only be a year: yyyy-mm-dd, whatever the local order.
    let order: Vec<char> = if numbers[0].len() == 4 {
        vec!['y', 'M', 'd']
    } else {
        let mut seen = Vec::new();
        for c in pattern.chars().filter(|c| matches!(c, 'd' | 'M' | 'y')) {
            if !seen.contains(&c) {
                seen.push(c);
            }
        }
        seen
    };
    if order.len() != 3 {
        return None;
    }
    let mut y = None;
    let (mut m, mut d) = (None, None);
    for (part, kind) in numbers.iter().zip(&order) {
        let n: i64 = part.parse().ok()?;
        match kind {
            'y' => y = Some(if part.len() <= 2 { 2000 + n } else { n }),
            'M' => m = Some(n as u32),
            _ => d = Some(n as u32),
        }
    }
    let (y, m, d) = (y?, m?, d?);
    let real = (1..=12).contains(&m) && d >= 1 && {
        let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        let days = crate::cost::day_number(ny, nm, 1) - crate::cost::day_number(y, m, 1);
        (d as i64) <= days
    };
    real.then_some((y, m, d))
}

pub fn parse(text: &str) -> Option<(i64, u32, u32)> {
    parse_with(pattern(), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_windows() {
        assert_eq!(format_with("M/d/yyyy", 2027, 9, 9), "9/9/2027");
        assert_eq!(format_with("dd/MM/yyyy", 2027, 9, 9), "09/09/2027");
        assert_eq!(format_with("yyyy-MM-dd", 2027, 9, 9), "2027-09-09");
        assert_eq!(format_with("d.M.yy", 2027, 3, 1), "1.3.27");
    }

    #[test]
    fn reads_the_local_order_and_iso() {
        assert_eq!(parse_with("M/d/yyyy", "09/09/2027"), Some((2027, 9, 9)));
        assert_eq!(parse_with("M/d/yyyy", "3/1/2027"), Some((2027, 3, 1)));
        assert_eq!(parse_with("dd/MM/yyyy", "3/1/2027"), Some((2027, 1, 3)));
        assert_eq!(parse_with("M/d/yyyy", "2027-03-01"), Some((2027, 3, 1)));
        assert_eq!(parse_with("M/d/yy", "3/1/27"), Some((2027, 3, 1)));
        assert_eq!(parse_with("M/d/yyyy", "2/30/2027"), None);
        assert_eq!(parse_with("M/d/yyyy", "13/1/2027"), None);
        assert_eq!(parse_with("M/d/yyyy", "next week"), None);
    }

    #[test]
    fn stored_dates_are_shown_the_local_way() {
        assert!(!show_iso("2027-03-01").is_empty());
        assert_eq!(show_iso("not a date"), "not a date");
    }
}
