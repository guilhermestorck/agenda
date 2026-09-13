# Capability map: UI improvements

*Proposed 2026-09-13. Not yet approved — module boundaries, dependency direction and build
order are reviewed before any module spec is written.*

This is **v2 work**. `SPEC.md` §2 currently lists "month and day views, search,
drag-to-create/move/resize (M9)" under *Explicitly deferred past v1*; approving this map
means amending that list, not quietly contradicting it. v1's read-only constraint still
holds: nothing here creates, edits or moves an event.

## Modules

| Module id | Responsibility | Depends on |
|---|---|---|
| `views-timegrid` | One parameterised time grid: day (N=1), week (N=7), multi-day (N=3/5). Refactors the existing week view rather than adding views beside it. Owns the view switcher. | — |
| `views-month` | Month grid — whole weeks, compact chips, no hour axis. A different layout problem. | `views-timegrid` (switcher, shared queries) |
| `views-agenda` | Chronological list of upcoming events. The cheapest view to build and often the most used. | `views-timegrid` (switcher) |
| `shell` | Collapsible sidebar with its three states, its visual design, and Connect account relocated into it. | — |
| `timezones` | A display-zone override, and an optional secondary zone shown alongside. | `views-timegrid` |
| `lead-in` | A translucent band from a reminder's fire time to its event's start, so the user can see when they will be told. | `views-timegrid`, `notify` |

## Build order

```
views-timegrid ─┬─▶ views-month
                ├─▶ views-agenda
                ├─▶ timezones
                └─▶ lead-in

shell ───────────── (independent, any time)
```

`timezones` and `lead-in` are both drawn *inside* a time grid — a secondary hour axis and a
lead-in band are the grid's business. Building either before the grid is parameterised means
building it twice.

`shell` touches no view code and can land at any point, including first.

## Why the grid is one module and not three

The existing week view already computes day columns from a start date and a `DAYS`
constant, and places events by `layout::segments`, which takes the day count as an argument.
Day and multi-day are that constant becoming a variable plus a date-range label. Specifying
them as separate capabilities would triple the contract for one parameter.

Month and agenda genuinely are separate: neither has an hour axis, so neither reuses the
positioning maths at all.

## Open questions for the map review

1. **Do `timezones` and `lead-in` apply outside the time grid?** A secondary hour axis is
   meaningless in a month grid. A lead-in band could appear in the agenda list as a leading
   marker, or be omitted. Cheapest answer: both are time-grid only, and month/agenda ignore
   them.
2. **Does the view switcher belong to `views-timegrid` or `shell`?** Proposed: to the grid
   module, since it switches views. That keeps `shell` free of any dependency on which views
   exist.
3. **Is `views-agenda` wanted before `views-month`?** It is markedly cheaper and, on a
   calendar with 5,573 events across four accounts, plausibly the one used daily.
