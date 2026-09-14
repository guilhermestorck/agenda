# Spec: `lead-in`

*Module of [SPEC-ui-map.md](SPEC-ui-map.md). Depends on `views-timegrid` and `notify`. Time
grid only — month and agenda show nothing. `SPEC.md` (v1) still holds.*

## Objective

Show when a reminder will fire, as a translucent band running from the fire time to the
event's start. The user can see at a glance how much warning they will get, which is
currently invisible until the notification arrives.

**The one real change is to `Item`.** It carries summary, times, colours, account and picture
— and nothing about reminders. `notify::lead_minutes(event, calendar, account, global)`
already resolves the cascade; `collect_items` already resolves the colour cascade in exactly
the same place. The lead joins it there: one field on `Item`, one resolution site, no second
copy of the cascade.

## Success criteria

1. **An event with a reminder draws a band** from `start − lead` to `start`, in the event's
   calendar colour at reduced opacity.
2. **An event with no reminder draws nothing.** Silence is the default and must look like it.
3. **The band uses the same resolved lead the notification will use** — event, then calendar,
   then account, then global. If the two disagree, the band is lying.
4. **The band obeys the vertical mapping.** Its height is `y(start) − y(start − lead)`, not
   `lead × scale`, so a lead crossing 08:00 is drawn correctly.
5. **A band is clipped at the top of the visible day** rather than overflowing, and a lead
   reaching into the previous day shows only the part in view.
6. **The band never widens the event's column** or affects overlap packing — it is painted
   behind, and `layout::columns` does not see it.
7. **All-day events draw no band**, whatever their reminder — they have no start edge on the
   hour axis. `notify` already gives them an `all_day_hour`; the grid does not.

## Commands / style / testing

Unchanged from `SPEC.md` §5, §7, §8. Criterion 3 is the one that matters and is testable
without GTK: the lead the grid resolves and the lead `notify` schedules come from the same
call, asserted against a fixture with all four cascade levels set.

## Boundaries

**Always** — resolve the lead through `notify::lead_minutes`, once, in `collect_items`.
**Ask first** — any new field on `Item` beyond the resolved lead.
**Never** — a second copy of the cascade; a band on a view with no hour axis.

## Open questions

1. **How transparent?** Proposed: the calendar colour at ~25% alpha, tested in both light and
   dark — a value that reads in one may vanish in the other.
2. **Does it fade, or is it flat?** The user's original phrasing was "fading away till the
   notification start time". A gradient is more literal; a flat wash is cheaper and survives
   a theme change more predictably. Proposed: flat, revisit once it is on screen.
