# Spec: `timezones`

*Module of [SPEC-ui-map.md](SPEC-ui-map.md). Depends on `views-timegrid` and `views-agenda`.
`SPEC.md` (v1) still holds.*

## Objective

Let the user choose the display timezone instead of inheriting the system's, and show an
optional second zone alongside it — for working across them without doing the arithmetic.

Today `week.rs` reads `/etc/localtime`'s symlink target and that is the only zone there is.
The override is a settings lookup with that read as the fallback, so the existing function
stays and gains a caller rather than being replaced.

## Success criteria

1. **A display zone set in `settings.toml` wins over `/etc/localtime`**; absent, behaviour is
   exactly as today.
2. **An unknown or malformed zone name falls back to the system zone** and says so in the
   log, rather than failing to start.
3. **Changing the zone redraws every view** without restart, and events keep their real
   instants — an event does not move in time, only where it lands on the grid.
4. **The secondary zone, when set, is a second hour-label column** in the time grid, left of
   the primary, labelled with its abbreviation.
5. **Agenda rows show both times** when a secondary zone is set — `14:00 (08:00 EDT)`.
6. **Month ignores the secondary zone entirely.** Its chips have no room for two clocks.
7. **All-day events do not acquire a second time.** They have no instant to convert.
8. **A DST transition in either zone is handled per-instant**, so the offset between the two
   columns may change partway down the day and must be computed per hour label, not once.

## Commands / style / testing

Unchanged from `SPEC.md` §5, §7, §8. The conversion is pure and carries tests, including
criterion 8 — the two zones' transitions falling on different dates is the case that breaks a
single cached offset.

## Boundaries

**Always** — keep `zone_name_of` and its `/etc/localtime` read as the fallback path.
**Ask first** — any new dependency; `chrono-tz` is already present and sufficient.
**Never** — store converted local times; the store keeps UTC (`SPEC.md` §4).

## Open questions

1. **Where is the zone picked?** A preferences dialog does not exist yet. Proposed:
   `settings.toml` only for now, with a picker when there is somewhere to put it.
2. **Is the secondary zone labelled by abbreviation or city?** `EDT` is compact but
   ambiguous twice a year; `New York` is unambiguous and wide. Proposed: abbreviation.
