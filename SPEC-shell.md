# Spec: `shell`

*Module of [SPEC-ui-map.md](SPEC-ui-map.md). `SPEC.md` (v1) still holds — read-only, no
keyring, §7 style, §8 testing. Independent of every other module; blocks none of them.*

## Objective

Make the sidebar collapsible and quieter, and move the account actions into it. Today the
sidebar is a hand-rolled `gtk::Box` + `Separator` pinned at 280px that cannot be hidden, and
`Connect account` sits in the header wearing `suggested-action` — the loudest widget in the
window, for something done four times ever.

**Most of this is already built.** `adw::OverlaySplitView` (libadwaita 1.4, already enabled
via the `v1_4` feature) handles collapsing, the overlay behaviour and the swipe gesture.
Replacing the `Box` with it covers two of the three states. Only the icon rail is custom.

## Success criteria

1. **Three states**: expanded (280px), icon rail, hidden. The grid takes the freed width in
   each.
2. **The rail shows account avatars and calendar colour dots**, enough to see who is on
   screen and toggle a calendar without expanding.
3. **The state survives a restart.**
4. **Narrow windows collapse automatically** via `adw::Breakpoint`, and the user's explicit
   choice wins over the automatic one until the window is resized again.
5. **`Connect account` lives in the sidebar footer**, flat rather than `suggested-action`,
   with the sync button beside it. Both are reachable from the rail.
6. **The header's right-hand corner is left alone** — `views-timegrid` puts the view switcher
   there beside previous/next/today.
7. **Reconnect still surfaces per account** (§2.11) in every state, including the rail: an
   account needing re-auth must be visible without expanding.
8. **The empty state still reads** "No accounts connected yet" with a way to connect.

## Commands / style / testing

Unchanged from `SPEC.md` §5, §7, §8. This module is almost entirely GTK and therefore exempt
from unit tests under §8; it is verified by running. The one pure part — which state a given
window width and stored preference resolve to — is testable and gets a test.

## Boundaries

**Always** — use `OverlaySplitView` rather than hand-rolling collapse; keep the colour rule
where it is.
**Ask first** — any new dependency; any change to `settings.toml` beyond the sidebar state.
**Never** — move the view switcher or navigation out of the header; hide Reconnect behind a
state the user has to discover.

## Open questions

1. **How does the user cycle three states?** A single toggle button is ambiguous with three.
   Options: one button cycling expanded → rail → hidden; or a button toggling hidden/shown
   with the breakpoint choosing rail; or expanded/hidden by button and rail only ever
   automatic.
2. **Does the rail show calendars, or only accounts?** Four accounts with sixteen calendars
   is a long rail. Proposed: account avatars always, calendar dots under the account the
   pointer is over.
