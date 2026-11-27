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
    created_utc TEXT NOT NULL,
    -- The seat plan they're on, by name from Cost.SeatPlans in comptroller.json (schema version 3).
    -- The admin side sets it. Seat cost is counted from created_utc.
    seat_plan   TEXT
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

-- One row per proxied request (schema version 2). Metadata only: never a prompt or a reply.
-- The service writes; the admin side reads.
CREATE TABLE IF NOT EXISTS requests (
    id                    INTEGER PRIMARY KEY,
    started_utc           TEXT NOT NULL,
    user_name             TEXT,               -- as signed in at the time; NULL for the hello probe
    device_name           TEXT,
    client_ip             TEXT,
    method                TEXT NOT NULL,
    path                  TEXT NOT NULL,      -- without the query string
    status                INTEGER NOT NULL,
    duration_ms           INTEGER NOT NULL,
    request_bytes         INTEGER,            -- NULL when the client didn't say
    response_bytes        INTEGER NOT NULL,
    -- Why the proxy got no complete reply, as YARP names it (RequestTimedOut, RequestCanceled, …).
    proxy_error           TEXT,
    model                 TEXT,
    input_tokens          INTEGER,
    output_tokens         INTEGER,
    cache_write_tokens    INTEGER,            -- cache_creation_input_tokens
    cache_write_5m_tokens INTEGER,            -- split by cache lifetime, when the reply gives it
    cache_write_1h_tokens INTEGER,
    cache_read_tokens     INTEGER,
    -- 'parsed', or 'missing' when a reply should have had usage but it couldn't be read.
    -- NULL when none was expected (not POST /v1/messages, or not a 200).
    usage                 TEXT CHECK (usage IN ('parsed', 'missing'))
);
CREATE INDEX IF NOT EXISTS requests_by_time ON requests (started_utc);
CREATE INDEX IF NOT EXISTS requests_by_device ON requests (user_name, device_name, started_utc);

-- One-time codes for manual identity mode. The admin side inserts, the service redeems.
CREATE TABLE IF NOT EXISTS enrollment_codes (
    code_hash   TEXT PRIMARY KEY,                               -- SHA-256 of the code, upper-case, no dashes
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_name TEXT NOT NULL,
    created_utc TEXT NOT NULL,
    expires_utc TEXT NOT NULL,
    used_utc    TEXT
);
