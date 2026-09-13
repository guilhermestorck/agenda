# agenda — specification

Derived from PLAN.md (architecture, milestones, verified constraints), which remains the
local working record. This file is the contract: what "done" means, and what may not be
done without asking.

Status of this document: v1 scope agreed 2026-09-10. PLAN.md's M0–M10 is the long road;
this spec fences off the part that has to work before the calendar is usable daily.

**Implementation status (2026-09-11).** The repository was reset to zero commits on
2026-09-10. Built since: `auth`, `store`, `sync`, `accounts`, `recur`, and the week grid.
Not yet built: the settings surfaces, the background sync loop, `notify` and `tray`.
Everything Google-facing is verified against recorded fixtures and a local stand-in server;
**nothing in this project has yet made a request to Google**, because no account has been
connected. Where a criterion below has been met only against fixtures or a synthetic
database, `tasks/todo.md` says so rather than implying coverage that does not exist (§9).

---

## 1. Objective

A GTK4/libadwaita calendar for Linux that replaces Morgen for daily use and Evolution
entirely, on a single-user KDE Plasma/Wayland machine.

Multi-account Google Calendar, synced by our own engine into SQLite, rendered offline-first
so the UI never waits on the network. Tokens live in the Secret Service (ksecretd), never
on disk in the repo.

### Multiple accounts, shown together

The user holds several Google accounts — personal and professional — and needs them on
screen **simultaneously**, in one week. This is not an account switcher: there is no
"current account", and no mode where some of the schedule is hidden. Every connected
account contributes to the same grid at the same time.

Two consequences that drive the design:

- **Every event must be attributable to its account at a glance**, without clicking it.
  A week is only useful if a work meeting is instantly distinguishable from a personal one.
- **Styling is the user's, not Google's.** Per-account defaults and per-calendar overrides
  are both settable and persist locally, because Google's colour assignments were made for
  Google's UI and carry no work/personal meaning.

**Colour model: calendar colour + account marker.** An event's fill is its calendar's
colour — the user's override if set, otherwise Google's. Its *account* is carried by a
separate, secondary cue (a coloured leading stripe or equivalent) so the two dimensions
stay independently readable. An account-level colour is a default for its calendars; a
per-calendar override wins over it.

**Duplicates across accounts are shown, not merged — for now.** A meeting you are invited
to on two addresses arrives twice and renders twice. The event genuinely exists on both
calendars, and hiding one would misrepresent which account it is on.

This is the **v1 default, not a permanent decision.** How duplicates are displayed is
intended to become a user preference — show both, collapse into one, or show both visually
linked. To keep that option open at no later cost, `ical_uid` is captured from the very
first sync (§4), even though nothing in v1 reads it. Storing it later would mean a full
resync of every calendar, because Google will not backfill the field through a `syncToken`
delta.

### v1 is read-only

Events are created and edited in Google's own web UI. Agenda shows them, correctly, and
reminds you about them. This is a deliberate narrowing: the write path (M5) is where etag
conflicts, offline queues and partial-failure recovery live, and none of that is needed to
stop paying for Morgen.

**Consequence to accept:** PLAN.md's M8 describes a tray with "next event, quick-add".
Quick-add is a write. For v1 the tray shows the next event and toggles the window; the
quick-add entry point is deferred with the rest of the write path.

### Non-goals

Not in scope at any point, unless this document changes: CalDAV or Exchange/Outlook
backends, mobile or web clients, multi-user or shared installs, a plugin system, or any
hosted service component. Nothing about this app phones anywhere except Google's API.

---

## 2. Acceptance criteria

v1 is done when **all** of the following hold on the target machine:

1. **Sync is correct.** A first run performs a full sync of every visible calendar on every
   connected account; subsequent runs use `syncToken` and transfer only deltas. A deletion
   made in Google's UI disappears locally on the next sync. A `410 GONE` on a stale token
   silently triggers a full resync rather than surfacing an error.
2. **The week is accurate.** The week view matches Google Calendar's own web UI for the
   same week, including: recurring series, single modified instances of a series, all-day
   events on the correct day, events spanning midnight, and a week containing a DST
   transition in `Europe/Madrid` — the zone this machine runs in and the one that matters.
   *(Corrected 2026-09-11: this criterion originally also demanded a DST week in
   `America/Sao_Paulo`. Brazil abolished daylight saving in 2019 and the zone has been a
   flat `-03:00` ever since, so no current date satisfies it. São Paulo is still tested, at
   its real historical transitions in November 2018 and February 2019, which exercises the
   same code against genuine offset changes.)*
3. **Every account is on screen at once.** With at least three Google accounts connected
   (two personal, one professional), one week view shows events from all of them
   simultaneously. No account switcher, no filter that must be changed to see the rest.
4. **Accounts are visually distinguishable without interaction.** Given a week containing
   events from every connected account, each event's source account is identifiable at a
   glance — no hover, no click, no legend lookup. Verified with the accounts' calendars
   deliberately set to similar Google colours, which is the case this must survive.
5. **Styling is the user's and it persists.** A colour can be set per account (a default
   for its calendars) and per calendar (which wins over the account default). Both survive
   an app restart *and* a full resync — a sync must never overwrite a user's choice. The
   same guarantee covers per-calendar visibility (§4).
6. **A second account can be added from the UI.** Connecting an additional Google account
   is reachable at any time, not only on first run, and requires no new OAuth credentials.
7. **Duplicates render honestly, and are detectable.** A meeting present on two accounts
   appears twice, each copy carrying its own account's marker. Both copies have the same
   non-null `ical_uid` in the store — verified by query, not by eye, since nothing in v1
   displays it. This is what makes the post-v1 preference (§10) buildable without a resync.
8. **Offline works.** With networking disabled, launching the app still paints the last
   synced week. No spinner, no error page, no empty grid.
9. **Reminders fire.** A notification appears at the configured lead time before an event,
   including for recurring occurrences, and including across a suspend/resume cycle that
   spans the scheduled time.
10. **The tray is live.** A StatusNotifierItem shows the next upcoming event and toggles
    the window. It survives a plasmashell restart.
11. **Re-auth is graceful, and isolated.** A revoked or expired refresh token puts that
    account back on a "Reconnect" affordance rather than an error dump — and the other
    accounts keep syncing and rendering meanwhile. One dead token must never blank the
    whole calendar. Verified by deleting a single account's keyring entry.
12. **Evolution is gone.** `evolution` and `evolution-data-server` are uninstalled and
    nothing regressed. (Gated on the user; see Boundaries.)
13. **The quality bar in §8 is met** with no suppressions.

### Explicitly deferred past v1

Write path (M5), local calendars and ICS import/export (M6), month and day views, search,
drag-to-create/move/resize (M9), packaging as a PKGBUILD (M10). The ICS import matters
only for one Evolution event, backed up in `migration-backup/`, and can wait.

---

## 3. Capability map

```
auth ──────▶ sync ──────▶ store ◀────── recur ──────▶ views
  ▲                         ▲                           ▲
  │                         │                notify ────┤
accounts ───────────────────┘                tray ──────┘
```

| Module | Owns | Depends on | State |
|---|---|---|---|
| `auth` | OAuth2 PKCE loopback, token lifecycle, keyring | — | not started |
| `store` | SQLite schema, upserts, range queries | — | not started |
| `sync` | `events.list`, syncToken full→delta, JSON→Event | `auth`, `store` | not started |
| `accounts` | Add/remove accounts, per-account + per-calendar styling, visibility | `auth`, `store` | not started |
| `recur` | RRULE expansion, timezones, DST, all-day | `store` | not started |
| `views` | Week grid, calendar colour + account marker, style settings UI | `store`, `recur`, `accounts` | not started |
| `notify` | Lead-time scheduler, sleep/resume safe | `store`, `recur` | not started |
| `tray` | ksni StatusNotifierItem, next event | `store`, `recur` | not started |

**Build order:** `auth` → `store` → `sync` → `accounts` → `recur` → `views` → `notify` → `tray`

`accounts` lands before `views` because the week grid cannot be built against a
styling model that does not exist yet — the colour resolution rule (§4) is an input to
every event widget, not a decoration added afterwards.

This reorders PLAN.md, which put the week view (M3) before recurrence (M4). A week view
that cannot expand a recurring series renders a mostly-empty calendar, because nearly every
real event repeats — so `recur` lands first and `views` is built against it.

---

## 4. Data model — APPROVED 2026-09-10

Signed off by the user. Not yet written — there is no code in the repository. Any change to
this schema from here needs sign-off again (§9).

The repository is greenfield, so this is the schema at `SCHEMA_VERSION = 1` — stated whole,
with multi-account and user styling native from the first line rather than migrated in
later. There is no migration to write and no version 2.

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;

-- Connected accounts: the user's display preferences, plus the profile Google reports.
-- Sync never writes here. Connect refreshes the two server-owned columns and nothing else;
-- label, color and sort_order are the user's and are never overwritten.
CREATE TABLE accounts (
    email       TEXT PRIMARY KEY,
    provider    TEXT NOT NULL DEFAULT 'google',
    added_at    INTEGER NOT NULL,
    -- Server-owned, refreshed on connect: Google's own profile for this address, so the
    -- sidebar can show a name and a face. Added 2026-09-11 with sign-off; no data existed.
    display_name TEXT,
    picture_url  TEXT,
    -- Short human label: "work" reads better in a sidebar than a 30-character address.
    label       TEXT,
    -- The account's marker colour, and the default fill for its calendars. Assigned from
    -- a palette on connect, so a newly added account is never unmarked.
    color       TEXT,
    -- Sidebar ordering: the user's grouping, not Google's.
    sort_order  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE calendars (
    account     TEXT NOT NULL REFERENCES accounts(email) ON DELETE CASCADE,
    id          TEXT NOT NULL,
    -- Server-owned: refreshed on every sync.
    summary     TEXT NOT NULL,
    color       TEXT,                     -- Google's colour for this calendar
    timezone    TEXT,
    access_role TEXT NOT NULL DEFAULT 'reader',
    is_primary  INTEGER NOT NULL DEFAULT 0,
    -- User-owned: sync must never overwrite these two. See below.
    visible     INTEGER NOT NULL DEFAULT 1,
    user_color  TEXT,                     -- NULL means "use Google's"
    -- Google's opaque incremental cursor; NULL forces a full resync.
    sync_token  TEXT,
    synced_at   INTEGER,
    PRIMARY KEY (account, id)
);

CREATE TABLE events (
    account       TEXT NOT NULL,
    calendar_id   TEXT NOT NULL,
    id            TEXT NOT NULL,
    -- Google's cross-calendar identity. The same meeting invited to two of the user's
    -- addresses carries the SAME value on both copies, which is the only thing that makes
    -- the duplicate case detectable. Stored from the first sync so the display choice in
    -- §10 stays open without a later backfill resync. Nothing in v1 reads it.
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

CREATE INDEX events_by_range    ON events (start_utc, end_utc);  -- the hot window query
CREATE INDEX events_by_calendar ON events (account, calendar_id);
CREATE INDEX events_by_master   ON events (recurring_event_id);
CREATE INDEX events_by_ical_uid ON events (ical_uid);
```

Two changes from the earlier proposal, both enabled by starting fresh — flag if you disagree:

- **Per-account display lives on `accounts`, not a side table.** It is strictly 1:1, and
  `accounts` is never written by sync, so the separation a side table bought is unnecessary.
- **No `pending_writes` table.** It served the write path, which is deferred past v1 (§2).
  An unused table contradicts §8's stance on dead code; it arrives with the feature.

**`ical_uid` is not a unique key.** Every instance of a recurring series shares one, and so
does every copy of a meeting across accounts — which is the point. Any dedup logic built on
it must group by `(ical_uid, start_utc)` at minimum, never treat it as an identity.

### Colour resolution

- **Event fill:** `calendars.user_color` → `calendars.color` (Google's) →
  `accounts.color` → a generated fallback. *(Corrected 2026-09-11: the account colour was
  above Google's, which contradicted §1. Every account is assigned a colour on connect, so
  above Google's it became the fill of every calendar on the account — collapsing fill and
  marker into one colour and discarding the calendar dimension entirely.)*
- **Account marker:** `accounts.color` → a colour auto-assigned from a palette when
  the account is added, so a newly connected account is never unmarked.

Calendar beats account, per §1. The marker is always the account's, never the calendar's —
that is what keeps the two dimensions independently readable.

### The rule that must not be broken

**A sync must never overwrite `calendars.visible`, `calendars.user_color`, or any of
`accounts.label` / `.color` / `.sort_order`.** Since 2026-09-11 `accounts` also holds two
server-owned columns (`display_name`, `picture_url`), so the same `DO UPDATE` discipline now
applies to it: connect refreshes those two and names none of the user's. Google knows nothing about these; they exist
only because the user set them, and a routine metadata refresh silently reverting a
deliberate choice is the kind of bug that gets noticed weeks later and never reported
properly.

The mechanism: calendar upserts use `INSERT ... ON CONFLICT(account, id) DO UPDATE` that
names *only* the server-owned columns. User-owned columns appear in the `INSERT` (as initial
values for a calendar seen for the first time) and never in the `DO UPDATE`.

This needs a dedicated test, not a comment: set a visibility and a colour, run an upsert
carrying different server metadata, assert the server fields changed and the user fields did
not. It is cheap to write and it is the only thing standing between a preference and a
sync that quietly eats it.

## 5. Commands

```
cargo build                  # dev build
cargo build --release        # optimised, lto + strip
cargo test                   # all unit tests; must be green
cargo test -- --ignored      # tests that touch the real keyring
cargo clippy --all-targets   # must be clean; see §8
cargo fmt                    # before any commit
./scripts/install.sh         # user-local install to ~/.local (to be written)
```

Runtime logging is `tracing` via `RUST_LOG`, e.g. `RUST_LOG=agenda=debug cargo run`.

Setup that only the user can do: create a Google Cloud OAuth **Desktop** client and write
it to `~/.config/agenda/oauth.toml` — see `docs/google-oauth-setup.md`. The publishing
status must be **In production**, or refresh tokens expire every 7 days.

---

## 6. Project structure

The repository currently holds only this spec, PLAN.md, `.gitignore` and `docs/`. The
layout below is the **intended target**, not a description of what exists.

```
src/
  main.rs          adw::Application setup, single-instance activation
  config.rs        XDG paths; reads ~/.config/agenda/oauth.toml
  runtime.rs       the GTK↔tokio bridge; network work never blocks the main thread
  auth/            PKCE, the loopback redirect flow, tokens in the Secret Service
  google/          API client with token refresh, and the wire types it deserialises
  store/           the SQLite schema of §4, and typed access to it
  sync/            full and incremental sync, and the JSON→Event mapping
  accounts/        connecting and removing accounts, styling, visibility
  ui/              the window, the week grid, the settings surfaces
data/              .desktop entries, icons
docs/              setup and reference notes
scripts/           install.sh
```

Rules that hold across the tree:

- **SQLite is the only source of truth the UI reads.** No widget ever awaits the network.
  Sync writes into the store; the UI redraws from the store.
- **Secrets never touch the repo or the binary.** Client credentials are read from
  `~/.config/agenda/`; tokens go to the Secret Service.
- **Wire types stay in `google/`.** The rest of the app knows `store::Event`, not Google's
  JSON shape, so a second backend stays possible.

---

## 7. Code style

There is no code left to imitate, so the voice is specified here rather than pointed at.
The first module written sets the precedent; everything after it matches.

- `//!` module docs and `///` item docs that explain **why**, not what the signature already
  says. Comments earn their place by recording a decision or a trap — the DST case a line
  guards, the reason a parameter must never change. Restating the function name is noise.
- `anyhow::Result` throughout, with `.context(...)` on fallible I/O, so a failure arrives as
  a sentence rather than a bare `io::Error`.
- **Typed errors only where the caller must branch on them.** Two are already known to
  qualify: a refresh token Google has permanently rejected (the account must re-consent —
  no retry helps) and a `syncToken` refused with `410 GONE` (the cache must be dropped and
  refetched). Both change control flow, so neither may be a string.
- `#[cfg(test)] mod tests` inline at the bottom of the file it tests.
- Test names read as assertions, not labels: `recurring_masters_survive_the_range_filter`
  over `test_recurrence`. The name should state what would be broken if it failed.
- Rust edition 2024. `cargo fmt` before committing.

---

## 8. Testing strategy

**The bar: every module outside the GTK layer carries unit tests, and the build is warning-
clean.** No `#[allow(dead_code)]` to hide unfinished wiring — unused code is either wired
up or deleted.

- **Pure functions at the boundary.** Anything that parses or converts external data
  (Google JSON → `store::Event`, RRULE expansion, timezone conversion) must be a pure,
  directly testable function, tested against realistic recorded fixtures. This is the only
  way to test the sync engine at all, since live credentials may not exist.
- **The cases that must be covered**, because they are where calendar apps break: all-day
  vs timed events, DST transitions in both directions, events spanning midnight, EXDATE and
  RDATE, single modified instances of a recurring series, `cancelled` deletions, pagination,
  and `410 GONE` → full resync.
- **Fields stored for the future must be tested like any other.** `ical_uid` has no reader
  in v1, so nothing else would catch it silently going null — and the cost of discovering
  that is a full resync of every calendar. Its mapping is tested from the first commit that
  introduces it, including the case that proves the point: two events from different
  accounts, same meeting, same `ical_uid`.
- **Tests that touch real system services are `#[ignore]`d** with a stated reason, so a
  plain `cargo test` never writes to the user's keyring or hits the network. They are
  still written, still run deliberately (`cargo test -- --ignored`), and still clean up
  after themselves — the keyring round-trip is the obvious first one.
- **Multi-account cases need more than one account in the fixture.** A test with a single
  account passes trivially for account-keyed logic and proves nothing. Anything touching
  the store's `(account, …)` keys, colour resolution, or per-account isolation is set up
  with at least two accounts, one of them holding two calendars.
- **UI code is exempt** from unit tests, and is verified by running the app.
- **No test may be weakened or deleted to reach green.** If a test starts failing, the
  finding is the failure, not the test.

Fixtures written by whoever wrote the parser are worth less than fixtures checked against
the real API shape. Where live verification is impossible, say so explicitly rather than
implying coverage that does not exist.

---

## 9. Boundaries

### Always ask first

- **Commits and pushes.** Work is staged in the tree and the diff shown; nothing is
  committed or pushed without approval. The repository was reset to zero commits on
  2026-09-10 and is heading public, so this history is the one that will be read. Its
  identity is already configured as `guilhermestorck
  <9299698+guilhermestorck@users.noreply.github.com>` — the personal gmail address must not
  reach a commit, because scrubbing it afterwards means rewriting history again.
- **New dependencies.** No crate enters `Cargo.toml` without approval. The stack in PLAN.md
  was chosen deliberately.
- **Anything touching Evolution.** `migration-backup/`, `~/.config/evolution`,
  `~/.local/share/evolution`, and any `pacman` removal. `migration-backup/` holds the only
  copy of the one EDS event and must not be modified or deleted.
- **Database schema.** The schema in §4 needs sign-off before it is written, and any change
  to it afterwards needs sign-off again. Once real synced calendars exist, a bad migration
  costs a full resync at best.
- **Any `secret-tool` command, or anything else that reads the Secret Service.** Including
  the read-only ones. Added 2026-09-13 after `secret-tool search --all application agenda`
  printed four accounts' access and refresh tokens into a session transcript, which cost a
  revoke-and-reconnect of every account. The application's own redaction held throughout —
  `Tokens` and `Credentials` have hand-written `Debug` impls — and was simply walked around
  by a shell command. Nothing needing verification requires the keyring: account presence,
  calendar counts and sync state all come from SQLite, which holds no credentials.

### Never

- Rewrite git history without an explicit, specific instruction.
- Commit any credential, token, database, or `.ics` file. `.gitignore` defends against the
  likely accidents (`client_secret*.json` is the filename Google Cloud Console hands you).
- Create Google Cloud credentials or attempt to authenticate on the user's behalf.
- Weaken, skip, or delete a test to reach green.
- Print, echo, or otherwise emit credential material — tokens, client secrets, passwords —
  by any means, whatever tool produces it. Redaction inside the application is not a
  substitute for not asking for the value in the first place.
- Claim something was verified end-to-end when it was only verified against fixtures.

### Always

- Keep `PLAN.md` local and gitignored. It is the working record, not a published document.
- Treat `cargo test` green plus a warning-clean build as the minimum, not the goal.

---

## 10. Open questions

- **Licence: MIT** (decided 2026-09-10). A `LICENSE` file and a README section still need
  writing.
- **GitHub username.** A rename would break `guilhermestorck.github.io`, which is the
  intended host for the privacy policy and terms that Google's consent screen requires, and
  which would also be registered as an authorized domain. Resolve before publishing.
- ~~**Notification lead time**~~ — **decided 2026-09-12.** It cascades, most specific
  winning: the event's own reminder (read from Google, since v1 is read-only and the user
  sets it where they create the event) → the calendar's → the account's → a global default
  of 10 minutes in `~/.config/agenda/settings.toml`. All-day events are notified too, with
  the lead read as whole days and anchored to a configurable hour, default 09:00 — an
  all-day event starts at local midnight, so counting back from it literally would put every
  reminder in the middle of the night.
- **The form of the account marker.** §1 fixes that there *is* a secondary cue and that it
  carries the account; whether it is a leading stripe, a dot, a border treatment, or
  something else is a `views` decision. It must survive the criterion-4 test: legible when
  two accounts' calendars share similar Google colours.
- **Duplicate display should become a user preference.** v1 shows both copies; the intent
  is a setting offering at least: show both (current), collapse into one marked as
  multi-account, or show both visually linked. `ical_uid` is stored from v1 precisely so
  this needs no migration and no resync when it arrives — only UI and a grouping query.
  Open sub-questions: when collapsing, which copy's details win (and does it matter, given
  the copies are usually identical apart from response status)? Is the preference global or
  per-account-pair? Does a collapsed event show every account's marker, or just the first?
- **Styling when an account is removed and re-added.** `account_display` cascades on
  delete, so disconnecting an account discards its label, colour and ordering. Whether a
  reconnect should recover them (keep the row, or key styling to something more durable
  than the row's lifetime) is unresolved.
- **Adaptive poll interval.** PLAN.md says 60s–5min, backing off when idle or on battery.
  The battery-detection mechanism is unspecified. With several accounts this compounds:
  whether all accounts poll on the same schedule, or a busy work account polls faster than
  a quiet personal one, is undecided.
