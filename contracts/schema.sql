-- claude-comptroller database. The service creates it and owns schema_version.
-- All times are UTC text: yyyy-MM-ddTHH:mm:ss.fffZ

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS users (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,            -- DOMAIN\user in windows mode
    source      TEXT NOT NULL CHECK (source IN ('windows', 'manual')),
    status      TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
    created_utc TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS devices (
    id            INTEGER PRIMARY KEY,
    user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name          TEXT NOT NULL COLLATE NOCASE,
    token_hash    TEXT UNIQUE,                                  -- lower-case hex SHA-256; never the token
    status        TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended', 'revoked')),
    -- Who suspended it. A user can only reconnect from their own ('tray') suspension.
    suspended_by  TEXT CHECK (suspended_by IN ('tray', 'admin')),
    created_utc   TEXT NOT NULL,
    last_seen_utc TEXT,
    expires_utc   TEXT NOT NULL,
    UNIQUE (user_id, name)
);

-- One-time codes for manual identity mode. The admin side inserts, the service redeems.
CREATE TABLE IF NOT EXISTS enrollment_codes (
    code_hash   TEXT PRIMARY KEY,                               -- SHA-256 of the code, upper-case, no dashes
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_name TEXT NOT NULL,
    created_utc TEXT NOT NULL,
    expires_utc TEXT NOT NULL,
    used_utc    TEXT
);
