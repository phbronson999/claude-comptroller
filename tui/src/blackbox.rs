//! The black box: the rolling store of recorded requests. Per-day counts and an estimate of
//! each day's share of the file, and pruning by hand with the same rule the service uses each
//! night: keep today and the `retention_days - 1` days before it, by the local calendar.

use std::path::Path;

use rusqlite::OpenFlags;

use crate::activity;
use crate::directory::open;
use crate::health::{format_utc, parse_utc};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlackBox {
    /// Local day number and requests that day, newest first.
    pub days: Vec<(i64, i64)>,
    pub rows: i64,
    pub oldest: Option<i64>,
    pub newest: Option<i64>,
    /// The database file and its write-ahead log.
    pub size: u64,
}

impl BlackBox {
    /// A day's share of the file, by its share of the rows. Rows are much the same size.
    pub fn estimate(&self, requests: i64) -> u64 {
        if self.rows == 0 {
            0
        } else {
            (self.size as f64 * requests as f64 / self.rows as f64) as u64
        }
    }
}

fn size(db: &Path) -> u64 {
    let length = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    length(db) + length(&db.with_extension("db-wal"))
}

/// The first local day kept, as a UTC instant in the stored format.
pub fn cutoff(today: i64, retention_days: i64, offset: i64) -> String {
    format_utc((today - retention_days.max(1) + 1) * 86_400 - offset)
}

pub fn read(db: &Path, offset: i64) -> Result<Option<BlackBox>, String> {
    if !db.exists() {
        return Ok(None);
    }
    activity::track(
        format!("sqlite3 \"{}\" \"SELECT local day, COUNT(*) FROM requests GROUP BY 1\"", db.display()),
        || -> Result<BlackBox, String> {
            let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let e = |e: rusqlite::Error| e.to_string();
            // Grouped by UTC hour in SQL, then into local days here: SQLite doesn't know the offset.
            let mut statement = c.prepare("SELECT substr(started_utc, 1, 13), COUNT(*) FROM requests GROUP BY 1").map_err(e)?;
            let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map_err(e)?;
            let mut days: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
            let mut total = 0;
            for row in rows {
                let (hour, count) = row.map_err(e)?;
                if let Some(t) = parse_utc(&format!("{hour}:00:00")) {
                    *days.entry((t + offset).div_euclid(86_400)).or_default() += count;
                    total += count;
                }
            }
            let (oldest, newest) = c
                .query_row("SELECT MIN(started_utc), MAX(started_utc) FROM requests", [], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))
                .map_err(e)?;
            Ok(BlackBox {
                days: days.into_iter().rev().collect(),
                rows: total,
                oldest: oldest.as_deref().and_then(parse_utc),
                newest: newest.as_deref().and_then(parse_utc),
                size: size(db),
            })
        },
        |b| format!("{} rows · {} days", b.rows, b.days.len()),
    )
    .map(Some)
}

/// How many requests a prune now would remove.
pub fn prunable(db: &Path, today: i64, retention_days: i64, offset: i64) -> Result<i64, String> {
    let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.query_row("SELECT COUNT(*) FROM requests WHERE started_utc < ?1", [cutoff(today, retention_days, offset)], |r| r.get(0))
        .map_err(|e| e.to_string())
}

/// Removes requests older than the retention and gives the space back. Returns the rows
/// removed and the bytes freed.
pub fn prune(db: &Path, today: i64, retention_days: i64, offset: i64) -> Result<(i64, u64), String> {
    let cut = cutoff(today, retention_days, offset);
    let before = size(db);
    let removed = activity::track(
        format!("sqlite3 \"{}\" \"DELETE FROM requests WHERE started_utc < '{cut}'; PRAGMA incremental_vacuum\"", db.display()),
        || -> Result<i64, String> {
            let c = open(db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            let e = |e: rusqlite::Error| e.to_string();
            let mut removed = 0;
            // In batches, so the service's own writes never wait long.
            loop {
                let n = c
                    .execute("DELETE FROM requests WHERE id IN (SELECT id FROM requests WHERE started_utc < ?1 LIMIT 5000)", [&cut])
                    .map_err(e)?;
                removed += n as i64;
                if n < 5000 {
                    break;
                }
            }
            c.execute("DELETE FROM enrollment_codes WHERE expires_utc < ?1", [&cut]).map_err(e)?;
            c.execute_batch("PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);").map_err(e)?;
            Ok(removed)
        },
        |n| format!("{n} rows removed"),
    )?;
    Ok((removed, before.saturating_sub(size(db))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::now;
    use rusqlite::Connection;

    #[test]
    fn per_day_counts_and_pruning_by_the_local_calendar() {
        let db = std::env::temp_dir().join(format!("comptroller-blackbox-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let c = Connection::open(&db).unwrap();
        c.execute_batch(include_str!("../../contracts/schema.sql")).unwrap();
        c.execute("INSERT INTO schema_version (version) VALUES (?1)", [crate::SCHEMA_VERSION as i64]).unwrap();
        let today = now().div_euclid(86_400);
        // Noon UTC on each of these days, with no offset: two days ago, 20 days ago, 30 days ago.
        for (days_ago, count) in [(2, 3), (20, 2), (30, 1)] {
            for _ in 0..count {
                c.execute(
                    "INSERT INTO requests (started_utc, method, path, status, duration_ms, response_bytes) VALUES (?1, 'GET', '/v1/models', 200, 1, 1)",
                    [format_utc((today - days_ago) * 86_400 + 43_200)],
                )
                .unwrap();
            }
        }
        drop(c);

        let b = read(&db, 0).unwrap().unwrap();
        assert_eq!(b.rows, 6);
        assert_eq!(b.days, vec![(today - 2, 3), (today - 20, 2), (today - 30, 1)]);
        assert!(b.size > 0 && b.estimate(3) > b.estimate(1));

        assert_eq!(prunable(&db, today, 14, 0).unwrap(), 3);
        let (removed, _) = prune(&db, today, 14, 0).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(read(&db, 0).unwrap().unwrap().days, vec![(today - 2, 3)]);
        // Keeping 3 days keeps today and the two before it.
        assert_eq!(prunable(&db, today, 3, 0).unwrap(), 0);
        assert_eq!(prunable(&db, today, 2, 0).unwrap(), 3);
    }
}
