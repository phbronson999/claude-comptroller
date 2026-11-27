//! Archives: a date range of the black box saved to its own small SQLite file before the
//! nightly prune deletes it. Every archive keeps a summary: the traffic figures, requests by
//! weekday and hour, and the cost report together with the pricing table and seat plans used,
//! so an old archive reads the same after prices change. A raw archive also keeps a copy of
//! the requests themselves.
//!
//! One archive can be the baseline (`Archives.Baseline`): the Dashboard and the Cost screen
//! compare against it.
//!
//! The file is `<folder>\yyyy-mm-dd_to_yyyy-mm-dd.db` with a table `archive (key, value)`;
//! open it with `sqlite3` to read it by hand.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::activity;
use crate::config::Cost as CostConfig;
use crate::cost::{civil, custom_range, day_number, report_for, Report};
use crate::directory::open;
use crate::health::{format_utc, now, parse_utc};
use crate::pricing::Pricing;

/// The version of the file layout.
pub const FORMAT: i64 = 1;
const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Content {
    Summary,
    Raw,
}

impl Content {
    pub fn name(self) -> &'static str {
        match self {
            Content::Summary => "summary",
            Content::Raw => "raw",
        }
    }
}

/// Monday is 0. Day 0 (1970-01-01) was a Thursday.
pub fn weekday(day: i64) -> usize {
    (day + 3).rem_euclid(7) as usize
}

pub fn weekday_name(day: i64) -> &'static str {
    WEEKDAYS[weekday(day)]
}

/// The traffic in an archive's range.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Traffic {
    pub requests: i64,
    pub errors: i64,
    pub limit_hits: i64,
    /// Of complete /v1/messages replies.
    pub p50_ms: Option<i64>,
    pub p95_ms: Option<i64>,
    pub max_ms: Option<i64>,
    /// Requests by local weekday (Monday first) and hour.
    pub hours: Vec<Vec<i64>>,
    /// How many of each weekday the range covers, from the first recorded day in it.
    pub weekdays: Vec<i64>,
    /// Local day number and requests, oldest first.
    pub days: Vec<(i64, i64)>,
    /// Hours with at least one request.
    pub busy_hours: i64,
    /// Name, requests and plan-limit hits, most requests first.
    pub people: Vec<(String, i64, i64)>,
}

impl Traffic {
    pub fn day_count(&self) -> i64 {
        self.weekdays.iter().sum()
    }

    pub fn per_day(&self) -> f64 {
        self.requests as f64 / self.day_count().max(1) as f64
    }

    pub fn per_busy_hour(&self) -> f64 {
        self.requests as f64 / self.busy_hours.max(1) as f64
    }

    /// Errors as a percentage of requests.
    pub fn error_rate(&self) -> f64 {
        self.errors as f64 * 100.0 / self.requests.max(1) as f64
    }

    /// Whether the range has this weekday.
    pub fn has(&self, weekday: usize) -> bool {
        self.weekdays.get(weekday).is_some_and(|&n| n > 0)
    }

    /// Average requests in each hour of a `weekday`, or of any day when the range has none.
    pub fn hourly(&self, weekday: usize) -> [f64; 24] {
        let mut out = [0.0; 24];
        let days: Vec<usize> = if self.has(weekday) { vec![weekday] } else { (0..7).collect() };
        let count: i64 = days.iter().map(|&d| self.weekdays.get(d).copied().unwrap_or(0)).sum();
        for d in days {
            if let Some(hours) = self.hours.get(d) {
                for (h, n) in hours.iter().enumerate().take(24) {
                    out[h] += *n as f64;
                }
            }
        }
        out.map(|n| n / count.max(1) as f64)
    }

    /// Average requests on a `weekday`, or on any day when the range has none.
    pub fn daily(&self, weekday: usize) -> f64 {
        if self.has(weekday) {
            self.hours.get(weekday).map(|h| h.iter().sum::<i64>()).unwrap_or(0) as f64 / self.weekdays[weekday] as f64
        } else {
            self.per_day()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Archive {
    pub path: PathBuf,
    pub label: String,
    /// Local day numbers, inclusive.
    pub first_day: i64,
    pub last_day: i64,
    pub content: Content,
    pub created_utc: String,
    pub size: u64,
    pub currency: String,
    pub traffic: Traffic,
    /// None when there was no pricing table to use.
    pub cost: Option<Report>,
    /// The pricing table's own date.
    pub pricing_updated: Option<String>,
}

impl Archive {
    pub fn days(&self) -> i64 {
        self.last_day - self.first_day + 1
    }

    pub fn file_name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// The label, or the range when there's none.
    pub fn title(&self) -> String {
        if self.label.trim().is_empty() { custom_range(self.first_day, self.last_day).label } else { self.label.clone() }
    }
}

fn iso(day: i64) -> String {
    let (y, m, d) = civil(day);
    format!("{y:04}-{m:02}-{d:02}")
}

fn from_iso(text: &str) -> Option<i64> {
    let mut parts = text.split('-');
    let (y, m, d) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    Some(day_number(y, m, d))
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

// ── reading ─────────────────────────────────────────────────────────────────

/// Every archive in `folder`, newest range first. A missing folder has none.
pub fn list(folder: &Path) -> Result<Vec<Archive>, String> {
    if !folder.exists() {
        return Ok(Vec::new());
    }
    activity::track(
        format!("dir \"{}\\*.db\"", folder.display()),
        || -> Result<Vec<Archive>, String> {
            let mut archives = Vec::new();
            for entry in std::fs::read_dir(folder).map_err(|e| e.to_string())?.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("db")) {
                    // A file that isn't an archive is left out rather than failing the list.
                    if let Ok(archive) = read(&path) {
                        archives.push(archive);
                    }
                }
            }
            archives.sort_by(|a, b| b.first_day.cmp(&a.first_day).then(b.last_day.cmp(&a.last_day)).then_with(|| a.path.cmp(&b.path)));
            Ok(archives)
        },
        |a| format!("{} archives", a.len()),
    )
}

/// One archive.
pub fn open_archive(path: &Path) -> Result<Archive, String> {
    activity::track(format!("sqlite3 \"{}\" \"SELECT key, value FROM archive\"", path.display()), || read(path), |a| a.title())
}

fn read(path: &Path) -> Result<Archive, String> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
    let mut values: HashMap<String, String> = HashMap::new();
    let mut statement = c.prepare("SELECT key, value FROM archive").map_err(|e| format!("{}: not an archive ({e})", path.display()))?;
    let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
    for row in rows {
        let (k, v) = row.map_err(|e| e.to_string())?;
        values.insert(k, v);
    }
    let get = |k: &str| values.get(k).cloned().unwrap_or_default();
    let format: i64 = get("format").parse().unwrap_or(0);
    if format != FORMAT {
        return Err(format!("{}: archive format {format}, this terminal reads {FORMAT}", path.display()));
    }
    let bad = |what: &str| format!("{}: {what} is missing or damaged", path.display());
    Ok(Archive {
        path: path.to_path_buf(),
        label: get("label"),
        first_day: from_iso(&get("first_day")).ok_or_else(|| bad("first_day"))?,
        last_day: from_iso(&get("last_day")).ok_or_else(|| bad("last_day"))?,
        content: if get("content") == "raw" { Content::Raw } else { Content::Summary },
        created_utc: get("created_utc"),
        size: size(path),
        currency: values.get("currency").cloned().unwrap_or_else(|| "$".into()),
        traffic: serde_json::from_str(&get("traffic")).map_err(|_| bad("traffic"))?,
        cost: values.get("cost").and_then(|c| serde_json::from_str(c).ok()),
        pricing_updated: values.get("pricing").and_then(|p| Pricing::parse(p).ok()).map(|p| p.last_updated),
    })
}

// ── creating ────────────────────────────────────────────────────────────────

pub struct New<'a> {
    pub first_day: i64,
    pub last_day: i64,
    pub label: &'a str,
    pub content: Content,
}

/// Requests in a range of local days, and roughly how big a raw archive of them would be.
pub fn estimate(db: &Path, first_day: i64, last_day: i64, offset: i64) -> Result<(i64, u64), String> {
    let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let e = |e: rusqlite::Error| e.to_string();
    let (from, to) = (format_utc(first_day * 86_400 - offset), format_utc((last_day + 1) * 86_400 - offset));
    let rows: i64 = c.query_row("SELECT COUNT(*) FROM requests WHERE started_utc >= ?1 AND started_utc < ?2", [&from, &to], |r| r.get(0)).map_err(e)?;
    let total: i64 = c.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0)).map_err(e)?;
    let file = size(db) + size(&db.with_extension("db-wal"));
    Ok((rows, if total == 0 { 0 } else { (file as f64 * rows as f64 / total as f64) as u64 }))
}

/// The first local day with a recorded request.
pub fn first_recorded_day(db: &Path, offset: i64) -> Result<Option<i64>, String> {
    let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let oldest: Option<String> = c.query_row("SELECT MIN(started_utc) FROM requests", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    Ok(oldest.as_deref().and_then(parse_utc).map(|t| (t + offset).div_euclid(86_400)))
}

fn percentile(sorted: &[i64], p: f64) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let index = ((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1);
    Some(sorted[index.min(sorted.len() - 1)])
}

/// The traffic figures for local days `first_day..=last_day`.
pub fn traffic(db: &Path, first_day: i64, last_day: i64, offset: i64) -> Result<Traffic, String> {
    let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let e = |e: rusqlite::Error| e.to_string();
    let (from, to) = (format_utc(first_day * 86_400 - offset), format_utc((last_day + 1) * 86_400 - offset));
    let mut statement = c
        .prepare(
            "SELECT started_utc, user_name, status, duration_ms, path = '/v1/messages' AND status = 200 AND proxy_error IS NULL,
                    status >= 500 OR (proxy_error IS NOT NULL AND proxy_error <> 'RequestCanceled')
             FROM requests WHERE started_utc >= ?1 AND started_utc < ?2",
        )
        .map_err(e)?;
    let rows = statement
        .query_map([&from, &to], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, bool>(4)?, r.get::<_, bool>(5)?))
        })
        .map_err(e)?;

    let mut t = Traffic { hours: vec![vec![0; 24]; 7], weekdays: vec![0; 7], ..Default::default() };
    let mut durations = Vec::new();
    let mut days: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
    let mut busy = std::collections::HashSet::new();
    let mut people: HashMap<String, (i64, i64)> = HashMap::new();
    for row in rows {
        let (started, user, status, ms, complete, error) = row.map_err(e)?;
        let local = parse_utc(&started).unwrap_or(0) + offset;
        let day = local.div_euclid(86_400);
        let hour = (local.rem_euclid(86_400) / 3600) as usize;
        t.requests += 1;
        t.errors += i64::from(error);
        t.limit_hits += i64::from(status == 429);
        t.hours[weekday(day)][hour] += 1;
        *days.entry(day).or_default() += 1;
        busy.insert((day, hour));
        if complete {
            durations.push(ms);
        }
        if let Some(user) = user {
            let p = people.entry(user).or_default();
            p.0 += 1;
            p.1 += i64::from(status == 429);
        }
    }
    // Days before anything was recorded would only lower the averages.
    let start = days.keys().next().copied().unwrap_or(first_day).max(first_day);
    for day in start..=last_day {
        t.weekdays[weekday(day)] += 1;
    }
    durations.sort_unstable();
    t.p50_ms = percentile(&durations, 0.5);
    t.p95_ms = percentile(&durations, 0.95);
    t.max_ms = durations.last().copied();
    t.days = days.into_iter().collect();
    t.busy_hours = busy.len() as i64;
    t.people = people.into_iter().map(|(name, (n, hits))| (name, n, hits)).collect();
    t.people.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(t)
}

/// A file name for the range that isn't taken yet.
fn free_name(folder: &Path, first_day: i64, last_day: i64) -> PathBuf {
    let base = format!("{}_to_{}", iso(first_day), iso(last_day));
    let mut path = folder.join(format!("{base}.db"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{base}_{n}.db"));
        n += 1;
    }
    path
}

/// Saves an archive of `new`'s range from the database `db` into `folder`. `pricing` is the
/// table to price tokens with and its text, kept in the archive.
pub fn create(db: &Path, folder: &Path, new: &New, cost: &CostConfig, pricing: Option<(&Pricing, &str)>, offset: i64) -> Result<Archive, String> {
    if new.first_day > new.last_day {
        return Err("The range ends before it starts.".into());
    }
    let today = (now() + offset).div_euclid(86_400);
    if new.first_day > today {
        return Err("The range is in the future.".into());
    }
    let last_day = new.last_day.min(today);
    let traffic = traffic(db, new.first_day, last_day, offset)?;
    if traffic.requests == 0 {
        return Err("Nothing is recorded in that range.".into());
    }
    let report = match pricing {
        Some((table, _)) => Some(report_for(db, cost, table, custom_range(new.first_day, last_day), offset)?),
        None => None,
    };

    std::fs::create_dir_all(folder).map_err(|e| format!("Can't create {}: {e}", folder.display()))?;
    let path = free_name(folder, new.first_day, last_day);
    let temp = path.with_extension("db.new");
    let _ = std::fs::remove_file(&temp);
    let (from, to) = (format_utc(new.first_day * 86_400 - offset), format_utc((last_day + 1) * 86_400 - offset));
    let shown = match new.content {
        Content::Summary => format!("sqlite3 \"{}\" \"CREATE TABLE archive (key, value); INSERT … summary\"", path.display()),
        Content::Raw => format!(
            "sqlite3 \"{}\" \"ATTACH '{}' AS src; CREATE TABLE requests AS SELECT * FROM src.requests WHERE started_utc >= '{from}' AND started_utc < '{to}'; …\"",
            path.display(),
            db.display()
        ),
    };
    let write = || -> Result<(), String> {
        let c = Connection::open(&temp).map_err(|e| e.to_string())?;
        let e = |e: rusqlite::Error| e.to_string();
        c.execute_batch("CREATE TABLE archive (key TEXT PRIMARY KEY, value TEXT NOT NULL);").map_err(e)?;
        let plans = serde_json::to_string(&cost.seat_plans).map_err(|e| e.to_string())?;
        let mut values: Vec<(&str, String)> = vec![
            ("format", FORMAT.to_string()),
            ("label", new.label.trim().to_string()),
            ("first_day", iso(new.first_day)),
            ("last_day", iso(last_day)),
            ("content", new.content.name().into()),
            ("created_utc", format_utc(now())),
            ("utc_offset_seconds", offset.to_string()),
            ("currency", cost.currency.clone()),
            ("seat_plans", plans),
            ("traffic", serde_json::to_string(&traffic).map_err(|e| e.to_string())?),
        ];
        if let (Some(report), Some((_, text))) = (&report, pricing) {
            values.push(("cost", serde_json::to_string(report).map_err(|e| e.to_string())?));
            values.push(("pricing", text.to_string()));
        }
        for (k, v) in values {
            c.execute("INSERT INTO archive (key, value) VALUES (?1, ?2)", [k, v.as_str()]).map_err(e)?;
        }
        if new.content == Content::Raw {
            c.execute("ATTACH DATABASE ?1 AS src", [db.to_string_lossy().as_ref()]).map_err(e)?;
            c.execute("CREATE TABLE requests AS SELECT * FROM src.requests WHERE started_utc >= ?1 AND started_utc < ?2", [&from, &to]).map_err(e)?;
            c.execute_batch("DETACH DATABASE src;").map_err(e)?;
        }
        drop(c);
        std::fs::rename(&temp, &path).map_err(|e| e.to_string())
    };
    activity::track(shown, write, |_| "archive saved".into()).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("Can't save the archive: {e}")
    })?;
    read(&path)
}

pub fn delete(path: &Path) -> Result<(), String> {
    activity::track(format!("del \"{}\"", path.display()), || std::fs::remove_file(path), |_| "deleted".into()).map_err(|e| format!("Can't delete {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SeatPlan;

    #[test]
    fn weekdays() {
        assert_eq!(weekday_name(day_number(2026, 11, 27)), "Friday");
        assert_eq!(weekday(day_number(2026, 11, 30)), 0);
    }

    #[test]
    fn an_archive_keeps_its_figures_and_prices() {
        let dir = std::env::temp_dir().join(format!("comptroller-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("comptroller.db");
        let c = Connection::open(&db).unwrap();
        c.execute_batch(include_str!("../../contracts/schema.sql")).unwrap();
        c.execute("INSERT INTO schema_version (version) VALUES (?1)", [crate::SCHEMA_VERSION as i64]).unwrap();
        c.execute("INSERT INTO users (name, source, created_utc, seat_plan) VALUES ('goose', 'manual', '2020-01-01T00:00:00.000Z', 'Max')", []).unwrap();
        let today = now().div_euclid(86_400);
        // Noon UTC, with no offset: 3 requests 3 days ago, 1 (a 429) 2 days ago, 2 today.
        for (days_ago, status) in [(3, 200), (3, 200), (3, 200), (2, 429), (0, 200), (0, 200)] {
            c.execute(
                "INSERT INTO requests (started_utc, user_name, method, path, status, duration_ms, response_bytes, model, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, usage)
                 VALUES (?1, 'goose', 'POST', '/v1/messages', ?2, 1000, 10, 'claude-opus-5-5', 1000, 1000, 0, 0, CASE WHEN ?2 = 200 THEN 'parsed' END)",
                rusqlite::params![format_utc((today - days_ago) * 86_400 + 43_200), status],
            )
            .unwrap();
        }
        drop(c);

        let mut cost = CostConfig::default();
        cost.seat_plans = vec![SeatPlan { name: "Max".into(), billing: "monthly".into(), price: Some(100.0), renewal_date: None }];
        let pricing = Pricing::parse(crate::pricing::BUILT_IN).unwrap();
        let folder = dir.join("archives");
        let new = New { first_day: today - 3, last_day: today - 1, label: "week 1", content: Content::Raw };
        let a = create(&db, &folder, &new, &cost, Some((&pricing, crate::pricing::BUILT_IN)), 0).unwrap();

        assert_eq!(a.file_name(), format!("{}_to_{}.db", iso(today - 3), iso(today - 1)));
        assert_eq!((a.traffic.requests, a.traffic.limit_hits), (4, 1));
        assert_eq!(a.traffic.day_count(), 3, "from the first recorded day to the end");
        assert_eq!(a.traffic.days, vec![(today - 3, 3), (today - 2, 1)]);
        assert_eq!(a.traffic.hourly(weekday(today - 3))[12], 3.0);
        let report = a.cost.as_ref().unwrap();
        assert_eq!(report.tokens.total(), 6000, "today's requests aren't in the range");
        assert!(report.seat > 0.0 && report.api > 0.0);
        assert!(a.pricing_updated.is_some());
        let rows: i64 = Connection::open(&a.path).unwrap().query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 4);

        // The same range again gets its own file; both are listed, newest range first.
        let summary = New { content: Content::Summary, label: "", ..new };
        let b = create(&db, &folder, &summary, &cost, None, 0).unwrap();
        assert_ne!(a.path, b.path);
        assert!(b.cost.is_none() && b.title().contains('–'));
        assert_eq!(list(&folder).unwrap().len(), 2);
        assert!(create(&db, &folder, &New { first_day: today - 30, last_day: today - 20, label: "", content: Content::Summary }, &cost, None, 0).is_err());

        delete(&b.path).unwrap();
        assert_eq!(list(&folder).unwrap(), vec![open_archive(&a.path).unwrap()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
