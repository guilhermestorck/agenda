---
layout: default
title: v1 Acceptance
---

# v1 acceptance walkthrough

*Recorded 2026-09-12, against `SPEC.md` §2.*

*Updated 2026-09-13, after the first real Google accounts were connected.*

**v1 is not done**, but it is closer than the first draft of this page said. Several criteria
have now been verified against live Google data rather than fixtures. `SPEC.md` §9 forbids
claiming end-to-end verification that did not happen, so each criterion below says exactly
what was checked and how — and where a check was made against real data, it says so.

Two criteria remain outstanding for reasons that are not code: one needs more than one
account connected at once, the other is gated on the user.

| # | Criterion | Status | Evidence |
|---|---|---|---|
| 1 | Sync is correct | **Verified against Google** | A real account synced 2 calendars and 386 events, both calendars left holding a `nextSyncToken`. Delta, deletion and `410 GONE` → silent full resync remain fixture-verified against recorded payloads and a stand-in server that asserts request parameters. |
| 2 | The week is accurate | **Fixture-verified** | Recurrence, EXDATE/RDATE, modified and cancelled instances, all-day placement, midnight spans, and DST in both directions all tested. **Not** compared against Google's web UI. §2.2 itself was corrected — see below. |
| 3 | Every account on screen at once | **Partial** | Four real accounts were connected simultaneously — 16 calendars, 5,573 events, no switcher and no filter. The week was not visually compared against Google's web UI before they were disconnected, so the *rendering* half is still only verified synthetically. |
| 4 | Accounts distinguishable without interaction | **Met** | Verified in the case the criterion names: three calendars set to near-identical Google colours (`#16a765`/`#16a766`/`#16a764`), so the fill carries no account information and the leading stripe carries all of it. Synthetic data, but the visual test is the real one. |
| 5 | Styling is the user's and persists | **Met at the data layer** | A test runs a real sync over a set colour and visibility and asserts both survive; another reconnects an account and asserts label, colour and ordering survive. A live full resync has not been run. |
| 6 | A second account can be added from the UI | **Code complete, unexercised** | "Connect account" is in the header bar at all times and needs no new credentials. Never run against Google's consent screen. |
| 7 | Duplicates render honestly and are detectable | **Verified by query** | On four live accounts, `SELECT … GROUP BY ical_uid HAVING count(DISTINCT account) > 1` returned **551 distinct meetings present on two or more accounts**, and **zero** events anywhere were missing an `ical_uid`. Exactly the check §2.7 specifies — by query, not by eye. The rendering half (two copies, two markers) is verified visually against a synthetic database. |
| 8 | Offline works | **Verified with the network down** | With `nmcli networking off` and DNS failing, a cold start painted `week=2026-09-14 events=2` — the two events genuinely in that week — from SQLite. Zero `ERROR` lines, zero "needs reconnecting": the failure was classified transient, as an absent network must be. The 388 stored events were untouched by the failed sync, and syncing resumed on its own when the network returned. |
| 9 | Reminders fire | **Met, end to end** | A seeded event produced a real desktop notification 30 seconds after launch, through the actual notification daemon. Recurring occurrences are covered by the window query. The suspend/resume cycle is left to the user; the catch-up path is unit-tested. |
| 10 | The tray is live | **Verified across a plasmashell restart** | The item registers as `org.kde.StatusNotifierItem-<pid>-1` with a `ToolTip` naming the next real event. After `systemctl --user restart plasma-plasmashell` the same item was registered again with the same tooltip — ksni re-registers on host loss without the application noticing. |
| 11 | Re-auth is graceful and isolated | **Met in mechanism** | An account whose tokens are absent is parked on its own Reconnect button with a single toast, while the others carry on; one calendar failing does not cost the account the rest. Verified with tokens missing rather than revoked, and with synthetic accounts. |
| 12 | Evolution is gone | **Half done** | `evolution` is uninstalled. **`evolution-data-server 3.60.2-5` is still installed**, with four of its daemons running — including `evolution-alarm-notify`, which means the old stack is still firing calendar alarms alongside agenda's. Nothing depends on it (`pactree -r` returns only itself), so removal is safe; it needs the user's password. |
| 13 | The §8 quality bar, with no suppressions | **Met** | 223 tests passing, `cargo clippy --all-targets` clean at zero warnings, zero `#[allow(...)]` attributes anywhere in `src/`, one `#[ignore]`d test carrying its reason (it writes to the real keyring), `cargo fmt` clean. |

## What is actually blocking

**Comparing a real week against Google's web UI.** Criteria 2 and 3 are the last that
genuinely need it: the data layer is tested exhaustively, but nobody has yet put the two
renderings side by side. Criterion 6 needs a second account added from the UI after the
first, which is a single click nobody has made.

Live data has already exercised paths that fixtures could not. A holiday calendar id of the
form `pt.spain#holiday@group.v.calendar.google.com` went through the real API unharmed,
confirming that escaping calendar ids before they become path segments was not a theoretical
concern — left raw, the `#` truncates the request at a fragment.

## Corrections made to the spec along the way

- **§2.2 demanded a DST week in `America/Sao_Paulo`**, which no current date can satisfy:
  Brazil abolished daylight saving in 2019 and the zone has been a flat `-03:00` since,
  verified against this machine's tz database. The criterion now names `Europe/Madrid`, the
  zone this machine runs in. São Paulo is still tested, at its real historical transitions in
  November 2018 and February 2019.
- **§1 and §4 disagreed about an event's fill colour.** §4 put the account's colour above
  Google's, so fill and marker resolved to the same value and the account stripe disappeared
  into the event it sat on — discarding the calendar dimension §1 exists to preserve. §4 was
  corrected to `user_color → Google's → account → generated`.
- **`accounts` gained `display_name` and `picture_url`**, and `accounts`/`calendars` gained
  `notify_lead_minutes`, and `events` gained `reminder_minutes` — each signed off before
  being written, per §9. The user-owned ones inherit §4's protection structurally.

## Evolution

`evolution` was removed by the user on 2026-09-14. `evolution-data-server` remains, and
until it goes the criterion is not met — its `evolution-alarm-notify` daemon is still
running, so calendar alarms are being fired by two stacks at once.

```
sudo pacman -Rns evolution-data-server
```

`migration-backup/` holds the only copy of the single EDS event and has not been read,
modified or deleted. The ICS import that would restore it is M6, deferred past v1, so
`~/.config/evolution` and `~/.local/share/evolution` should stay until that lands.

## Deferred past v1, and untouched

The write path, local calendars and ICS import/export, month and day views, search,
drag-to-create/move/resize, and packaging as a PKGBUILD.
