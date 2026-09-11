-- SCHEMA_VERSION = 1. The schema of SPEC §4, stated whole: the repository is greenfield, so
-- multi-account and user styling are native from the first line and there is no migration.
--
-- DDL only. The pragmas of §4 are connection state, not database state, and are set in
-- Store::init — here they would be skipped by any future "schema already applied" guard,
-- silently turning foreign keys off.

-- Connected accounts, and the user's display preferences for each. Sync never writes
-- here: rows appear on connect and are edited only by the user.
CREATE TABLE IF NOT EXISTS accounts (
    email       TEXT PRIMARY KEY,
    provider    TEXT NOT NULL DEFAULT 'google',
    added_at    INTEGER NOT NULL,
    -- Short human label: "work" reads better in a sidebar than a 30-character address.
    label       TEXT,
    -- The account's marker colour, and the default fill for its calendars. Assigned from
    -- a palette on connect, so a newly added account is never unmarked.
    color       TEXT,
    -- Sidebar ordering: the user's grouping, not Google's.
    sort_order  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS calendars (
    account     TEXT NOT NULL REFERENCES accounts(email) ON DELETE CASCADE,
    id          TEXT NOT NULL,
    -- Server-owned: refreshed on every sync.
    summary     TEXT NOT NULL,
    color       TEXT,                     -- Google's colour for this calendar
    timezone    TEXT,
    access_role TEXT NOT NULL DEFAULT 'reader',
    is_primary  INTEGER NOT NULL DEFAULT 0,
    -- User-owned: sync must never overwrite these two.
    visible     INTEGER NOT NULL DEFAULT 1,
    user_color  TEXT,                     -- NULL means "use Google's"
    -- Google's opaque incremental cursor; NULL forces a full resync.
    sync_token  TEXT,
    synced_at   INTEGER,
    PRIMARY KEY (account, id)
);

CREATE TABLE IF NOT EXISTS events (
    account       TEXT NOT NULL,
    calendar_id   TEXT NOT NULL,
    id            TEXT NOT NULL,
    -- Google's cross-calendar identity. The same meeting invited to two of the user's
    -- addresses carries the SAME value on both copies, which is the only thing that makes
    -- the duplicate case detectable. Nothing in v1 reads it.
    ical_uid      TEXT,
    etag          TEXT,
    summary       TEXT NOT NULL DEFAULT '',
    description   TEXT,
    location      TEXT,
    -- UTC unix seconds; for all-day events, midnight in the event's own timezone.
    start_utc     INTEGER NOT NULL,
    end_utc       INTEGER NOT NULL,
    timezone      TEXT,
    all_day       INTEGER NOT NULL DEFAULT 0,
    -- The WHOLE recurrence block, newline-joined: RRULE plus any EXDATE and RDATE.
    -- Dropping EXDATE resurrects deleted occurrences; dropping RDATE loses added ones.
    rrule         TEXT,
    -- A modified single instance points back at its master, and records where it would
    -- have started, so expansion knows which occurrence it replaces.
    recurring_event_id TEXT,
    original_start_utc INTEGER,
    status        TEXT NOT NULL DEFAULT 'confirmed',
    -- The server's `updated` stamp when sync supplies one, our write time otherwise.
    updated_at    INTEGER,
    PRIMARY KEY (account, calendar_id, id),
    FOREIGN KEY (account, calendar_id)
        REFERENCES calendars(account, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS events_by_range    ON events (start_utc, end_utc);
CREATE INDEX IF NOT EXISTS events_by_calendar ON events (account, calendar_id);
CREATE INDEX IF NOT EXISTS events_by_master   ON events (recurring_event_id);
CREATE INDEX IF NOT EXISTS events_by_ical_uid ON events (ical_uid);
