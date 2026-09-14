#!/usr/bin/env python3
"""Build a throwaway fixture calendar database for eyeballing the views.

Not test data for `cargo test` — those fixtures live in `tests/`. This is a database to
*look at*: four accounts, fourteen calendars and a week engineered to contain every case
the grid, month and agenda views are specified to survive (SPEC §2.2, §2.7,
SPEC-views-timegrid "The vertical mapping", SPEC-views-month, SPEC-views-agenda).

    scripts/make-fixture.py [FIXTURE_XDG_DATA_HOME]
    XDG_DATA_HOME=<that dir> ./target/release/agenda

The default target is a scratch directory, never `~/.local/share`: this script drops and
rebuilds the file it is pointed at, and the real store must not be within reach of that.
Re-runnable by construction — the database is deleted and recreated, never appended to.

The schema is read from `src/store/schema.sql` rather than restated here, so the fixture
cannot drift from the shape the app expects and force a migration on open.
"""

import os
import sqlite3
import sys
from datetime import datetime, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo

REPO = Path(__file__).resolve().parent.parent
SCHEMA = REPO / "src" / "store" / "schema.sql"
DEFAULT_DATA_HOME = Path(
    "/tmp/claude-1000/-home-gui-workspace-personal-agenda"
    "/87bdbd77-0fc5-41b9-ba62-974f0750fd44/scratchpad/fixture"
)

MADRID = ZoneInfo("Europe/Madrid")
TODAY = "2026-09-14"  # a Monday; everything is anchored to this week so it is visible.
NOW = int(datetime(2026, 9, 14, 9, 0, tzinfo=MADRID).timestamp())


def ts(local: str, zone: str = "Europe/Madrid", fold: int = 0) -> int:
    """Local wall time in `zone` as UTC seconds.

    `fold` picks which side of an ambiguous local time is meant, which is the whole point
    of the 2026-10-25 02:30 case: fold=0 is the first pass (CEST), fold=1 the second (CET).
    """
    naive = datetime.fromisoformat(local)
    return int(naive.replace(tzinfo=ZoneInfo(zone), fold=fold).timestamp())


def day(date: str) -> int:
    """Midnight Madrid — how the schema stores an all-day boundary."""
    return ts(f"{date} 00:00")


def plus_days(date: str, n: int) -> str:
    return (datetime.fromisoformat(date) + timedelta(days=n)).date().isoformat()


# ---------------------------------------------------------------------------- accounts

ACCOUNTS = [
    # email, label, display name, colour, sort order
    ("work@example.com", "work", "Guilherme (work)", "#e66100", 0),
    ("personal@example.com", "personal", "Guilherme Storck", "#3584e4", 1),
    ("side@example.com", "side", "G. Storck (side)", "#33d17a", 2),
    ("family@example.com", "family", "Casa Storck", "#9141ac", 3),
]

CALENDARS = [
    # account, id, summary, google colour, tz, access role, primary, user colour
    ("work@example.com", "primary", "Guilherme (work)", "#e66100", "Europe/Madrid", "owner", 1, None),
    ("work@example.com", "team-platform", "Team platform", "#c64600", "Europe/Madrid", "writer", 0, None),
    ("work@example.com", "oncall", "On-call rota", "#b5835a", "UTC", "reader", 0, "#f66151"),
    ("work@example.com", "travel", "Travel", "#865e3c", "Europe/Madrid", "writer", 0, None),
    ("personal@example.com", "primary", "Personal", "#3584e4", "Europe/Madrid", "owner", 1, None),
    ("personal@example.com", "health", "Health", "#1a5fb4", "Europe/Madrid", "owner", 0, None),
    ("personal@example.com", "birthdays", "Birthdays", "#62a0ea", "Europe/Madrid", "reader", 0, None),
    ("personal@example.com", "holidays", "Holidays in Spain", "#99c1f1", "Europe/Madrid", "reader", 0, None),
    ("side@example.com", "primary", "Side projects", "#33d17a", "Europe/Madrid", "owner", 1, None),
    ("side@example.com", "oss", "OSS releases", "#26a269", "UTC", "owner", 0, None),
    ("side@example.com", "clients", "Clients", "#8ff0a4", "America/New_York", "writer", 0, "#2ec27e"),
    ("family@example.com", "primary", "Family", "#9141ac", "Europe/Madrid", "owner", 1, None),
    ("family@example.com", "school", "School", "#c061cb", "Europe/Madrid", "reader", 0, None),
    ("family@example.com", "trips", "Trips", "#dc8add", "Europe/Madrid", "writer", 0, None),
]

# ------------------------------------------------------------------------------ events

events: list[dict] = []


def ev(
    account,
    calendar,
    eid,
    summary,
    start,
    end,
    *,
    all_day=0,
    tz="Europe/Madrid",
    uid=None,
    rrule=None,
    master=None,
    original=None,
    status="confirmed",
    location=None,
    reminder=None,
):
    events.append(
        {
            "account": account,
            "calendar_id": calendar,
            "id": eid,
            "ical_uid": uid or f"{eid}@example.com",
            "etag": f'"{abs(hash(eid)) % 10**12}"',
            "summary": summary,
            "description": None,
            "location": location,
            "start_utc": start,
            "end_utc": end,
            "timezone": tz,
            "all_day": all_day,
            "rrule": rrule,
            "recurring_event_id": master,
            "original_start_utc": original,
            "status": status,
            "reminder_minutes": reminder,
            "updated_at": NOW - 86_400,
        }
    )


def timed(account, calendar, eid, summary, date, start_hm, end_hm, **kw):
    """A same-day timed event given as local wall clock."""
    zone = kw.get("tz", "Europe/Madrid")
    ev(
        account,
        calendar,
        eid,
        summary,
        ts(f"{date} {start_hm}", zone),
        ts(f"{date} {end_hm}", zone),
        **kw,
    )


W = "work@example.com"
P = "personal@example.com"
S = "side@example.com"
F = "family@example.com"

MON, TUE, WED = "2026-09-14", "2026-09-15", "2026-09-16"
THU, FRI, SAT, SUN = "2026-09-17", "2026-09-18", "2026-09-19", "2026-09-20"

# --- Monday: the quiet-hours / core-boundary set. --------------------------------------
# These are the cases the piecewise vertical mapping gets wrong if a height is computed as
# duration x scale instead of y(end) - y(start): each one straddles a row-height change.
timed(P, "health", "q-dawn", "Early run", MON, "06:00", "06:45")
timed(P, "primary", "q-0700", "语言 class", MON, "07:00", "07:30", tz="Europe/Madrid")
timed(W, "travel", "q-cross-core-open", "Airport transfer to T4", MON, "07:00", "09:00",
      location="Madrid-Barajas T4")
timed(W, "team-platform", "q-cross-core-close", "Release freeze window", MON, "21:00", "23:00")
timed(W, "oncall", "q-2300", "Night log review", MON, "23:00", "23:45")
timed(W, "primary", "mon-core-1", "Sprint planning", MON, "10:00", "11:30")
timed(F, "school", "mon-core-2", "Pick-up", MON, "16:30", "17:00")

# --- Tuesday: overlaps (2 and 3 concurrent) and the very short events. -----------------
timed(W, "primary", "ov2-a", "Design review", TUE, "10:00", "11:00")
timed(P, "primary", "ov2-b", "Dentist", TUE, "10:30", "11:30")
timed(W, "team-platform", "ov3-a", "Incident postmortem", TUE, "15:00", "16:00")
timed(S, "clients", "ov3-b", "Client call — Acme", TUE, "15:15", "16:15")
timed(F, "primary", "ov3-c", "Piano lesson", TUE, "15:30", "16:30")
timed(W, "primary", "short-5", "Deploy ack", TUE, "09:05", "09:10")
timed(P, "primary", "short-15", "Coffee sync", TUE, "12:00", "12:15")

# --- Wednesday: five concurrent, long titles, a standalone tombstone. ------------------
for n, (acct, cal, start, end) in enumerate(
    [
        (W, "primary", "11:00", "12:30"),
        (W, "team-platform", "11:10", "12:00"),
        (P, "primary", "11:15", "12:15"),
        (S, "primary", "11:20", "12:40"),
        (F, "primary", "11:30", "12:10"),
    ],
    start=1,
):
    timed(acct, cal, f"ov5-{n}", f"Concurrent block {n}", WED, start, end)

timed(
    W,
    "primary",
    "long-title",
    "Quarterly cross-functional platform architecture review and roadmap "
    "alignment working session (part 2 of 3, continued from Tuesday)",
    WED,
    "14:00",
    "15:00",
)
timed(S, "primary", "emoji-title", "🎉 Offsite retro 🍕 — bring ideas ✨", WED, "17:00", "18:00")
timed(W, "primary", "cancelled-standalone", "Vendor call (cancelled)", WED, "13:00", "13:30",
      status="cancelled")
# All-day, single.
ev(P, "holidays", "allday-single", "Fiesta local", day(WED), day(THU), all_day=1)

# --- Thursday: duplicates across accounts (SPEC §2.7). --------------------------------
# The same ical_uid on two accounts, and another on three. Each copy must render, carrying
# its own account's marker; nothing in v1 dedupes them.
for acct, cal in [(W, "primary"), (P, "primary")]:
    timed(acct, cal, f"dup2-{acct.split('@')[0]}", "Quarterly business review", THU,
          "14:00", "15:00", uid="dup-quarterly-2026q3@example.com")
for acct, cal in [(W, "primary"), (P, "primary"), (S, "primary")]:
    timed(acct, cal, f"dup3-{acct.split('@')[0]}", "All-hands", THU, "16:00", "17:00",
          uid="dup-allhands-2026-09@example.com")
# All-day, three-day span (end exclusive).
ev(W, "travel", "allday-span3", "DevConf Barcelona", day(THU), day(SUN), all_day=1)

# --- Friday: midnight spanning. -------------------------------------------------------
ev(W, "team-platform", "midnight-short", "Release window",
   ts(f"{FRI} 23:00"), ts(f"{SAT} 01:00"))
ev(S, "oss", "midnight-long", "Overnight rebuild of the package index",
   ts(f"{SAT} 22:00"), ts(f"{SUN} 06:00"))
timed(P, "primary", "fri-dinner", "Dinner with Marta", FRI, "21:00", "23:30")

# --- Sunday: four all-day events at once. ---------------------------------------------
for n, (acct, cal, name) in enumerate(
    [
        (P, "birthdays", "Ana's birthday"),
        (P, "holidays", "Día de la Comunidad"),
        (F, "trips", "Sierra weekend"),
        (W, "oncall", "On-call: Guilherme"),
    ],
    start=1,
):
    ev(acct, cal, f"allday-stack-{n}", name, day(SUN), day(plus_days(SUN, 1)), all_day=1)

# --- Recurrence: masters, an EXDATE, a moved instance, a tombstone. -------------------
ev(W, "team-platform", "rec-standup", "Daily standup",
   ts("2026-09-07 09:15"), ts("2026-09-07 09:30"),
   rrule="RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;UNTIL=20261231T220000Z", reminder=5)
ev(W, "primary", "rec-one-on-one", "1:1 with Elena",
   ts("2026-09-01 11:00"), ts("2026-09-01 11:30"),
   rrule="RRULE:FREQ=WEEKLY;BYDAY=TU;UNTIL=20261231T220000Z\n"
         "EXDATE;TZID=Europe/Madrid:20260922T110000")
ev(P, "health", "rec-gym", "Gym",
   ts("2026-09-02 07:00"), ts("2026-09-02 08:00"),
   rrule="RRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261231T220000Z")
# A recurring series pinned to a zone that is not the display zone: its wall time must hold
# in Tokyo and therefore drift on screen across the Madrid transition.
ev(S, "oss", "rec-tokyo", "Release sync (Tokyo)",
   ts("2026-09-03 18:00", "Asia/Tokyo"), ts("2026-09-03 18:30", "Asia/Tokyo"),
   tz="Asia/Tokyo", rrule="RRULE:FREQ=WEEKLY;BYDAY=TH;UNTIL=20261231T220000Z")

# A single modified instance: Thursday's standup moved later, keyed to what the rule would
# have produced.
ev(W, "team-platform", "rec-standup_20260917T071500Z", "Daily standup (moved)",
   ts(f"{THU} 09:45"), ts(f"{THU} 10:15"),
   master="rec-standup", original=ts(f"{THU} 09:15"))
# A tombstone: Friday's standup deleted. Must suppress the occurrence and render nothing.
ev(W, "team-platform", "rec-standup_20260918T071500Z", "Daily standup",
   ts(f"{FRI} 09:15"), ts(f"{FRI} 09:30"),
   master="rec-standup", original=ts(f"{FRI} 09:15"), status="cancelled")

# --- Timezones that are not the display zone. -----------------------------------------
timed(S, "clients", "tz-ny", "Client call — New York", THU, "09:30", "10:15",
      tz="America/New_York")
timed(S, "oss", "tz-utc", "CI window (UTC)", WED, "08:00", "09:00", tz="UTC")
timed(S, "clients", "tz-sp", "Client call — São Paulo", FRI, "10:00", "10:45",
      tz="America/Sao_Paulo")

# --- DST: the Madrid autumn transition week (2026-10-25, 03:00 CEST -> 02:00 CET). -----
DST_SAT, DST_SUN = "2026-10-24", "2026-10-25"
# Straddles the transition: 22:00 CEST (20:00Z) to 02:00 CET on the second pass (01:00Z),
# so it is five hours long despite reading as four on the clock.
ev(W, "oncall", "dst-straddle", "On-call handover (spans the change)",
   ts(f"{DST_SAT} 22:00"), ts(f"{DST_SUN} 02:00", fold=1))
# The ambiguous hour itself: 02:30 happens twice. Both are stored.
timed(W, "oncall", "dst-0230-first", "Batch run 02:30 (CEST)", DST_SUN, "02:30", "03:00")
ev(W, "oncall", "dst-0230-second", "Batch run 02:30 (CET)",
   ts(f"{DST_SUN} 02:30", fold=1), ts(f"{DST_SUN} 03:00", fold=1))
timed(P, "primary", "dst-morning", "Brunch", DST_SUN, "11:00", "13:00")
timed(W, "primary", "dst-monday", "Post-transition planning", "2026-10-26", "09:00", "10:00")
ev(F, "trips", "dst-allday", "Clocks go back", day(DST_SUN), day("2026-10-26"), all_day=1)

# --- Month density: several days with 5+ events, across 2026-09 and 2026-10. -----------
# Deterministic, not random, so a re-run produces byte-identical data.
DENSE_DAYS = [
    "2026-09-08", "2026-09-09", "2026-09-21", "2026-09-22", "2026-09-28", "2026-09-29",
    "2026-10-01", "2026-10-06", "2026-10-07", "2026-10-13", "2026-10-14", "2026-10-20",
]
SLOTS = [("08:30", "09:00"), ("10:00", "10:45"), ("12:30", "13:00"), ("14:00", "14:30"),
         ("16:00", "17:00"), ("18:30", "19:00"), ("20:00", "20:30")]
FILL = [(W, "primary"), (W, "team-platform"), (P, "primary"), (S, "primary"),
        (F, "primary"), (P, "health"), (F, "school")]
for d, date in enumerate(DENSE_DAYS):
    for n in range(5 + d % 3):  # 5, 6 or 7 events on the day
        acct, cal = FILL[(d + n) % len(FILL)]
        start, end = SLOTS[n]
        timed(acct, cal, f"fill-{date}-{n}", f"Block {n + 1}", date, start, end)

# Two deliberately empty days in the current month: 2026-09-26 and 2026-09-27, a weekend,
# so none of the weekday recurrences reach them either. Asserted below, not assumed.
EMPTY_DAYS = ["2026-09-26", "2026-09-27"]


# ------------------------------------------------------------------------------- build


def build(data_home: Path) -> Path:
    db = data_home / "agenda" / "agenda.db"
    db.parent.mkdir(parents=True, exist_ok=True)
    for suffix in ("", "-wal", "-shm"):
        Path(str(db) + suffix).unlink(missing_ok=True)

    conn = sqlite3.connect(db)
    conn.executescript("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
    conn.executescript(SCHEMA.read_text())
    conn.execute("PRAGMA user_version = 1")  # SCHEMA_VERSION in src/store/mod.rs

    conn.executemany(
        "INSERT INTO accounts (email, provider, added_at, display_name, picture_url,"
        " label, color, sort_order) VALUES (?, 'google', ?, ?, NULL, ?, ?, ?)",
        [(email, NOW - 30 * 86_400, name, label, color, order)
         for email, label, name, color, order in ACCOUNTS],
    )
    conn.executemany(
        "INSERT INTO calendars (account, id, summary, color, timezone, access_role,"
        " is_primary, visible, user_color, sync_token, synced_at)"
        " VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, 'fixture-token', ?)",
        [(a, i, s, c, tz, role, prim, uc, NOW)
         for a, i, s, c, tz, role, prim, uc in CALENDARS],
    )
    columns = list(events[0])
    conn.executemany(
        f"INSERT INTO events ({', '.join(columns)})"
        f" VALUES ({', '.join('?' * len(columns))})",
        [tuple(e[c] for c in columns) for e in events],
    )
    conn.commit()
    return db, conn


def report(conn: sqlite3.Connection) -> None:
    """Prove each edge case is present, by query rather than by eye."""

    def one(sql, *args):
        return conn.execute(sql, args).fetchone()[0]

    def between(date, start_hm, end_hm):
        return ts(f"{date} {start_hm}"), ts(f"{date} {end_hm}")

    checks = [
        ("accounts", one("SELECT count(*) FROM accounts"), 4),
        ("calendars", one("SELECT count(*) FROM calendars"), 14),
        ("events (rows)", one("SELECT count(*) FROM events"), None),
        ("2 concurrent (Tue 10:00-11:30)",
         one("SELECT count(*) FROM events WHERE start_utc < ? AND end_utc > ? AND all_day=0",
             *reversed(between(TUE, "10:00", "11:30"))), 2),
        ("3 concurrent (Tue 15:30-16:00)",
         one("SELECT count(*) FROM events WHERE start_utc < ? AND end_utc > ? AND all_day=0",
             *reversed(between(TUE, "15:30", "16:00"))), 3),
        ("5 concurrent (Wed 11:30-12:00)",
         one("SELECT count(*) FROM events WHERE start_utc < ? AND end_utc > ? AND all_day=0",
             *reversed(between(WED, "11:30", "12:00"))), 5),
        ("duplicate ical_uid on 2 accounts",
         one("SELECT count(DISTINCT account) FROM events"
             " WHERE ical_uid = 'dup-quarterly-2026q3@example.com'"), 2),
        ("duplicate ical_uid on 3 accounts",
         one("SELECT count(DISTINCT account) FROM events"
             " WHERE ical_uid = 'dup-allhands-2026-09@example.com'"), 3),
        ("midnight-spanning events",
         one("SELECT count(*) FROM events WHERE all_day = 0 AND rrule IS NULL"
             " AND date(start_utc,'unixepoch','localtime')"
             "  <> date(end_utc - 1,'unixepoch','localtime')"), 3),  # + the DST straddle
        ("quiet-hour events (start < 08:00 or >= 22:00 local)",
         one("SELECT count(*) FROM events WHERE all_day = 0 AND ("
             " cast(strftime('%H', start_utc,'unixepoch','localtime') AS int) < 8"
             " OR cast(strftime('%H', start_utc,'unixepoch','localtime') AS int) >= 22)"), None),
        ("crossing the 08:00 core boundary",
         one("SELECT count(*) FROM events WHERE all_day = 0"
             " AND start_utc < ? AND end_utc > ?", ts(f"{MON} 08:00"), ts(f"{MON} 08:00")), None),
        ("crossing the 22:00 core boundary",
         one("SELECT count(*) FROM events WHERE all_day = 0"
             " AND start_utc < ? AND end_utc > ?", ts(f"{MON} 22:00"), ts(f"{MON} 22:00")), None),
        ("all-day: single-day", one("SELECT count(*) FROM events WHERE all_day=1"
                                    " AND end_utc - start_utc = 86400"), None),
        ("all-day: 3-day span", one("SELECT count(*) FROM events WHERE all_day=1"
                                    " AND end_utc - start_utc = 3*86400"), 1),
        ("all-day: 4 stacked on Sun 2026-09-20",
         one("SELECT count(*) FROM events WHERE all_day=1 AND start_utc < ? AND end_utc > ?",
             day(plus_days(SUN, 1)), day(SUN)), 4),
        ("5-minute event", one("SELECT count(*) FROM events WHERE end_utc-start_utc = 300"), 1),
        ("15-minute event", one("SELECT count(*) FROM events WHERE end_utc-start_utc = 900"
                                " AND rrule IS NULL AND recurring_event_id IS NULL"), 1),
        ("titles over 60 chars", one("SELECT count(*) FROM events WHERE length(summary) > 60"), None),
        ("titles with emoji", one("SELECT count(*) FROM events WHERE summary LIKE '%🎉%'"
                                  " OR summary LIKE '%语%'"), 2),
        ("recurring masters", one("SELECT count(*) FROM events WHERE rrule IS NOT NULL"), 4),
        ("masters carrying EXDATE",
         one("SELECT count(*) FROM events WHERE rrule LIKE '%EXDATE%'"), 1),
        ("modified instances (recurring_event_id)",
         one("SELECT count(*) FROM events WHERE recurring_event_id IS NOT NULL"), 2),
        ("cancelled tombstones", one("SELECT count(*) FROM events WHERE status='cancelled'"), 2),
        ("DST week 2026-10-19..26 events",
         one("SELECT count(*) FROM events WHERE start_utc >= ? AND start_utc < ?",
             day("2026-10-19"), day("2026-10-26")), None),
        ("DST 02:30 local on 2026-10-25",
         one("SELECT count(*) FROM events WHERE"
             " strftime('%Y-%m-%d %H:%M', start_utc,'unixepoch','localtime')"
             " = '2026-10-25 02:30'"), 2),
        ("timezone <> Europe/Madrid",
         one("SELECT count(*) FROM events WHERE timezone <> 'Europe/Madrid'"), None),
    ]

    print(f"fixture: {conn.execute('PRAGMA database_list').fetchone()[2]}")
    print(f"user_version: {conn.execute('PRAGMA user_version').fetchone()[0]}\n")
    failed = 0
    for name, got, want in checks:
        ok = "   " if want is None else ("ok " if got == want else "BAD")
        failed += want is not None and got != want
        print(f"  {ok} {name:<44} {got}")

    print("\n  days in 2026-09/10 with 5+ non-recurring events (month view '+N more'):")
    dense = conn.execute(
        "SELECT date(start_utc,'unixepoch','localtime') d, count(*) n FROM events"
        " WHERE all_day = 0 AND status='confirmed' AND rrule IS NULL"
        " GROUP BY d HAVING n >= 5 ORDER BY d"
    ).fetchall()
    print("      " + ", ".join(f"{d}={n}" for d, n in dense))
    print(f"      {len(dense)} such days")

    print("\n  empty days (no stored row overlapping the day, recurrences excluded):")
    for date in EMPTY_DAYS:
        n = conn.execute(
            "SELECT count(*) FROM events WHERE start_utc < ? AND end_utc > ?",
            (day(plus_days(date, 1)), day(date)),
        ).fetchone()[0]
        state = "empty" if n == 0 else f"NOT EMPTY ({n})"
        failed += n != 0
        print(f"      {date}: {state}")

    if failed:
        print(f"\n{failed} check(s) failed")
        sys.exit(1)


if __name__ == "__main__":
    home = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_DATA_HOME
    real = Path(os.path.expanduser("~/.local/share")).resolve()
    if home.resolve() == real or real in home.resolve().parents:
        sys.exit(f"refusing to build a fixture inside the real data dir: {home}")
    database, connection = build(home)
    report(connection)
    print(f"\n  launch: cd {REPO} && XDG_DATA_HOME={home} ./target/release/agenda")
