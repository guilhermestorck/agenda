# Spec: `views-timegrid`

*Module `views-timegrid` of [SPEC-ui-map.md](SPEC-ui-map.md), approved 2026-09-14. This is
v2 work; `SPEC.md` remains the v1 contract and everything in it still holds — above all that
v1 is read-only, so nothing here creates, edits or moves an event.*

## Objective

Turn the week view into **one time grid with a variable column count**, so that day (N=1),
week (N=7) and multi-day (N=3 or 5) are the same widget configured differently rather than
three implementations to keep in step. Add the switcher that moves between them.

The user is a single person with several Google accounts who lives in this view daily. The
week is already usable; the point of this module is that "what am I doing today" and "what
does this week look like" stop being different tools.

**Why this is smaller than it looks.** `ui::layout::segments` already takes the day count as
an argument and is tested against it. The geometry is done. What is hard-coded is
`week.rs`'s `const DAYS: usize = 7`, used in ten places, plus one in `ui/mod.rs` that sizes
the query window. The work is making that a field, rebuilding the day headers when it
changes, and teaching navigation and the title to speak in spans.

## Success criteria

1. **One grid, three spans.** Day, 3-day, 5-day and week are the same widget. Switching
   between them redraws without reconstructing the window.
2. **Each span has its own anchoring and its own step.** Not one generalised rule — the set
   is small and discrete, and the rules differ:

   | Span | Starts on | Previous/next steps |
   |---|---|---|
   | 1 — day | the focused day | 1 day |
   | 3 — three days | the focused day, today **first** rather than centred | 3 days |
   | 5 — working week | the **Monday** of the focused day's week, always Mon–Fri | 7 days |
   
   | 7 — week | the **Monday** of the focused day's week | 7 days |

   The five-day step is seven days on purpose: stepping five from a Monday lands on a
   Saturday, and the span is defined as Mon–Fri, not as "five days from wherever".

   "Today" always brings the current day into view, under whichever rule the span uses.
3. **The day headers follow the span.** One column in day view, seven in week, correctly
   dated, with today still marked.
4. **Events land in the right column at the right height in every span.** The same event
   appears at the same clock position whether the grid shows one day or seven.
5. **An event spanning midnight still renders on both days**, in every span that contains
   both — and contributes only its visible part when the span clips it.
6. **The all-day row is a fixed band in every span**, sized to the same columns. It keeps
   its height when the day has nothing all-day, rather than collapsing: 26px of grid is
   cheaper than an hour axis that jumps every time the user steps to a day whose all-day
   content differs — and with a holiday calendar connected, that is most days.
7. **The current-time line appears only when today is on screen**, in the right column.
8. **The chosen span survives a restart.** Kept in its own `view-state` file beside the
   database, *not* `settings.toml`: that file is hand-written and may carry comments, and
   rewriting it on every span click would destroy them. This is last-used state, closer to
   window geometry than to a preference. Changed 2026-09-14 during T3.
9. **A week containing a DST transition still shows the right number of columns**, in every
   span, and the hour axis stays sane.
10. **Quiet hours are compressed to half height.** Hours outside a configurable core band —
    `core_hours_start` and `core_hours_end` in `settings.toml`, defaulting to 08:00 and
    22:00 — render at half the usual row height, so a day is graspable without scrolling.
11. **An hour containing an event renders at full height**, even outside the core band, and
    it does so **for every column at once**. See the constraint below.

## The vertical mapping

This is the part of the module everything else inherits, so it is stated rather than left to
implementation.

Time currently maps to pixels linearly: `y = minutes / 60 × HOUR_HEIGHT`. Criteria 10 and 11
make that **piecewise and data-dependent**, and three things follow.

**One mapping for the whole grid.** The hour axis is a single column shared by every day, so
it cannot claim 06:00 is tall for Monday and short for Tuesday. An hour expands if *any*
visible day has an event in it, and then expands for all columns.

**The consequence is accepted deliberately.** In day view this is exactly right. In week
view, one early meeting anywhere in the week expands that hour for the whole week — and with
four accounts connected, most weeks will have something before 08:00, so compression will
often be inert there. Decided 2026-09-14: one coherent rule beats a rule that changes with
the span.

**Height is never `duration × scale`.** An event from 07:00 to 09:00 crosses the boundary, so
its height is `y(end) − y(start)` computed through the same mapping. Anything else is wrong
exactly at the boundary, which is where the 07:30 meeting the user actually cares about
lives.

`layout.rs` is unaffected: it returns minutes and stays a pure function of the span. Only the
grid converts minutes to pixels, and `y` takes the occupied-hour set as an argument rather
than reading widgets — so it stays a pure function and is unit-tested like the rest.

## Commands

Unchanged from `SPEC.md` §5 — `cargo build`, `cargo test`, `cargo clippy --all-targets`,
`cargo fmt`. `RUST_LOG=agenda=debug cargo run` for the grid's own debug output.

## Project structure

```
src/ui/week.rs     → becomes the parameterised grid; likely renamed src/ui/timegrid.rs
src/ui/layout.rs   → unchanged; already takes a day count
src/ui/mod.rs      → the query window follows the span instead of a constant 7
src/config.rs      → the remembered span and the core hour bounds, in settings.toml
```

No new module directory. A second file appears only if the switcher grows past a few widgets.

## Code style

`SPEC.md` §7 unchanged: `//!` and `///` docs recording decisions and traps rather than
restating signatures, `anyhow::Result` with `.context(...)`, typed errors only where a caller
branches, `#[cfg(test)] mod tests` at the foot of the file, and test names that read as
assertions.

## Testing strategy

`SPEC.md` §8 unchanged — GTK code is exempt from unit tests and verified by running, and
everything outside it carries tests.

The thing that makes this module testable is that the arithmetic is already outside the
widgets. `layout::segments` and `layout::columns` are pure and take the span as an argument,
so criteria 4, 5 and 9 are unit tests, not screenshots:

- the same event in spans of 1, 3, 5 and 7 lands at the same `top_minutes`
- an event spanning midnight yields two segments in a span containing both days, and one
  when the span clips it
- a span crossing the Madrid DST transition still yields the expected column count, each day
  measured against its own midnight

`y(minutes, occupied_hours, core_bounds)` is pure and carries its own tests:

- monotonic across the whole day, so nothing ever renders above an earlier event
- an hour outside the core band with no event is half height; with one, full height
- hours inside the core band are always full height, occupied or not
- an event from 07:00 to 09:00 has height `y(09:00) − y(07:00)`, not two hours of scale
- the total height of a day with no events outside the core band is the expected saving

Criteria 1, 2, 3, 6 and 7 are visual and verified by running the application.

## Boundaries

**Always**
- Keep the grid one widget. A second implementation for day view is the failure this module
  exists to prevent.
- Keep the arithmetic in `layout.rs`, pure and span-agnostic, so it stays testable.
- Leave `SPEC.md` §4's user-owned columns alone; nothing here touches the store's schema.

**Ask first**
- Any new dependency (`SPEC.md` §9).
- Any change to `settings.toml`'s shape beyond adding the remembered span and the two core
  hour bounds.
- Renaming `week.rs` — cosmetic, but it moves a file every other module's spec references.

**Never**
- Add event creation, editing or dragging. v1 is read-only and v2 has not changed that.
- Weaken or delete a test to make a span pass.
- Reach for the keyring (`SPEC.md` §9, added 2026-09-13).

## Open questions

1. ~~**Which multi-day spans?**~~ **Resolved:** a fixed set of 1, 3, 5 and 7. No arbitrary
   N, no control to cap.
2. ~~**Where does the switcher live?**~~ **Resolved:** the header bar, beside the
   previous/next/today controls. `shell` must leave that corner alone when it moves the
   account actions into the sidebar.
3. ~~**Per-window or global persistence?**~~ **Assumed:** global, in `settings.toml`.
   `tray-day` keeps its own fixed span of 1 regardless.
4. ~~**Today first or centred?**~~ **Resolved:** see the table above. The rules are
   per-span, not general.
5. ~~**Does day view keep the all-day row's height?**~~ **Resolved 2026-09-14** after
   comparing both: a fixed band. See criterion 6.
