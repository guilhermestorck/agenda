# Spec: `tray-day`

*Module of [SPEC-ui-map.md](SPEC-ui-map.md). Depends on `views-timegrid`. `SPEC.md` (v1)
still holds — including §2.10, which the existing tray item already meets.*

## Objective

Clicking the tray opens the current day over a short span, so "what is next" costs one click
and no window management. Today a click toggles the whole application window.

**It is the time grid with `N = 1` and a bounded vertical window.** No second grid, no second
layout path — which is why the map made it depend on `views-timegrid` rather than the tray.

## Success criteria

1. **A left click opens the day view**, not the main window. The tray menu keeps Show agenda
   and Quit.
2. **The visible span defaults to five hours** — one before now, four after — and is
   configurable in `settings.toml`.
3. **It scrolls to the rest of the day**, so the span is a starting position and not a limit.
4. **Previous day, next day and today** controls.
5. **A button opens the full application window** and closes the popup.
6. **It reflects calendar visibility and account colours** exactly as the main grid does. Two
   views of the same day must never disagree.
7. **Closing and reopening returns to now**, not to wherever the user last scrolled.
8. **It opens anchored near the tray** — deferred to the second slice, see below.

## Two slices

The user approved `gtk4-layer-shell` on 2026-09-14 but staged it behind the view itself.
`gtk4-layer-shell 1.3.0` is installed and KWin 6.7.4 advertises `zwlr_layer_shell_v1` v5.

- **Slice 1** — an ordinary `adw::Window`. KWin places it. Everything but criterion 8.
- **Slice 2** — layer-shell anchoring, dismissal on focus loss, and the crate added to
  `Cargo.toml` under §9's dependency rule.

Splitting them means the view is verifiable before the compositor integration is in the way.

## Commands / style / testing

Unchanged from `SPEC.md` §5, §7, §8. Almost entirely GTK and verified by running. The one
pure part — the span's start and end given a clock time and the configured hours — is tested,
including the case where now is close to midnight and the window would run past it.

## Boundaries

**Always** — reuse the time grid; reuse `collect_items`.
**Ask first** — adding `gtk4-layer-shell` to `Cargo.toml` (slice 2).
**Never** — a second grid implementation; event creation; quick-add. v1 is read-only.

## Open questions

1. **Does the popup close when it loses focus?** Natural with layer-shell, fiddly without.
   Proposed: slice 1 closes on Escape and on the open-full-window button only.
2. **Does it follow the main window's chosen span?** Proposed: no — it is always one day,
   whatever the main window shows.
