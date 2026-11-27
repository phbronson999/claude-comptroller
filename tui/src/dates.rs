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

// ── times of day ────────────────────────────────────────────────────────────

/// The short time pattern, such as `h:mm tt`. Falls back to `HH:mm`.
pub fn time_pattern() -> &'static str {
    static PATTERN: OnceLock<String> = OnceLock::new();
    PATTERN.get_or_init(|| {
        use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SSHORTTIME};
        let mut buffer = [0u16; 80];
        // SAFETY: a null locale name means the user's default; the buffer is ours.
        let written = unsafe { GetLocaleInfoEx(windows::core::PCWSTR::null(), LOCALE_SSHORTTIME, Some(&mut buffer)) };
        let text = if written > 1 { String::from_utf16_lossy(&buffer[..written as usize - 1]) } else { String::new() };
        if text.contains('m') && (text.contains('h') || text.contains('H')) { text } else { "HH:mm".into() }
    })
}

/// Formats a time of day with `pattern`: h/hh 12-hour, H/HH 24-hour, mm minutes, tt AM/PM.
pub fn format_time_with(pattern: &str, hour: u32, minute: u32) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let run = chars[i..].iter().take_while(|&&x| x == c).count();
        let twelve = if hour % 12 == 0 { 12 } else { hour % 12 };
        match c {
            'h' => out += &if run >= 2 { format!("{twelve:02}") } else { twelve.to_string() },
            'H' => out += &if run >= 2 { format!("{hour:02}") } else { hour.to_string() },
            'm' => out += &format!("{minute:02}"),
            't' => out += if hour < 12 { if run >= 2 { "AM" } else { "A" } } else if run >= 2 { "PM" } else { "P" },
            's' => out += "00",
            '\'' => {}
            other => out.extend(std::iter::repeat_n(other, run)),
        }
        i += run;
    }
    out
}

pub fn format_time(hour: u32, minute: u32) -> String {
    format_time_with(time_pattern(), hour, minute)
}

/// Reads a time typed as 2:00, 02:00, 14:30, 2am or 2:30 PM.
pub fn parse_time(text: &str) -> Option<(u32, u32)> {
    let lower = text.trim().to_ascii_lowercase();
    let (body, half) = if let Some(b) = lower.strip_suffix("am").or_else(|| lower.strip_suffix("a")) {
        (b.trim(), Some(false))
    } else if let Some(b) = lower.strip_suffix("pm").or_else(|| lower.strip_suffix("p")) {
        (b.trim(), Some(true))
    } else {
        (lower.as_str(), None)
    };
    let mut parts = body.split([':', '.', ' ']).filter(|p| !p.is_empty());
    let hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next().map(|m| m.parse().ok()).unwrap_or(Some(0))?;
    if parts.next().is_some() || minute > 59 {
        return None;
    }
    let hour = match half {
        Some(pm) if (1..=12).contains(&hour) => (hour % 12) + if pm { 12 } else { 0 },
        Some(_) => return None,
        None if hour <= 23 => hour,
        None => return None,
    };
    Some((hour, minute))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_and_show_the_local_way() {
        assert_eq!(format_time_with("h:mm tt", 2, 0), "2:00 AM");
        assert_eq!(format_time_with("h:mm tt", 14, 30), "2:30 PM");
        assert_eq!(format_time_with("HH:mm", 2, 0), "02:00");
        assert_eq!(parse_time("2:00"), Some((2, 0)));
        assert_eq!(parse_time("14:30"), Some((14, 30)));
        assert_eq!(parse_time("2am"), Some((2, 0)));
        assert_eq!(parse_time("2:30 PM"), Some((14, 30)));
        assert_eq!(parse_time("12:15 am"), Some((0, 15)));
        assert_eq!(parse_time("12 pm"), Some((12, 0)));
        assert_eq!(parse_time("25:00"), None);
        assert_eq!(parse_time("13 pm"), None);
        assert_eq!(parse_time(&format_time(2, 0)), Some((2, 0)), "the local format reads back");
    }

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
