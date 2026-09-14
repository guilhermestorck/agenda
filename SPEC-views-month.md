# Spec: `views-month`

*Module of [SPEC-ui-map.md](SPEC-ui-map.md). `SPEC.md` (v1) and `SPEC-views-timegrid.md`
still hold — read-only, no keyring, §7 style, §8 testing.*

## Objective

A month grid: whole weeks, one cell per day, events as compact chips. No hour axis, so none
of `views-timegrid`'s vertical mapping applies here — the compressed quiet hours, the
expansion rule and the lead-in band are all irrelevant to this view.

**It needs no new layout code either.** `layout::segments(start, end, grid_start, zone, 42)`
already splits an occurrence into per-day pieces, clamped to the window, and already handles
multi-day and midnight-spanning events. Month wants only the `day` field of each segment and
ignores the minutes. Six weeks is 42 days; the function does not care.

## Success criteria

1. Six week rows, always — a fixed 42-day grid so the layout never reflows between months.
2. Days outside the month are dimmed but present and still show their events.
3. Weeks start Monday, matching the grid.
4. Today is marked the same way it is in the grid.
5. A multi-day event appears in every cell it covers.
6. Each cell shows chips up to what fits, then "+N more"; N is correct, not an estimate.
7. Previous/next moves one calendar month, and "today" returns to the current one.
8. Chips carry the calendar colour and account marker, same facts as every other view.

## Commands / style / testing

Unchanged from `SPEC.md` §5, §7, §8. The day-bucketing is `segments` and is already tested.
What is new and pure, and therefore tested:

- the 42-day window for a given month starts on the Monday on or before the 1st
- a month whose 1st is a Monday still yields 42 days, not 35 or 49
- a multi-day event yields one entry per covered cell
- "+N more" counts every event not shown, not just the next one

## Boundaries

**Always** — reuse `layout::segments` for day bucketing; reuse `collect_items` for data.
**Ask first** — any new dependency.
**Never** — event creation, editing or dragging; a second day-bucketing implementation.

## Open questions

1. **How many chips per cell before "+N more"?** Depends on cell height, so proposed:
   computed from the measured cell, not a constant.
2. **Does clicking a day switch to day view?** It is the natural gesture and costs almost
   nothing once the switcher exists. Proposed: yes.
