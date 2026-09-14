# Capability map: UI improvements

*Approved 2026-09-14.* Module boundaries, dependency direction and build order are settled;
module specs follow.

**Both `views-month` and `views-agenda` ship before the next manual test.** Their order
relative to each other is an implementation detail, not a scoping decision.

All seven module specs are written as of 2026-09-14: `SPEC-views-timegrid.md`,
`SPEC-views-month.md`, `SPEC-views-agenda.md`, `SPEC-shell.md`, `SPEC-timezones.md`,
`SPEC-lead-in.md`, `SPEC-tray-day.md`.

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
| `shell` | Collapsible sidebar with its three states, its visual design, and the account actions relocated into it — Connect account, made discreet, alongside a manual "sync now" control. | — |
| `timezones` | A display-zone override, and an optional secondary zone: a second hour-label column in the grid, and both times on agenda rows. | `views-timegrid`, `views-agenda` |
| `tray-day` | A compact day view opened from the tray: the current day over a configurable hour span (default 5h — one hour back, four forward), scrollable within the day, with previous/next/today controls and a button that opens the full window. | `views-timegrid` |
| `lead-in` | A translucent band from a reminder's fire time to its event's start, so the user can see when they will be told. Time grid only. | `views-timegrid`, `notify` |

## Build order

```
views-timegrid ─┬─▶ views-month
                ├─▶ views-agenda ──┐
                ├─────────────────▶┴─ timezones   (needs both: grid axis + agenda rows)
                └─▶ lead-in

shell ───────────── (independent, any time)

views-timegrid ──▶ tray-day
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

## `tray-day` — stated requirements

1. Clicking the tray item opens the **current day's view**, not the main window.
2. The visible span is **configurable, defaulting to five hours**: one hour before now,
   four after.
3. It **scrolls** within the day, so the rest of it is reachable.
4. **Previous day / next day / today** controls.
5. A button that **opens the full application**.

### Constraints that shape it

- **The tray menu cannot hold this.** A StatusNotifierItem menu is a D-Bus menu — items,
  not widgets — so the day view must be its own small GTK window that the tray click opens.
- **Wayland clients cannot position their own windows.** An ordinary window lands wherever
  KWin decides. Anchoring needs `gtk4-layer-shell`; KWin 6.7.4 advertises
  `zwlr_layer_shell_v1` version 5, verified against the running compositor.
  **Approved 2026-09-14, staged:** the view ships in an ordinary window first, and anchoring
  arrives as a follow-up slice once the content is right. The crate is `gtk4-layer-shell`
  0.8.1 and it binds to a system library that is **not yet installed** —
  `sudo pacman -S gtk4-layer-shell` is the user's to run when that slice starts.
- **Plasma's volume, clipboard and bluetooth popups are applets**, QML running inside
  plasmashell, not tray clients. An external application cannot be one of those without
  being rewritten as a Plasma applet. Layer-shell gets the behaviour — anchored, above other
  windows, dismissed on focus loss — but the popup will look like agenda, not like Plasma.
- The hour span makes this a time grid with `N=1` and a bounded vertical window, which is
  why it depends on `views-timegrid` rather than reimplementing the grid.

## Open questions for the map review

1. ~~**Do `timezones` and `lead-in` apply outside the time grid?**~~ **Resolved 2026-09-14:**
   - `timezones`: a second hour-label column in the time grid, **and** both times on agenda
     rows. Month ignores it — its chips have no room for two clocks.
   - `lead-in`: **time grid only.** The band needs time-space to have a height; month and
     agenda show nothing.
2. **Does the view switcher belong to `views-timegrid` or `shell`?** Proposed: to the grid
   module, since it switches views. That keeps `shell` free of any dependency on which views
   exist.
3. ~~**Is `views-agenda` wanted before `views-month`?**~~ **Resolved 2026-09-14:** both are
   in scope before the next manual test; the order between them is mine to choose.
