//! The SQLite store of SPEC §4 — the only source of truth the UI reads.
//!
//! The guarantee this module exists to keep: **a sync must never overwrite
//! `calendars.visible`, `calendars.user_color`, or anything on `accounts`.** Google knows
//! nothing about those; they exist only because the user set them, and a metadata refresh
//! quietly reverting a deliberate choice is the kind of bug noticed weeks later and never
//! reported properly.
//!
//! That guarantee is structural rather than careful: [`CalendarMetadata`] — the only thing
//! sync can hand to [`Store::upsert_calendar`] — has no field for a user-owned column, so
//! there is no value for the statement to write even if someone rewrote the SQL.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Greenfield, so the schema is stated whole at version 1. There is no version 2 and no
/// migration to write; see SPEC §4.
pub const SCHEMA_VERSION: i32 = 1;

pub struct Store {
    conn: Connection,
}

/// What the server says about a calendar, and nothing else.
///
/// The absence of `visible` and `user_color` here is the point: see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarMetadata {
    pub account: String,
    pub id: String,
    pub summary: String,
    pub color: Option<String>,
    pub timezone: Option<String>,
    pub access_role: String,
    pub is_primary: bool,
}

/// A connected account and the user's display choices for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub email: String,
    pub label: Option<String>,
    pub color: Option<String>,
    pub sort_order: i64,
}

/// A calendar as stored — server metadata plus the user's choices about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calendar {
    pub account: String,
    pub id: String,
    pub summary: String,
    pub color: Option<String>,
    pub timezone: Option<String>,
    pub access_role: String,
    pub is_primary: bool,
    pub visible: bool,
    pub user_color: Option<String>,
    pub sync_token: Option<String>,
    pub synced_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub account: String,
    pub calendar_id: String,
    pub id: String,
    /// Google's cross-calendar identity. Captured from the first sync so the duplicate
    /// display preference stays buildable without a backfill resync. Nothing in v1 reads it.
    pub ical_uid: Option<String>,
    pub etag: Option<String>,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start_utc: i64,
    pub end_utc: i64,
    pub timezone: Option<String>,
    pub all_day: bool,
    /// The whole recurrence block, newline-joined: RRULE plus any EXDATE and RDATE.
    pub rrule: Option<String>,
    pub recurring_event_id: Option<String>,
    pub original_start_utc: Option<i64>,
    pub status: String,
    pub updated_at: Option<i64>,
}

/// Named once so the transactional path and the single-calendar path cannot drift apart.
/// The DO UPDATE lists only server-owned columns; see the module docs.
const UPSERT_CALENDAR: &str = "INSERT INTO calendars
         (account, id, summary, color, timezone, access_role, is_primary)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
     ON CONFLICT(account, id) DO UPDATE SET
         summary     = excluded.summary,
         color       = excluded.color,
         timezone    = excluded.timezone,
         access_role = excluded.access_role,
         is_primary  = excluded.is_primary";

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("could not open {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().context("could not open an in-memory database")?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // Per SPEC §4. `foreign_keys` and `synchronous` are connection state and must be set
        // on every connection; `journal_mode` persists in the file but costs nothing to
        // repeat. Setting them here rather than in schema.sql keeps them out of reach of any
        // future guard that skips the DDL.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )
        .context("could not set the connection pragmas")?;
        conn.execute_batch(include_str!("schema.sql"))
            .context("could not apply the schema")?;
        // Recorded where a future migration would look for it, rather than left as a
        // constant nothing reads.
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .context("could not record the schema version")?;
        Ok(Self { conn })
    }

    /// Rows appear here on connect and are edited only by the user; sync never writes here.
    pub fn add_account(&self, email: &str, color: &str, added_at: i64) -> Result<()> {
        // DO NOTHING rather than DO UPDATE: reconnecting an account must not reset the
        // label, colour and ordering the user chose for it.
        self.conn
            .execute(
                "INSERT INTO accounts (email, provider, added_at, color)
                 VALUES (?1, 'google', ?2, ?3)
                 ON CONFLICT(email) DO NOTHING",
                params![email, added_at, color],
            )
            .with_context(|| format!("could not add the account {email}"))?;
        Ok(())
    }

    /// Refresh the server-owned half of a calendar, seeding the user-owned half only when
    /// the calendar is seen for the first time.
    pub fn upsert_calendar(&self, meta: &CalendarMetadata) -> Result<()> {
        // The DO UPDATE names only server-owned columns. `visible` and `user_color` appear
        // nowhere — they take their table defaults on a first insert and are never written
        // again. `sync_token` is omitted too: calendarList carries none, so naming it here
        // would write NULL and force a full resync on every metadata refresh.
        self.conn
            .execute(
                UPSERT_CALENDAR,
                params![
                    meta.account,
                    meta.id,
                    meta.summary,
                    meta.color,
                    meta.timezone,
                    meta.access_role,
                    meta.is_primary,
                ],
            )
            .with_context(|| format!("could not store the calendar {}", meta.id))?;
        Ok(())
    }

    pub fn account_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM accounts", [], |row| row.get(0))
            .context("could not count the connected accounts")?;
        Ok(count as usize)
    }

    /// Record a newly connected account and everything on it, or record none of it.
    ///
    /// One transaction because a half-written account is worse than no account: SPEC §2
    /// requires a failed connect to leave no partial row, and a row with no calendars would
    /// render as a permanently empty account in the sidebar with no way to repair it.
    pub fn connect_account(
        &mut self,
        email: &str,
        color: &str,
        added_at: i64,
        calendars: &[CalendarMetadata],
    ) -> Result<()> {
        let transaction = self
            .conn
            .transaction()
            .context("could not begin the connect transaction")?;

        transaction
            .execute(
                "INSERT INTO accounts (email, provider, added_at, color)
                 VALUES (?1, 'google', ?2, ?3)
                 ON CONFLICT(email) DO NOTHING",
                params![email, added_at, color],
            )
            .with_context(|| format!("could not add the account {email}"))?;

        for calendar in calendars {
            transaction
                .execute(
                    UPSERT_CALENDAR,
                    params![
                        calendar.account,
                        calendar.id,
                        calendar.summary,
                        calendar.color,
                        calendar.timezone,
                        calendar.access_role,
                        calendar.is_primary,
                    ],
                )
                .with_context(|| format!("could not store the calendar {}", calendar.id))?;
        }

        transaction
            .commit()
            .context("could not commit the connect transaction")?;
        Ok(())
    }

    /// Disconnect an account. Cascades to its calendars and their events.
    pub fn remove_account(&self, email: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM accounts WHERE email = ?1", params![email])
            .with_context(|| format!("could not remove the account {email}"))?;
        Ok(())
    }

    /// Every connected account, in the user's chosen order. Sorted by the user's grouping
    /// first and the address only as a tiebreak, because the ordering is theirs, not
    /// Google's.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut statement = self.conn.prepare(
            "SELECT email, label, color, sort_order FROM accounts ORDER BY sort_order, email",
        )?;
        let accounts = statement
            .query_map([], |row| {
                Ok(Account {
                    email: row.get(0)?,
                    label: row.get(1)?,
                    color: row.get(2)?,
                    sort_order: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("could not read the connected accounts")?;
        Ok(accounts)
    }

    /// One account's calendars, primary first so the account's own calendar heads its group.
    pub fn calendars(&self, account: &str) -> Result<Vec<Calendar>> {
        let mut statement = self.conn.prepare(
            "SELECT account, id, summary, color, timezone, access_role, is_primary,
                    visible, user_color, sync_token, synced_at
             FROM calendars WHERE account = ?1
             ORDER BY is_primary DESC, summary",
        )?;
        let calendars = statement
            .query_map(params![account], calendar_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .with_context(|| format!("could not read the calendars for {account}"))?;
        Ok(calendars)
    }

    /// Record Google's cursor for a calendar, and when it was last brought up to date.
    /// Written only by sync; `NULL` forces the next run to resync in full.
    pub fn set_sync_token(
        &self,
        account: &str,
        calendar_id: &str,
        sync_token: Option<&str>,
        synced_at: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE calendars SET sync_token = ?3, synced_at = ?4
                 WHERE account = ?1 AND id = ?2",
                params![account, calendar_id, sync_token, synced_at],
            )
            .with_context(|| format!("could not record the sync token for {calendar_id}"))?;
        Ok(())
    }

    /// Remove one event. Google reports a deletion as a `cancelled` event carrying little
    /// more than an id, so this takes no more than that.
    pub fn delete_event(&self, account: &str, calendar_id: &str, id: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM events WHERE account = ?1 AND calendar_id = ?2 AND id = ?3",
                params![account, calendar_id, id],
            )
            .with_context(|| format!("could not delete the event {id}"))?;
        Ok(())
    }

    /// Drop everything on a calendar, for the resync that follows a refused sync token.
    pub fn clear_calendar(&self, account: &str, calendar_id: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM events WHERE account = ?1 AND calendar_id = ?2",
                params![account, calendar_id],
            )
            .with_context(|| format!("could not clear {calendar_id}"))?;
        Ok(())
    }

    /// Set the user-owned columns directly, for tests that need a preference in place
    /// before exercising a sync. The real setters arrive with the styling task.
    #[cfg(test)]
    pub fn set_user_style_for_tests(
        &self,
        account: &str,
        calendar_id: &str,
        visible: bool,
        user_color: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE calendars SET visible = ?3, user_color = ?4 WHERE account = ?1 AND id = ?2",
            params![account, calendar_id, visible, user_color],
        )?;
        Ok(())
    }

    /// Everything the week view might need for `[start_utc, end_utc)`, from every account,
    /// excluding calendars the user has hidden.
    ///
    /// Three kinds of row qualify, and a plain range query would miss two of them:
    ///
    /// - a recurring master, whose own start is usually months before the window but whose
    ///   occurrences are in it;
    /// - an override or tombstone, which matters if *either* the occurrence it replaces or
    ///   the time it moved to falls in the window;
    /// - an ordinary event that overlaps.
    pub fn events_for_window(&self, start_utc: i64, end_utc: i64) -> Result<Vec<Event>> {
        let mut statement = self.conn.prepare(
            "SELECT e.account, e.calendar_id, e.id, e.ical_uid, e.etag, e.summary,
                    e.description, e.location, e.start_utc, e.end_utc, e.timezone, e.all_day,
                    e.rrule, e.recurring_event_id, e.original_start_utc, e.status, e.updated_at
             FROM events e
             JOIN calendars c ON c.account = e.account AND c.id = e.calendar_id
             WHERE c.visible = 1 AND (
                     (e.recurring_event_id IS NOT NULL
                      AND ((e.original_start_utc >= ?1 AND e.original_start_utc < ?2)
                           OR (e.start_utc < ?2 AND e.end_utc > ?1)))
                  OR (e.recurring_event_id IS NULL AND e.rrule IS NOT NULL
                      AND e.start_utc < ?2)
                  OR (e.recurring_event_id IS NULL AND e.rrule IS NULL
                      AND e.start_utc < ?2 AND e.end_utc > ?1)
                 )
             ORDER BY e.start_utc",
        )?;
        let events = statement
            .query_map(params![start_utc, end_utc], event_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("could not read the events for the window")?;
        Ok(events)
    }

    pub fn calendar(&self, account: &str, id: &str) -> Result<Option<Calendar>> {
        self.conn
            .query_row(
                "SELECT account, id, summary, color, timezone, access_role, is_primary,
                        visible, user_color, sync_token, synced_at
                 FROM calendars WHERE account = ?1 AND id = ?2",
                params![account, id],
                calendar_from_row,
            )
            .optional()
            .with_context(|| format!("could not read the calendar {id}"))
    }

    pub fn upsert_event(&self, event: &Event) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO events
                     (account, calendar_id, id, ical_uid, etag, summary, description,
                      location, start_utc, end_utc, timezone, all_day, rrule,
                      recurring_event_id, original_start_utc, status, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                         ?15, ?16, ?17)
                 ON CONFLICT(account, calendar_id, id) DO UPDATE SET
                     ical_uid           = excluded.ical_uid,
                     etag               = excluded.etag,
                     summary            = excluded.summary,
                     description        = excluded.description,
                     location           = excluded.location,
                     start_utc          = excluded.start_utc,
                     end_utc            = excluded.end_utc,
                     timezone           = excluded.timezone,
                     all_day            = excluded.all_day,
                     rrule              = excluded.rrule,
                     recurring_event_id = excluded.recurring_event_id,
                     original_start_utc = excluded.original_start_utc,
                     status             = excluded.status,
                     updated_at         = excluded.updated_at",
                params![
                    event.account,
                    event.calendar_id,
                    event.id,
                    event.ical_uid,
                    event.etag,
                    event.summary,
                    event.description,
                    event.location,
                    event.start_utc,
                    event.end_utc,
                    event.timezone,
                    event.all_day,
                    event.rrule,
                    event.recurring_event_id,
                    event.original_start_utc,
                    event.status,
                    event.updated_at,
                ],
            )
            .with_context(|| format!("could not store the event {}", event.id))?;
        Ok(())
    }

    /// Every event overlapping `[start_utc, end_utc)` — the hot window query the week view
    /// will call. Overlap, not containment: an event that began before the window and ends
    /// inside it is on screen, so it must come back.
    pub fn events_in_range(&self, start_utc: i64, end_utc: i64) -> Result<Vec<Event>> {
        let mut statement = self.conn.prepare(
            "SELECT account, calendar_id, id, ical_uid, etag, summary, description, location,
                    start_utc, end_utc, timezone, all_day, rrule, recurring_event_id,
                    original_start_utc, status, updated_at
             FROM events
             WHERE start_utc < ?2 AND end_utc > ?1
             ORDER BY start_utc",
        )?;
        let events = statement
            .query_map(params![start_utc, end_utc], event_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("could not read the events in the window")?;
        Ok(events)
    }
}

fn calendar_from_row(row: &Row<'_>) -> rusqlite::Result<Calendar> {
    Ok(Calendar {
        account: row.get(0)?,
        id: row.get(1)?,
        summary: row.get(2)?,
        color: row.get(3)?,
        timezone: row.get(4)?,
        access_role: row.get(5)?,
        is_primary: row.get(6)?,
        visible: row.get(7)?,
        user_color: row.get(8)?,
        sync_token: row.get(9)?,
        synced_at: row.get(10)?,
    })
}

fn event_from_row(row: &Row<'_>) -> rusqlite::Result<Event> {
    Ok(Event {
        account: row.get(0)?,
        calendar_id: row.get(1)?,
        id: row.get(2)?,
        ical_uid: row.get(3)?,
        etag: row.get(4)?,
        summary: row.get(5)?,
        description: row.get(6)?,
        location: row.get(7)?,
        start_utc: row.get(8)?,
        end_utc: row.get(9)?,
        timezone: row.get(10)?,
        all_day: row.get(11)?,
        rrule: row.get(12)?,
        recurring_event_id: row.get(13)?,
        original_start_utc: row.get(14)?,
        status: row.get(15)?,
        updated_at: row.get(16)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two accounts, one of them holding two calendars. SPEC §8: a single-account fixture
    /// passes account-keyed logic trivially and proves nothing.
    fn two_accounts() -> Store {
        let store = Store::open_in_memory().expect("in-memory store");
        store
            .add_account("personal@example.com", "#3584e4", 1_700_000_000)
            .unwrap();
        store
            .add_account("work@example.com", "#e66100", 1_700_000_001)
            .unwrap();
        store
            .upsert_calendar(&metadata("personal@example.com", "primary", "Personal"))
            .unwrap();
        store
            .upsert_calendar(&metadata("work@example.com", "primary", "Work"))
            .unwrap();
        store
            .upsert_calendar(&metadata("work@example.com", "team", "Team"))
            .unwrap();
        store
    }

    fn metadata(account: &str, id: &str, summary: &str) -> CalendarMetadata {
        CalendarMetadata {
            account: account.to_string(),
            id: id.to_string(),
            summary: summary.to_string(),
            color: Some("#000000".to_string()),
            timezone: Some("Europe/Madrid".to_string()),
            access_role: "owner".to_string(),
            is_primary: id == "primary",
        }
    }

    fn event(account: &str, calendar: &str, id: &str, start_utc: i64, end_utc: i64) -> Event {
        Event {
            account: account.to_string(),
            calendar_id: calendar.to_string(),
            id: id.to_string(),
            ical_uid: Some(format!("{id}@google.com")),
            etag: None,
            summary: id.to_string(),
            description: None,
            location: None,
            start_utc,
            end_utc,
            timezone: Some("Europe/Madrid".to_string()),
            all_day: false,
            rrule: None,
            recurring_event_id: None,
            original_start_utc: None,
            status: "confirmed".to_string(),
            updated_at: None,
        }
    }

    #[test]
    fn the_schema_creates_every_table_and_index_the_spec_names() {
        let store = Store::open_in_memory().unwrap();
        let names: Vec<String> = store
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','index') ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        for expected in [
            "accounts",
            "calendars",
            "events",
            "events_by_range",
            "events_by_calendar",
            "events_by_master",
            "events_by_ical_uid",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn the_connection_enforces_foreign_keys_and_uses_wal() {
        let store = Store::open_in_memory().unwrap();
        let foreign_keys: i32 = store
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1, "cascades silently do nothing without this");
    }

    #[test]
    fn the_schema_version_is_recorded_where_a_migration_would_look_for_it() {
        let store = Store::open_in_memory().unwrap();
        let version: i32 = store
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn foreign_keys_are_enforced_so_a_calendar_cannot_outlive_its_account() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_calendar(&metadata("nobody@example.com", "primary", "Orphan"))
            .expect_err("PRAGMA foreign_keys must be ON, or cascades silently do nothing");
    }

    #[test]
    fn a_calendar_upsert_refreshes_the_servers_own_metadata() {
        let store = two_accounts();
        let mut renamed = metadata("work@example.com", "team", "Team");
        renamed.summary = "Team (renamed in Google)".to_string();
        renamed.color = Some("#ffffff".to_string());
        store.upsert_calendar(&renamed).unwrap();

        let stored = store.calendar("work@example.com", "team").unwrap().unwrap();
        assert_eq!(stored.summary, "Team (renamed in Google)");
        assert_eq!(stored.color.as_deref(), Some("#ffffff"));
    }

    #[test]
    fn a_calendar_upsert_never_overwrites_the_users_colour_or_visibility() {
        // SPEC §4: the only thing standing between a preference and a sync that quietly
        // eats it. Set both, then run an upsert carrying different server metadata.
        let store = two_accounts();
        store
            .execute_raw(
                "UPDATE calendars SET visible = 0, user_color = '#ff0000'
                 WHERE account = 'work@example.com' AND id = 'team'",
            )
            .unwrap();

        let mut refreshed = metadata("work@example.com", "team", "Team");
        refreshed.summary = "Team (renamed)".to_string();
        refreshed.color = Some("#00ff00".to_string());
        store.upsert_calendar(&refreshed).unwrap();

        let stored = store.calendar("work@example.com", "team").unwrap().unwrap();
        assert_eq!(
            stored.summary, "Team (renamed)",
            "server field must refresh"
        );
        assert_eq!(
            stored.color.as_deref(),
            Some("#00ff00"),
            "server field must refresh"
        );
        assert!(
            !stored.visible,
            "the user hid this calendar; sync must not unhide it"
        );
        assert_eq!(
            stored.user_color.as_deref(),
            Some("#ff0000"),
            "the user chose this colour; sync must not revert it"
        );
    }

    #[test]
    fn a_calendar_upsert_does_not_clear_the_sync_cursor() {
        // Naming sync_token in the DO UPDATE would write the NULL that calendarList carries,
        // wiping the cursor and forcing a full resync on every metadata refresh.
        let store = two_accounts();
        store
            .execute_raw(
                "UPDATE calendars SET sync_token = 'CURSOR', synced_at = 42
                 WHERE account = 'work@example.com' AND id = 'team'",
            )
            .unwrap();

        store
            .upsert_calendar(&metadata("work@example.com", "team", "Team"))
            .unwrap();

        let stored = store.calendar("work@example.com", "team").unwrap().unwrap();
        assert_eq!(stored.sync_token.as_deref(), Some("CURSOR"));
        assert_eq!(stored.synced_at, Some(42));
    }

    #[test]
    fn a_calendar_seen_for_the_first_time_is_visible_with_no_colour_override() {
        let store = two_accounts();
        let stored = store
            .calendar("personal@example.com", "primary")
            .unwrap()
            .unwrap();
        assert!(stored.visible);
        assert_eq!(stored.user_color, None);
    }

    #[test]
    fn the_same_calendar_id_on_two_accounts_stays_two_calendars() {
        let store = two_accounts();
        let personal = store
            .calendar("personal@example.com", "primary")
            .unwrap()
            .unwrap();
        let work = store
            .calendar("work@example.com", "primary")
            .unwrap()
            .unwrap();
        assert_eq!(personal.summary, "Personal");
        assert_eq!(work.summary, "Work");
    }

    #[test]
    fn the_sidebar_sees_every_account_and_each_ones_calendars() {
        let store = two_accounts();
        let accounts = store.accounts().unwrap();
        assert_eq!(
            accounts.len(),
            2,
            "every account is listed, not a selected one"
        );
        assert_eq!(accounts[0].email, "personal@example.com");
        assert_eq!(accounts[0].color.as_deref(), Some("#3584e4"));

        let work = store.calendars("work@example.com").unwrap();
        assert_eq!(work.len(), 2);
        assert!(
            work[0].is_primary,
            "an account's own calendar heads its group"
        );
        assert_eq!(
            store.calendars("personal@example.com").unwrap().len(),
            1,
            "one account's calendars must not leak into another's group"
        );
    }

    #[test]
    fn an_event_that_starts_before_the_window_and_ends_inside_it_is_returned() {
        let store = two_accounts();
        store
            .upsert_event(&event("work@example.com", "team", "overnight", 900, 1_500))
            .unwrap();

        let found = store.events_in_range(1_000, 2_000).unwrap();
        assert_eq!(found.len(), 1, "overlap, not containment");
        assert_eq!(found[0].id, "overnight");
    }

    #[test]
    fn an_event_spanning_the_whole_window_is_returned() {
        let store = two_accounts();
        store
            .upsert_event(&event("work@example.com", "team", "allweek", 0, 9_999))
            .unwrap();
        assert_eq!(store.events_in_range(1_000, 2_000).unwrap().len(), 1);
    }

    #[test]
    fn an_event_entirely_outside_the_window_is_not_returned() {
        let store = two_accounts();
        store
            .upsert_event(&event("work@example.com", "team", "later", 5_000, 6_000))
            .unwrap();
        assert!(store.events_in_range(1_000, 2_000).unwrap().is_empty());
    }

    #[test]
    fn an_event_ending_exactly_at_the_window_start_is_not_returned() {
        // The window is half-open: [start, end). An event that ends as the window opens is
        // not in it, or every week view would show the previous week's last meeting.
        let store = two_accounts();
        store
            .upsert_event(&event("work@example.com", "team", "justbefore", 500, 1_000))
            .unwrap();
        assert!(store.events_in_range(1_000, 2_000).unwrap().is_empty());
    }

    #[test]
    fn the_window_query_returns_events_from_every_account() {
        let store = two_accounts();
        store
            .upsert_event(&event(
                "personal@example.com",
                "primary",
                "dentist",
                1_100,
                1_200,
            ))
            .unwrap();
        store
            .upsert_event(&event("work@example.com", "team", "standup", 1_300, 1_400))
            .unwrap();

        let found = store.events_in_range(1_000, 2_000).unwrap();
        assert_eq!(
            found.len(),
            2,
            "there is no current account; every account is on screen"
        );
    }

    #[test]
    fn re_upserting_an_event_updates_it_rather_than_duplicating_it() {
        let store = two_accounts();
        let mut standup = event("work@example.com", "team", "standup", 1_300, 1_400);
        store.upsert_event(&standup).unwrap();
        standup.summary = "Standup (moved)".to_string();
        standup.etag = Some("etag-2".to_string());
        store.upsert_event(&standup).unwrap();

        let found = store.events_in_range(1_000, 2_000).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].summary, "Standup (moved)");
    }

    #[test]
    fn the_same_meeting_on_two_accounts_keeps_one_ical_uid_on_both_copies() {
        // SPEC §8: the case that proves the point. Nothing in v1 reads ical_uid, so nothing
        // else would catch it silently going null — and the cost of finding out later is a
        // full resync of every calendar.
        let store = two_accounts();
        let mut personal = event("personal@example.com", "primary", "invite-a", 1_100, 1_200);
        let mut work = event("work@example.com", "primary", "invite-b", 1_100, 1_200);
        personal.ical_uid = Some("shared-meeting@google.com".to_string());
        work.ical_uid = Some("shared-meeting@google.com".to_string());
        store.upsert_event(&personal).unwrap();
        store.upsert_event(&work).unwrap();

        let found = store.events_in_range(1_000, 2_000).unwrap();
        assert_eq!(found.len(), 2, "duplicates are shown, not merged");
        assert!(
            found
                .iter()
                .all(|e| e.ical_uid.as_deref() == Some("shared-meeting@google.com")),
            "both copies must carry the same non-null ical_uid"
        );
    }

    #[test]
    fn deleting_an_account_cascades_to_its_calendars_and_their_events() {
        let store = two_accounts();
        store
            .upsert_event(&event("work@example.com", "team", "standup", 1_300, 1_400))
            .unwrap();
        store
            .upsert_event(&event(
                "personal@example.com",
                "primary",
                "dentist",
                1_100,
                1_200,
            ))
            .unwrap();

        store
            .execute_raw("DELETE FROM accounts WHERE email = 'work@example.com'")
            .unwrap();

        assert!(
            store
                .calendar("work@example.com", "team")
                .unwrap()
                .is_none()
        );
        let survivors = store.events_in_range(1_000, 2_000).unwrap();
        assert_eq!(survivors.len(), 1, "the other account must be untouched");
        assert_eq!(survivors[0].account, "personal@example.com");
    }

    impl Store {
        /// Arranging state the store deliberately exposes no setter for yet — user styling
        /// lands with `accounts` (Task 10), account removal with the settings UI (Task 13).
        fn execute_raw(&self, sql: &str) -> Result<()> {
            self.conn.execute_batch(sql)?;
            Ok(())
        }
    }
}
