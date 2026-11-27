//! Users, their devices and one-time codes, read from and changed in the service's
//! database (contracts/schema.sql). The service checks a device on every request, so a
//! change here applies to the next request, with no restart.
//!
//! Each change goes to the Commands panel as the `sqlite3` command that does the same.

use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::activity;
use crate::health::{format_utc, now, parse_utc};

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: i64,
    pub name: String,
    /// `active`, `suspended` or `revoked`.
    pub status: String,
    /// `tray` when the developer disconnected it, `admin` when an admin suspended it.
    pub suspended_by: Option<String>,
    pub created: i64,
    pub last_seen: Option<i64>,
    pub expires: i64,
}

impl Device {
    pub fn expired(&self) -> bool {
        self.expires <= now()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub id: i64,
    pub name: String,
    /// `windows` or `manual`.
    pub source: String,
    pub revoked: bool,
    pub created: i64,
    pub devices: Vec<Device>,
}

impl User {
    pub fn last_seen(&self) -> Option<i64> {
        self.devices.iter().filter_map(|d| d.last_seen).max()
    }
}

/// A one-time code not used yet. Only its hash is stored, so the code itself can't be shown again.
#[derive(Clone, Debug, PartialEq)]
pub struct Code {
    pub hash: String,
    pub user: String,
    pub device: String,
    pub created: i64,
    pub expires: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Directory {
    pub users: Vec<User>,
    pub codes: Vec<Code>,
}

fn sql_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn open(db: &Path, flags: OpenFlags) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(db, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| match e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::CannotOpen => {
            format!("Can't open {}. It needs an administrator terminal.", db.display())
        }
        other => other.to_string(),
    })?;
    connection.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    connection.pragma_update(None, "foreign_keys", true).map_err(|e| e.to_string())?;
    let version: Option<i64> = connection.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if version != Some(crate::SCHEMA_VERSION as i64) {
        return Err(format!(
            "The database is schema version {}, but this terminal knows version {}. Update both together.",
            version.map(|v| v.to_string()).unwrap_or("none".into()),
            crate::SCHEMA_VERSION
        ));
    }
    Ok(connection)
}

fn time(text: Option<String>) -> Option<i64> {
    text.as_deref().and_then(parse_utc)
}

/// Everyone, newest activity first. `None` when the service hasn't created the database yet.
pub fn load(db: &Path) -> Result<Option<Directory>, String> {
    if !db.exists() {
        return Ok(None);
    }
    activity::track(
        format!("sqlite3 \"{}\" \"SELECT * FROM users; SELECT * FROM devices; SELECT * FROM enrollment_codes\"", db.display()),
        || read(db).map(Some),
        |d: &Option<Directory>| {
            let d = d.as_ref().expect("read returns Some");
            let devices: usize = d.users.iter().map(|u| u.devices.len()).sum();
            format!("{} users · {devices} devices", d.users.len())
        },
    )
}

fn read(db: &Path) -> Result<Directory, String> {
    let c = open(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let e = |e: rusqlite::Error| e.to_string();

    let mut users: Vec<User> = c
        .prepare("SELECT id, name, source, status, created_utc FROM users")
        .map_err(e)?
        .query_map([], |r| {
            Ok(User {
                id: r.get(0)?,
                name: r.get(1)?,
                source: r.get(2)?,
                revoked: r.get::<_, String>(3)? == "revoked",
                created: time(r.get(4)?).unwrap_or(0),
                devices: Vec::new(),
            })
        })
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)?;

    let devices = c
        .prepare("SELECT user_id, id, name, status, suspended_by, created_utc, last_seen_utc, expires_utc FROM devices ORDER BY name")
        .map_err(e)?
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Device {
                    id: r.get(1)?,
                    name: r.get(2)?,
                    status: r.get(3)?,
                    suspended_by: r.get(4)?,
                    created: time(r.get(5)?).unwrap_or(0),
                    last_seen: time(r.get(6)?),
                    expires: time(r.get(7)?).unwrap_or(0),
                },
            ))
        })
        .map_err(e)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(e)?;
    for (user_id, device) in devices {
        if let Some(user) = users.iter_mut().find(|u| u.id == user_id) {
            user.devices.push(device);
        }
    }
    // Most recently seen first; never-seen users by name.
    users.sort_by(|a, b| b.last_seen().cmp(&a.last_seen()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));

    let codes = c
        .prepare(
            "SELECT c.code_hash, u.name, c.device_name, c.created_utc, c.expires_utc FROM enrollment_codes c JOIN users u ON u.id = c.user_id
             WHERE c.used_utc IS NULL AND c.expires_utc > ?1 ORDER BY c.created_utc",
        )
        .map_err(e)?
        .query_map([format_utc(now())], |r| {
            Ok(Code {
                hash: r.get(0)?,
                user: r.get(1)?,
                device: r.get(2)?,
                created: time(r.get(3)?).unwrap_or(0),
                expires: time(r.get(4)?).unwrap_or(0),
            })
        })
        .map_err(e)?
        .collect::<Result<_, _>>()
        .map_err(e)?;

    Ok(Directory { users, codes })
}

/// Runs one change, shown in the Commands panel as `sql` with its values filled in.
fn change(db: &Path, shown: String, run: impl FnOnce(&Connection) -> rusqlite::Result<usize>) -> Result<usize, String> {
    activity::track(
        format!("sqlite3 \"{}\" \"{shown}\"", db.display()),
        || {
            let c = open(db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            run(&c).map_err(|e| e.to_string())
        },
        |n| format!("{n} row{} changed", if *n == 1 { "" } else { "s" }),
    )
}

/// Stops one computer until an admin resumes it. Requests from it get a clear refusal.
pub fn suspend_device(db: &Path, id: i64) -> Result<usize, String> {
    change(db, format!("UPDATE devices SET status = 'suspended', suspended_by = 'admin' WHERE id = {id} AND status <> 'revoked'"), |c| {
        c.execute("UPDATE devices SET status = 'suspended', suspended_by = 'admin' WHERE id = ?1 AND status <> 'revoked'", [id])
    })
}

/// Lifts an admin suspension. A developer's own disconnect is theirs to undo from the tray.
pub fn resume_device(db: &Path, id: i64) -> Result<usize, String> {
    change(db, format!("UPDATE devices SET status = 'active', suspended_by = NULL WHERE id = {id} AND suspended_by = 'admin'"), |c| {
        c.execute("UPDATE devices SET status = 'active', suspended_by = NULL WHERE id = ?1 AND status = 'suspended' AND suspended_by = 'admin'", [id])
    })
}

/// Permanent: this computer can't connect again under this name until it's removed.
pub fn revoke_device(db: &Path, id: i64) -> Result<usize, String> {
    change(db, format!("UPDATE devices SET status = 'revoked' WHERE id = {id}"), |c| {
        c.execute("UPDATE devices SET status = 'revoked' WHERE id = ?1", [id])
    })
}

/// Forgets the device. Its token stops working, and the computer can connect again as new.
pub fn remove_device(db: &Path, id: i64) -> Result<usize, String> {
    change(db, format!("DELETE FROM devices WHERE id = {id}"), |c| c.execute("DELETE FROM devices WHERE id = ?1", [id]))
}

/// Revokes or restores a person on every computer at once.
pub fn set_user_revoked(db: &Path, id: i64, revoked: bool) -> Result<usize, String> {
    let status = if revoked { "revoked" } else { "active" };
    change(db, format!("UPDATE users SET status = '{status}' WHERE id = {id}"), |c| {
        c.execute("UPDATE users SET status = ?1 WHERE id = ?2", params![status, id])
    })
}

/// Forgets a person, their devices and codes. In Windows mode they reappear if they connect again.
pub fn remove_user(db: &Path, id: i64) -> Result<usize, String> {
    change(db, format!("DELETE FROM users WHERE id = {id}"), |c| c.execute("DELETE FROM users WHERE id = ?1", [id]))
}

pub fn cancel_code(db: &Path, hash: &str) -> Result<usize, String> {
    change(db, format!("DELETE FROM enrollment_codes WHERE code_hash = '{}…'", &hash[..hash.len().min(12)]), |c| {
        c.execute("DELETE FROM enrollment_codes WHERE code_hash = ?1", [hash])
    })
}

// ── one-time codes ───────────────────────────────────────────────────────────

/// No 0/O, 1/I/L: codes get read aloud and retyped. The same alphabet as the service.
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

fn new_code() -> Result<String, String> {
    let mut chars = Vec::with_capacity(12);
    while chars.len() < 12 {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        // Rejects the top of the byte range so every character is equally likely.
        let limit = 256 - 256 % CODE_ALPHABET.len();
        chars.extend(bytes.iter().filter(|&&b| (b as usize) < limit).map(|&b| CODE_ALPHABET[b as usize % CODE_ALPHABET.len()] as char));
    }
    chars.truncate(12);
    let s: String = chars.into_iter().collect();
    Ok(format!("{}-{}-{}", &s[..4], &s[4..8], &s[8..]))
}

fn hex_sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// How the service stores a code: SHA-256 of the code in capitals without dashes or spaces.
pub fn code_hash(code: &str) -> String {
    hex_sha256(&code.replace(['-', ' '], "").to_uppercase())
}

pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control)
}

/// Creates the user if needed and a code that connects `device` for them, once.
pub fn create_code(db: &Path, user: &str, device: &str, lifetime_hours: i64) -> Result<String, String> {
    let (user, device) = (user.trim(), device.trim());
    if !valid_name(user) || !valid_name(device) {
        return Err("Names must be 1 to 64 printable characters.".into());
    }
    let code = new_code()?;
    let created = now();
    let (created_utc, expires_utc) = (format_utc(created), format_utc(created + lifetime_hours * 3600));
    let shown = format!(
        "INSERT OR IGNORE INTO users (name, source, created_utc) VALUES ({}, 'manual', '{created_utc}'); INSERT INTO enrollment_codes … expires {expires_utc}",
        sql_text(user)
    );
    change(db, shown, |c| {
        c.execute("INSERT INTO users (name, source, created_utc) VALUES (?1, 'manual', ?2) ON CONFLICT (name) DO NOTHING", params![user, created_utc])?;
        let (user_id, status): (i64, String) = c.query_row("SELECT id, status FROM users WHERE name = ?1", [user], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if status == "revoked" {
            return Err(rusqlite::Error::InvalidParameterName(format!("{user} is revoked. Restore them first.")));
        }
        let revoked_device: Option<String> =
            c.query_row("SELECT status FROM devices WHERE user_id = ?1 AND name = ?2", params![user_id, device], |r| r.get(0)).optional()?;
        if revoked_device.as_deref() == Some("revoked") {
            return Err(rusqlite::Error::InvalidParameterName(format!("{device} is revoked for {user}. Remove it first to connect it again.")));
        }
        c.execute(
            "INSERT INTO enrollment_codes (code_hash, user_id, device_name, created_utc, expires_utc) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![code_hash(&code), user_id, device, created_utc, expires_utc],
        )
    })
    .map_err(|e| e.trim_start_matches("Invalid parameter name: ").to_string())?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database built from the shipped schema, as the service creates it.
    fn database(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("comptroller-directory-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).unwrap();
        c.execute_batch(include_str!("../../contracts/schema.sql")).unwrap();
        c.execute("INSERT INTO schema_version (version) VALUES (?1)", [crate::SCHEMA_VERSION as i64]).unwrap();
        c.execute_batch(
            "INSERT INTO users (id, name, source, created_utc) VALUES (1, 'EXAMPLE\\jdoe', 'windows', '2026-01-01T00:00:00.000Z');
             INSERT INTO devices (id, user_id, name, token_hash, status, suspended_by, created_utc, last_seen_utc, expires_utc) VALUES
               (1, 1, 'LAPTOP-1', 'aa', 'active', NULL, '2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z', '2099-01-01T00:00:00.000Z'),
               (2, 1, 'DESKTOP-2', 'bb', 'suspended', 'tray', '2026-01-01T00:00:00.000Z', NULL, '2099-01-01T00:00:00.000Z');",
        )
        .unwrap();
        path
    }

    #[test]
    fn codes_match_the_service() {
        // Tokens.HashCode in the service: SHA-256 of the code in capitals, no dashes or spaces.
        assert_eq!(code_hash("abcd-2345-wxyz"), hex_sha256("ABCD2345WXYZ"));
        assert_eq!(hex_sha256("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let code = new_code().unwrap();
        assert_eq!(code.len(), 14);
        assert!(code.replace('-', "").bytes().all(|b| CODE_ALPHABET.contains(&b)));
    }

    #[test]
    fn reads_users_with_their_devices() {
        let db = database("read");
        let d = load(&db).unwrap().unwrap();
        assert_eq!(d.users.len(), 1);
        assert_eq!(d.users[0].devices.len(), 2);
        assert_eq!(d.users[0].devices[0].name, "DESKTOP-2");
        assert_eq!(d.users[0].devices[0].suspended_by.as_deref(), Some("tray"));
        assert!(load(&db.with_extension("missing")).unwrap().is_none());
    }

    #[test]
    fn an_admin_lifts_only_their_own_suspension() {
        let db = database("suspend");
        assert_eq!(resume_device(&db, 2).unwrap(), 0, "the developer's own disconnect stays");
        suspend_device(&db, 1).unwrap();
        let device = |id| load(&db).unwrap().unwrap().users[0].devices.iter().find(|d| d.id == id).unwrap().clone();
        assert_eq!((device(1).status.as_str(), device(1).suspended_by.as_deref()), ("suspended", Some("admin")));
        resume_device(&db, 1).unwrap();
        assert_eq!(device(1).status, "active");
        revoke_device(&db, 1).unwrap();
        assert_eq!(suspend_device(&db, 1).unwrap(), 0, "revoked stays revoked");
        remove_device(&db, 1).unwrap();
        assert_eq!(load(&db).unwrap().unwrap().users[0].devices.len(), 1);
    }

    #[test]
    fn a_code_creates_the_user_and_is_listed_until_cancelled() {
        let db = database("code");
        let code = create_code(&db, "Pat", "LAPTOP-9", 24).unwrap();
        let d = load(&db).unwrap().unwrap();
        assert!(d.users.iter().any(|u| u.name == "Pat" && u.source == "manual"));
        assert_eq!(d.codes.len(), 1);
        assert_eq!(d.codes[0].hash, code_hash(&code));
        cancel_code(&db, &d.codes[0].hash).unwrap();
        assert!(load(&db).unwrap().unwrap().codes.is_empty());

        set_user_revoked(&db, 1, true).unwrap();
        assert!(create_code(&db, "EXAMPLE\\jdoe", "X", 24).unwrap_err().contains("revoked"));
    }
}
