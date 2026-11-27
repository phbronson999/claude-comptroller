//! Everything the admin terminal does that isn't drawing: reading the config, talking to
//! Windows Service Control and the service's /health, installing, the preflight test,
//! changing settings, and the users and devices in the database.

pub mod activity;
pub mod config;
pub mod directory;
pub mod health;
pub mod install;
pub mod preflight;
pub mod service;
pub mod cost;
pub mod dates;
pub mod pricing;
pub mod settings;
pub mod traffic;

/// Seconds to add to UTC for this computer's local time, now.
pub fn local_offset() -> i64 {
    // SAFETY: fills plain structs.
    let (local, utc) = unsafe { (windows::Win32::System::SystemInformation::GetLocalTime(), windows::Win32::System::SystemInformation::GetSystemTime()) };
    let secs = |t: &windows::Win32::Foundation::SYSTEMTIME| {
        health::parse_utc(&format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)).unwrap_or(0)
    };
    // Rounded to a quarter hour: the two readings can straddle a second.
    ((secs(&local) - secs(&utc)) as f64 / 900.0).round() as i64 * 900
}

/// Local time of day, `HH:MM:SS`, for seconds since 1970.
pub fn local_clock(utc: i64) -> String {
    let t = (utc + local_offset()).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", t / 3600, t % 3600 / 60, t % 60)
}

/// The database schema version this build understands. Matches the service's.
pub const SCHEMA_VERSION: u32 = 3;

/// Formats a duration in seconds as "3d 4h", "4h 12m" or "12m".
pub fn short_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (d, h) {
        (0, 0) => format!("{}m", m.max(if secs > 0 { 1 } else { 0 })),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

/// Formats a duration in seconds as "3 days 4 h 12 min".
pub fn long_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    let mut parts = Vec::new();
    if d > 0 {
        parts.push(format!("{d} day{}", if d == 1 { "" } else { "s" }));
    }
    if h > 0 || d > 0 {
        parts.push(format!("{h} h"));
    }
    parts.push(format!("{m} min"));
    parts.join(" ")
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{n} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations_and_sizes() {
        assert_eq!(short_duration(3 * 86_400 + 4 * 3_600 + 60), "3d 4h");
        assert_eq!(short_duration(4 * 3_600 + 12 * 60), "4h 12m");
        assert_eq!(short_duration(30), "1m");
        assert_eq!(long_duration(3 * 86_400 + 4 * 3_600 + 12 * 60), "3 days 4 h 12 min");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(12_373_196), "11.8 MB");
    }
}
