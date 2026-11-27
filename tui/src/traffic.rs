//! Recorded requests, read from the service's database: the Radar feed, today's figures and
//! per-person counts. "Today" is the server's local day, as the service counts it for /me.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params_from_iter, OpenFlags, Row};

use crate::activity;
use crate::directory::open;
use crate::health::parse_utc;

/// The SQL for local midnight, in the stored UTC format.
const MIDNIGHT: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now', 'localtime', 'start of day', 'utc')";
/// A request that went wrong on the way: a server error, or no complete reply. A client
/// that cancels (Esc in Claude Code) isn't an error.
const IS_ERROR: &str = "(status >= 500 OR (proxy_error IS NOT NULL AND proxy_error <> 'RequestCanceled'))";

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: i64,
    pub started: i64,
    pub user: Option<String>,
    pub device: Option<String>,
    pub client_ip: Option<String>,
    pub method: String,
    pub path: String,
    pub status: i64,
    pub duration_ms: i64,
    pub request_bytes: Option<i64>,
    pub response_bytes: i64,
    pub proxy_error: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    /// `parsed`, `missing` or none.
    pub usage: Option<String>,
}

impl Request {
    pub fn is_error(&self) -> bool {
        self.status >= 500 || self.proxy_error.as_deref().is_some_and(|e| e != "RequestCanceled")
    }

    pub fn limit_hit(&self) -> bool {
        self.status == 429
    }
}

/// Today's figures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Today {
    pub requests: i64,
    /// Of successful /v1/messages replies.
    pub p95_ms: Option<i64>,
    pub limit_hits: i64,
    /// 429s by person, most first.
    pub limit_hits_by_user: Vec<(String, i64)>,
    pub errors: i64,
    /// Replies whose token usage couldn't be read.
    pub usage_missing: i64,
    pub by_user: HashMap<String, i64>,
    pub by_device: HashMap<(String, String), i64>,
    pub last: Option<(i64, String)>,
    /// Computers that sent something in the last 10 minutes, and whether one hit a limit today.
    pub active: Vec<(String, String, bool)>,
}

const COLUMNS: &str = "id, started_utc, user_name, device_name, client_ip, method, path, status, duration_ms, request_bytes, response_bytes,
    proxy_error, model, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, usage";

fn request(r: &Row) -> rusqlite::Result<Request> {
    Ok(Request {
        id: r.get(0)?,
        started: parse_utc(&r.get::<_, String>(1)?).unwrap_or(0),
        user: r.get(2)?,
        device: r.get(3)?,
        client_ip: r.get(4)?,
        method: r.get(5)?,
        path: r.get(6)?,
        status: r.get(7)?,
        duration_ms: r.get(8)?,
        request_bytes: r.get(9)?,
        response_bytes: r.get(10)?,
        proxy_error: r.get(11)?,
        model: r.get(12)?,
        input_tokens: r.get(13)?,
        output_tokens: r.get(14)?,
        cache_write_tokens: r.get(15)?,
        cache_read_tokens: r.get(16)?,
        usage: r.get(17)?,
    })
}

/// The newest requests, newest first. `None` when there's no database yet.
pub fn recent(db: &Path, limit: usize, user: Option<&str>, errors_only: bool) -> Result<Option<Vec<Request>>, String> {
    if !db.exists() {
        return Ok(None);
    }
    let mut filters = Vec::new();
    let mut values: Vec<String> = Vec::new();
    if let Some(user) = user {
        filters.push("user_name = ?".to_string());
        values.push(user.to_string());
    }
    if errors_only {
        filters.push(format!("({IS_ERROR} OR status = 429)"));
    }
    let filter = if filters.is_empty() { String::new() } else { format!(" WHERE {}", filters.join(" AND ")) };
    let sql = format!("SELECT {COLUMNS} FROM requests{filter} ORDER BY id DESC LIMIT {limit}");
    let shown = format!("sqlite3 \"{}\" \"SELECT … FROM requests{} ORDER BY id DESC LIMIT {limit}\"", db.display(), filter.replace('?', &format!("'{}'", user.unwrap_or_default())));
    activity::track(
        shown,
        || -> Result<Vec<Request>, String> {
            let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let mut statement = c.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = statement.query_map(params_from_iter(values.iter()), request).map_err(|e| e.to_string())?;
            rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
        },
        |rows| format!("{} rows", rows.len()),
    )
    .map(Some)
}

/// Today's figures. `None` when there's no database yet.
pub fn today(db: &Path) -> Result<Option<Today>, String> {
    if !db.exists() {
        return Ok(None);
    }
    activity::track(
        format!("sqlite3 \"{}\" \"SELECT … FROM requests WHERE started_utc >= <local midnight>\"", db.display()),
        || read_today(db),
        |t| format!("{} today", t.requests),
    )
    .map(Some)
}

fn read_today(db: &Path) -> Result<Today, String> {
    let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let e = |e: rusqlite::Error| e.to_string();
    let mut today = Today::default();

    let mut by = c
        .prepare(&format!(
            "SELECT COALESCE(user_name, ''), COALESCE(device_name, ''), COUNT(*), SUM(status = 429), SUM({IS_ERROR}), SUM(usage = 'missing')
             FROM requests WHERE started_utc >= {MIDNIGHT} GROUP BY 1, 2"
        ))
        .map_err(e)?;
    let rows = by
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, Option<i64>>(5)?)))
        .map_err(e)?;
    let mut limits: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let (user, device, count, hits, errors, missing) = row.map_err(e)?;
        today.requests += count;
        today.limit_hits += hits;
        today.errors += errors;
        today.usage_missing += missing.unwrap_or(0);
        if !user.is_empty() {
            *today.by_user.entry(user.clone()).or_default() += count;
            today.by_device.insert((user.clone(), device), count);
            if hits > 0 {
                *limits.entry(user).or_default() += hits;
            }
        }
    }
    today.limit_hits_by_user = limits.into_iter().collect();
    today.limit_hits_by_user.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let durations: Vec<i64> = c
        .prepare(&format!("SELECT duration_ms FROM requests WHERE started_utc >= {MIDNIGHT} AND path = '/v1/messages' AND status = 200 ORDER BY duration_ms"))
        .map_err(e)?
        .query_map([], |r| r.get(0))
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)?;
    if !durations.is_empty() {
        let index = ((durations.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
        today.p95_ms = Some(durations[index.min(durations.len() - 1)]);
    }

    today.last = c
        .query_row("SELECT started_utc, COALESCE(user_name, '') FROM requests WHERE user_name IS NOT NULL ORDER BY id DESC LIMIT 1", [], |r| {
            Ok((parse_utc(&r.get::<_, String>(0)?).unwrap_or(0), r.get::<_, String>(1)?))
        })
        .ok();

    today.active = c
        .prepare(&format!(
            "SELECT user_name, device_name, MAX(started_utc >= {MIDNIGHT} AND status = 429) FROM requests
             WHERE user_name IS NOT NULL AND started_utc >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-10 minutes')
             GROUP BY user_name, device_name ORDER BY user_name, device_name"
        ))
        .map_err(e)?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)?;
    // A limit hit earlier today, not only in the last 10 minutes.
    for (user, _, hit) in &mut today.active {
        *hit = today.limit_hits_by_user.iter().any(|(u, _)| u == user);
    }
    Ok(today)
}

/// One person's traffic over a Dashboard period.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PersonTraffic {
    pub name: String,
    pub requests: i64,
    pub limit_hits: i64,
    pub p95_ms: Option<i64>,
}

/// The Dashboard's figures for today, or the last 7 or 14 days.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub days: u32,
    /// Requests per local hour (one day) or per local day: label and count, oldest first.
    pub buckets: Vec<(String, i64)>,
    /// The same slots one period earlier (yesterday's hours, or the days before), to compare.
    pub previous: Vec<i64>,
    pub people: Vec<PersonTraffic>,
    pub requests: i64,
    pub limit_hits: i64,
    pub errors: i64,
    /// Of complete /v1/messages replies.
    pub p50_ms: Option<i64>,
    pub p95_ms: Option<i64>,
    pub max_ms: Option<i64>,
}

fn percentile(sorted: &[i64], p: f64) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let index = ((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1);
    Some(sorted[index.min(sorted.len() - 1)])
}

/// The last `days` local days, today included. `offset` is local time minus UTC, in seconds.
pub fn summary(db: &Path, days: u32, offset: i64) -> Result<Option<Summary>, String> {
    if !db.exists() {
        return Ok(None);
    }
    let today = (crate::health::now() + offset).div_euclid(86_400);
    let first_day = today - days.max(1) as i64 + 1;
    // From one period earlier, for the comparison bars.
    let from = crate::health::format_utc((first_day - days.max(1) as i64) * 86_400 - offset);
    activity::track(
        format!("sqlite3 \"{}\" \"SELECT started_utc, user_name, status, duration_ms … FROM requests WHERE started_utc >= '{from}'\"", db.display()),
        || read_summary(db, days.max(1), first_day, &from, offset),
        |s| format!("{} requests", s.requests),
    )
    .map(Some)
}

fn read_summary(db: &Path, days: u32, first_day: i64, from: &str, offset: i64) -> Result<Summary, String> {
    let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let e = |e: rusqlite::Error| e.to_string();
    let mut statement = c
        .prepare(&format!(
            "SELECT started_utc, user_name, status, duration_ms, path = '/v1/messages' AND status = 200 AND proxy_error IS NULL, {IS_ERROR}
             FROM requests WHERE started_utc >= ?1"
        ))
        .map_err(e)?;
    let rows = statement
        .query_map([from], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, bool>(4)?, r.get::<_, bool>(5)?))
        })
        .map_err(e)?;

    let mut summary = Summary { days, ..Default::default() };
    let slots = if days == 1 { 24 } else { days as usize };
    let mut counts = vec![0i64; slots];
    let mut previous = vec![0i64; slots];
    let mut durations = Vec::new();
    let mut per_person: HashMap<String, (i64, i64, Vec<i64>)> = HashMap::new();
    for row in rows {
        let (started, user, status, ms, complete, error) = row.map_err(e)?;
        let local = parse_utc(&started).unwrap_or(0) + offset;
        let day = local.div_euclid(86_400);
        let earlier = day < first_day;
        let slot = if days == 1 { local.rem_euclid(86_400) / 3600 } else { day - first_day + if earlier { days as i64 } else { 0 } };
        let bucket = if earlier { &mut previous } else { &mut counts };
        if let Some(count) = bucket.get_mut(slot.max(0) as usize) {
            *count += 1;
        }
        // Everything else is about the period itself.
        if earlier {
            continue;
        }
        summary.requests += 1;
        summary.limit_hits += i64::from(status == 429);
        summary.errors += i64::from(error);
        if complete {
            durations.push(ms);
        }
        if let Some(user) = user {
            let person = per_person.entry(user).or_default();
            person.0 += 1;
            person.1 += i64::from(status == 429);
            if complete {
                person.2.push(ms);
            }
        }
    }
    summary.buckets = counts
        .into_iter()
        .enumerate()
        .map(|(i, n)| {
            let label = if days == 1 { format!("{i:02}") } else { format!("{:02}", crate::cost::civil(first_day + i as i64).2) };
            (label, n)
        })
        .collect();
    summary.previous = previous;
    durations.sort_unstable();
    summary.p50_ms = percentile(&durations, 0.5);
    summary.p95_ms = percentile(&durations, 0.95);
    summary.max_ms = durations.last().copied();
    summary.people = per_person
        .into_iter()
        .map(|(name, (requests, limit_hits, mut ms))| {
            ms.sort_unstable();
            PersonTraffic { name, requests, limit_hits, p95_ms: percentile(&ms, 0.95) }
        })
        .collect();
    summary.people.sort_by(|a, b| b.requests.cmp(&a.requests).then_with(|| a.name.cmp(&b.name)));
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{format_utc, now};
    use rusqlite::Connection;

    fn database(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("comptroller-traffic-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).unwrap();
        c.execute_batch(include_str!("../../contracts/schema.sql")).unwrap();
        c.execute("INSERT INTO schema_version (version) VALUES (?1)", [crate::SCHEMA_VERSION as i64]).unwrap();
        let at = |ago: i64| format_utc(now() - ago);
        let rows: [(i64, &str, &str, i64, i64, Option<&str>, Option<&str>); 6] = [
            (3600 * 48, "EXAMPLE\\goose", "LAPTOP-1", 200, 900, None, Some("parsed")),
            (300, "EXAMPLE\\goose", "LAPTOP-1", 200, 1000, None, Some("parsed")),
            (200, "EXAMPLE\\goose", "LAPTOP-1", 200, 3000, None, Some("missing")),
            (120, "EXAMPLE\\maverick", "DESK-2", 429, 40, None, None),
            (60, "EXAMPLE\\maverick", "DESK-2", 502, 30000, Some("RequestTimedOut"), None),
            (30, "EXAMPLE\\maverick", "DESK-2", 200, 50, Some("RequestCanceled"), Some("missing")),
        ];
        for (ago, user, device, status, ms, error, usage) in rows {
            c.execute(
                "INSERT INTO requests (started_utc, user_name, device_name, method, path, status, duration_ms, response_bytes, proxy_error, usage)
                 VALUES (?1, ?2, ?3, 'POST', '/v1/messages', ?4, ?5, 100, ?6, ?7)",
                rusqlite::params![at(ago), user, device, status, ms, error, usage],
            )
            .unwrap();
        }
        path
    }

    #[test]
    fn todays_figures() {
        let db = database("today");
        // Rows from two days ago aren't today; the others are, unless the test runs just after midnight.
        let today = today(&db).unwrap().unwrap();
        if today.requests < 5 {
            return;
        }
        assert_eq!(today.requests, 5);
        assert_eq!(today.limit_hits, 1);
        assert_eq!(today.limit_hits_by_user, vec![("EXAMPLE\\maverick".to_string(), 1)]);
        assert_eq!(today.errors, 1, "a timeout counts, a cancel doesn't");
        assert_eq!(today.usage_missing, 2);
        assert_eq!(today.by_user["EXAMPLE\\goose"], 2);
        assert_eq!(today.p95_ms, Some(3000));
        assert_eq!(today.last.as_ref().map(|l| l.1.as_str()), Some("EXAMPLE\\maverick"));
        assert_eq!(today.active.len(), 2);
        assert!(today.active.iter().any(|(u, _, hit)| u == "EXAMPLE\\maverick" && *hit));
    }

    #[test]
    fn a_summary_over_days() {
        let db = database("summary");
        let two_weeks = summary(&db, 14, 0).unwrap().unwrap();
        assert_eq!(two_weeks.requests, 6);
        assert_eq!(two_weeks.buckets.len(), 14);
        assert_eq!(two_weeks.buckets.iter().map(|b| b.1).sum::<i64>(), 6);
        assert_eq!(two_weeks.previous.iter().sum::<i64>(), 0);
        // The request from two days ago is in the previous period of a one-day summary.
        let one = summary(&db, 1, 0).unwrap().unwrap();
        assert_eq!(one.previous.len(), 24);
        let two = summary(&db, 2, 0).unwrap().unwrap();
        assert_eq!(two.requests + two.previous.iter().sum::<i64>(), 6);
        assert_eq!(two_weeks.limit_hits, 1);
        assert_eq!(two_weeks.errors, 1);
        assert_eq!(two_weeks.people[0].name, "EXAMPLE\\goose");
        assert_eq!((two_weeks.p50_ms, two_weeks.max_ms), (Some(1000), Some(3000)));
        let today = summary(&db, 1, 0).unwrap().unwrap();
        assert_eq!(today.buckets.len(), 24);
        assert!(today.requests <= 5);
    }

    #[test]
    fn the_feed_filters_by_person_and_errors() {
        let db = database("feed");
        let all = recent(&db, 100, None, false).unwrap().unwrap();
        assert_eq!(all.len(), 6);
        assert!(all[0].id > all[1].id, "newest first");
        assert_eq!(recent(&db, 100, Some("EXAMPLE\\goose"), false).unwrap().unwrap().len(), 3);
        let errors = recent(&db, 100, None, true).unwrap().unwrap();
        assert_eq!(errors.iter().map(|r| r.status).collect::<Vec<_>>(), vec![502, 429]);
        assert!(errors[0].is_error() && errors[1].limit_hit());
        assert!(recent(&db.with_extension("missing"), 10, None, false).unwrap().is_none());
    }
}
