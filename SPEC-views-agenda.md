# Spec: `views-agenda`

*Module of [SPEC-ui-map.md](SPEC-ui-map.md). `SPEC.md` (v1) and `SPEC-views-timegrid.md`
still hold — read-only, no keyring, §7 style, §8 testing.*

## Objective

A scrolling chronological list of upcoming events. The cheapest view to build and likely the
one used daily: "what is next" without reading a grid.

**It needs no new machinery.** `collect_items(ui, from, to)` already returns `Vec<Item>` for
any window, already resolves colours and account pictures, and already applies §4's colour
rule. Agenda is that vector, sorted by `start_utc`, rendered as rows. No layout module, no
new query, no new store code.

## Success criteria

1. Events from now forward, in start order, grouped under a date header per day.
2. Each row shows time, summary, calendar colour and account marker — the same four facts
   the grid shows, so the two views never disagree.
3. All-day events sort to the top of their day and read "All day" rather than a time.
4. An event spanning midnight appears under its start day only, with its end shown.
5. Today's already-started events are still listed; the list begins at the start of today,
   not at the current minute.
6. An empty window says so plainly rather than rendering a blank panel.
7. Scrolling to the bottom extends the window by another month.

## Commands / style / testing

Unchanged from `SPEC.md` §5, §7, §8. GTK exempt from unit tests; the sort and grouping are
pure and carry tests:

- all-day events precede timed ones within the same day
- ties on `start_utc` break on summary, so the order is stable between redraws
- grouping puts an event in the day its start falls in, in the display zone, not UTC

## Boundaries

**Always** — reuse `collect_items`; keep grouping pure and testable.
**Ask first** — any new dependency; any change to `Item`.
**Never** — event creation or editing; a second copy of the colour rule.

## Open questions

1. **How far forward initially?** Proposed: 30 days, extending by a month on scroll.
2. **Does clicking a row do anything?** v1 is read-only, so proposed: nothing. A click that
   jumps the grid to that day is the obvious later addition.
