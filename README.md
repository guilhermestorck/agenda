# agenda

A GTK4/libadwaita calendar for Linux, with multi-account Google Calendar sync.

**Status: in development. Nothing is implemented yet.** This repository currently holds the
specification, the policy documents, and setup notes. The first code lands with the tasks
described in [SPEC.md](SPEC.md).

## What it is meant to be

A desktop calendar that shows several Google accounts — personal and professional — in one
week view at the same time. Not an account switcher: every connected account contributes to
the same grid, and every event is attributable to its account at a glance.

Sync is our own engine, writing into a local SQLite database. The UI reads that database and
never waits on the network, so the calendar paints instantly and works offline. OAuth tokens
live in the system Secret Service, never in a file in this repository or beside the binary.

### Planned for the first release

- Multi-account Google Calendar, synced incrementally with `syncToken`
- A week view with per-account and per-calendar colours that a sync can never overwrite
- Recurring events, timezones and DST handled locally
- Desktop notifications at a configurable lead time
- A StatusNotifierItem tray showing the next event

The first release is **read-only**: events are created and edited in Google's own web UI.
Writing back, local calendars, ICS import/export, month and day views and search come later.

### Not planned

CalDAV or Exchange backends, mobile or web clients, multi-user installs, a plugin system, or
any hosted service. Nothing here talks to any server except Google's own API.

## Installing

Requires a Rust toolchain, GTK 4 and libadwaita.

```
./scripts/install.sh
```

That builds in release mode and installs the binary, desktop entry and icon under
`~/.local` — no root, nothing outside your home directory. Set `PREFIX` to install
elsewhere. Autostart is opt-in; the script prints the one command that enables it.

To build without installing:

```
cargo build --release
```

## Setup

Google Calendar access needs an OAuth client that only you can create. See
[docs/google-oauth-setup.md](docs/google-oauth-setup.md); it takes about ten minutes and is
a one-time step.

## Settings

Reminder preferences live in `~/.config/agenda/settings.toml`, and all of them are optional:

```toml
notify_lead_minutes = 10   # how long before an event to notify
all_day_notify_hour = 9    # what time to announce an all-day event
```

Those are the defaults. The lead time cascades, most specific winning: an event's own
reminder from Google, then the calendar's setting, then the account's, then this file. The
per-account and per-calendar settings are in the sidebar.

An all-day event has no start time to count back from, so its lead is read as whole days and
anchored to `all_day_notify_hour`: a lead under a day means that morning, a day or more
means that many mornings earlier.

## Privacy and terms

- [Privacy policy](https://guilhermestorck.github.io/agenda/privacy-policy.html)
- [Terms of service](https://guilhermestorck.github.io/agenda/terms-of-service.html)

## Licence

MIT — see [LICENSE](LICENSE).
