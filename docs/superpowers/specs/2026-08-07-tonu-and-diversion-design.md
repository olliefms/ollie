# TONU and Diversion — design

**Date:** 2026-08-07
**Status:** approved, ready for implementation planning

## Problem

Two real operational outcomes have no legal representation in the trip state machine, and
both leave money on the table or corrupt the record when they happen.

**TONU (Truck Ordered Not Used).** A load is dispatched, the driver runs to the shipper,
and while waiting to load is told the load is cancelled. The truck was ordered and it
rolled: there is real deadhead mileage the driver must be paid for, likely real detention
for the wait, and there is usually revenue — the broker pays a TONU fee even though nothing
moved.

**Diversion.** A load is cancelled, reconsigned, or corrected *after* the driver departs
the shipper, with freight on the truck. The driver pulls over somewhere safe and awaits
instructions; the freight then goes back to the shipper, to a cross-dock or relay, or to a
new consignee the broker nominates.

Today neither is expressible:

- `trip_lifecycle::cancel` refuses `in_transit` and `delivered`
  (`src/services/trip_lifecycle.rs:329`), and `TripStatus::can_transition_to`
  (`src/models/trip.rs:144`) has no edge out of `InTransit` except `Delivered`. A driver who
  departed and arrived at the shipper is **stuck** — the only escape is faking a delivery.
- Cancelling from `Dispatched` releases equipment and demotes the load to `planned`. The
  trip's deadhead is then stranded on a cancelled record, and reclassifying the load to
  `administrative` requires it to be trip-free.

### What already exists, and the boundary against it

`LoadKind::Administrative` (#411, v2.7.0) was built partly for TONU and AGENTS.md names it
explicitly. It models the **paper** TONU: no truck ever rolled, so the load invoices
straight from `planned` with no trip, no miles, no driver pay.

The boundary must be stated, or the wrong one gets picked later:

| | Truck ordered? | Trip | Invoices from |
|---|---|---|---|
| `LoadKind::Administrative` | never ordered, or cancelled before dispatch | none | `planned` |
| `LoadStatus::Tonu` | ordered and rolled | yes — real deadhead, driver pay, detention | `tonu` |

## The lifecycle boundary

The existing state machine already partitions the lifecycle by where the freight is.
`cascade_start_in_transit` (`src/services/trip_stops.rs:181`) promotes
`Dispatched -> InTransit` on **departure from the first `Pickup`**. No new boundary is
needed:

| Trip status | Freight | Cancellation is |
|---|---|---|
| `planned` / `assigned` | not ordered yet | plain `cancel` (exists) |
| `dispatched` | not on the truck — rolling to, or waiting at, stop 0 | **TONU** |
| `in_transit` | **on the truck** — departed stop 0 | **diversion** |
| `delivered` / `completed` | off the truck | terminal |

`dispatched` covers both TONU sub-cases — never arrived, and arrived-and-waiting — because
recording `actual_arrive` at the pickup does not move the status. Only departure does.

## Model

### New enum variants

```
TripStatus::Tonu       terminal.  Dispatched -> Tonu.  No edge out.
TripStopType::Hold     "truck stopped here awaiting instructions" — not a planned service stop.
LoadStatus::Tonu       Assigned|Dispatched -> Tonu -> Invoiced -> Settled.  Also Tonu -> Cancelled.
```

`Tonu -> Cancelled` exists because a broker may ultimately refuse to pay anything, and a $0
invoice is the wrong place to put that.

The load-side entry edge is deliberately looser than the trip-side gate (`Assigned` as well
as `Dispatched`) because load status is a best-effort denormalization whose cascades
log-and-swallow. The trip's status is the real gate.

`TripStatus::Tonu` is **not** delivery-complete, so `load_trips_all_delivered` keeps
returning false and a TONU'd leg can never drag a load to `Delivered`.

Diversion introduces no new status. It is a plan mutation on a trip that keeps running.

### New persisted fields

Four, all on `LoadRecord`. `TripRecord` needs none — `Tonu` and `Hold` are values in enums
already persisted as strings.

- `quoted_rate_items: Vec<RateLineItem>` — where linehaul + FSC go when TONU clears
  `rate_items`.
- `diverted_at: Option<String>`
- `diversion_reason: Option<String>` — the enum value (`diverted` | `reconsigned`), so
  "which loads were diverted, and why" is a filter rather than a text search.
- `diversion_notes: Option<String>` — the dispatcher's free text.

All four default empty/`None` so existing rows deserialize unchanged.

TONU adds no field for its free-text explanation. It reuses the load's existing
`cancellation_reason`, whose documented meaning widens from "why this load was cancelled" to
"why this load ended without delivery" — the two terminal-without-freight outcomes,
`cancelled` and `tonu`.

`diverted_at` deliberately does **not** carry the original destination. Because
`divert_trip` never rewrites the *load's* stops, the contracted delivery facility is still
recorded there; and because a diverted trip's new stops get `load_stop_index: None`,
`cascade_load_stop_arrive` never fires on them, so the contracted stop keeps
`actual_arrive: None` forever — which is exactly true: we never delivered there. **The load
documents what was sold; the trip documents what happened.**

## `tonu_trip(trip_id, { hold?, occurred_at?, reason? })`

Gate: `status == Dispatched`. The rejection names the correct verb — `planned`/`assigned`
say "use cancel_trip", `in_transit` says "use divert_trip". Also rejected when the trip has
a `settlement_ref`; miles and pay are frozen, the same gate `recalculate_miles_handler`
uses.

`hold` is a position — `facility_id`, or `facility_name` + `address` + `timezone` — resolved
to a facility through the same resolve-or-create + geocode path `StopInput` already uses.
**All resolution happens before any write**, so a geocode failure cannot leave a
half-TONU'd trip.

1. **Stops.** The last stop with an `actual_arrive` is the truncation point; everything
   after it is dropped. If no stop was reached, `hold` is **required** (422 otherwise) and
   replaces the stop list entirely. `hold` is also *allowed* alongside a reached stop and is
   appended — a driver released from the dock and sent to park elsewhere really did drive
   those miles.
2. **Release time.** If the truncation stop has an `actual_arrive` but no `actual_depart`,
   TONU sets `actual_depart = occurred_at`. This is what makes detention pay work: the
   driver sat at that dock from `actual_arrive` until he was released, and
   `compute_driver_pay` reads exactly that interval. Without it the wait is unpaid, which
   defeats half the point of the feature. `occurred_at` is a naive local datetime in the
   stop's own timezone, validated with `validate_stop_time_str`, and defaults to now in that
   timezone.
3. **Mileage.** Route the truncated waypoints, then assign the whole figure to
   `deadhead_miles` with `loaded_miles = None`. See "Loaded/deadhead misclassification"
   below.
4. **Load.** When `count_load_holding_trips(load_id) == 0`, load -> `tonu`; `rate_items`
   move to `quoted_rate_items` and `rate_items` is cleared; `reason` lands in
   `cancellation_reason`.
5. **Equipment** released to `Available` using `complete`'s `resource_on_other_active_trip`
   guard. **No** `try_auto_dispatch_next_for_driver`.
6. Emit `trip.tonu` and the load status-change event.

### Why no auto-dispatch

A normal delivery runs `try_auto_dispatch_next_for_driver`
(`src/services/trip_stops.rs:263`), rolling the driver onto their next `assigned` trip. That
helper does **not** recompute mileage. After a TONU the truck is not where the plan assumed,
so auto-dispatching would leave the follow-on trip with a deadhead measured from a facility
the truck never reached. The follow-on stays `assigned` until the dispatcher decides:
cancel it, re-point its `previous_trip_id` at the TONU trip and recalculate, or dispatch it
by hand. That decision is the dispatcher's; this design only guarantees the primitives
exist to carry it out.

### Loaded/deadhead misclassification

`compute_trip_mileage` classifies a leg as deadhead **only** when it originates from a
`previous_trip_id`; every other leg is loaded. A trip beginning at an explicit
`TripStopType::Origin` stop therefore already pays its empty run to the shipper at the
*loaded* rate — pre-existing, and mostly invisible as one leg among many.

On a TONU it stops being invisible: that empty run is the *entire* trip, so the trip would
pay 100% of its miles at the loaded rate. TONU therefore does not delegate the split. It
asserts the invariant directly: **a TONU trip has zero loaded miles by construction.**

## `divert_trip(trip_id, { hold?, stops, reason, notes? })`

Gate: `status == InTransit`, and no `settlement_ref`.

`reason` is `diverted` | `reconsigned` | `bol_correction`. The mechanism — validate, swap the
undeparted stops, recompute miles, audit — is identical across all three; only the
commercial consequence differs.

- **`diverted` / `reconsigned`** — the broker cancelled or renominated mid-transit. The load
  is flagged `diverted_at` / `diversion_reason` / `diversion_notes`, because there is a fee
  to negotiate.
- **`bol_correction`** — the BOL disagreed with the rate confirmation and the decision is to
  follow the BOL. Emphatically **no** `diverted_at`: nothing was diverted, the plan was wrong
  from the start. Flagging it would poison the exact query the field exists to answer.

`hold` is **optional and independent of `reason`**. Its presence means one thing only: the
truck physically stopped somewhere unplanned. A broker who reconsigns while the driver is
rolling produces a `reconsigned` divert with no `hold`; a BOL dispute that puts the driver in
a truck stop for three hours produces a `bol_correction` *with* one. Inferring the commercial
fact from the physical one would misfile both.

Algorithm:

1. Stops with an `actual_arrive` are kept as immutable history. Everything after is replaced
   by `[hold?] ++ stops`, resequenced from 0, all with `load_stop_index: None`. Attempting
   to replace an arrived-at stop is a 422 telling the caller to clear its actuals first.
2. All positions resolved to facilities before any write.
3. `stops` may be **empty** — "pulled over, disposition unknown". The trip then has no
   delivery to complete against, which is the honest representation. The dispatcher appends
   the destination later.
4. Miles recompute best-effort with a warning.
5. `rate_items` are left alone in all three cases; unlike TONU, the linehaul is at least
   partly earned.
6. Trip status is unchanged.

This is the safe version of something already possible and silently dangerous: `update_trip`
accepts a whole `stops` array with no status guard, so an in-transit trip's stops can be
hand-edited today with no mileage recompute, no audit trail, and no record of the original
destination.

### Cascade guard this requires

`cascade_final_stop_delivered` promotes a trip to `Delivered` when the **max-sequence** stop
is departed. After a `hold`-only divert the `Hold` *is* the max-sequence stop, so the driver
departing the truck stop would silently mark the load delivered. `Hold` must be excluded
from that cascade.

## Surfaces

Both verbs are thin wrappers over `services/trip_lifecycle.rs`, shaped like the existing
`cancel` / `complete` / `dispatch` verbs, so REST and MCP share one implementation.

- REST: `POST /fleet/api/v1/trips/{id}/tonu`, `POST /fleet/api/v1/trips/{id}/divert`
- MCP: `tonu_trip`, `divert_trip` — scope `trips:write`, registered in the scope map
  (`src/api/fleet_portal/mcp.rs:286`), the tool list, and the dispatch match arm.

No new scopes. **Fleet SPA actions are out of scope** — backend and MCP only, so no static
changes and no `?v=` cache-stamp bump at release. SPA buttons become a follow-up issue, as
the admin-load create UI did after #411.

Driver portal is out of scope: the driver reports arrive/depart; TONU and diversion are
dispatcher judgments made after a conversation with a broker.

## Errors

| Status | Cause |
|---|---|
| `404` | trip not found |
| `409` | wrong trip status — message names the correct verb |
| `409` | trip has a `settlement_ref`; miles and pay are frozen |
| `422` | `hold` required (no stop reached) but absent |
| `422` | a `hold` or replacement stop position cannot be resolved to a facility |
| `422` | attempt to replace a stop that has an `actual_arrive` |

ORS failure is deliberately **not** fatal. A TONU is an operational fact that must be
recordable with ORS down; the response carries a `mileage_recompute_warning` and
`recalculate_trip_miles` fixes it later. This is the degradation `apply_trip_patch` already
uses.

## Migration and startup

- Three new nullable/defaulted load columns, following the existing column-add pattern in
  `src/db/mod.rs`.
- `list_loads_needing_routing` selects on `miles IS NULL AND status NOT IN (terminal)`. If
  `tonu` is not added to that terminal set, every TONU'd load re-enters the routing requeue
  on **every startup, forever** — the silent permanent zombie AGENTS.md documents from #411.
- `load_doctor` must not flag a `tonu` load as status-mismatched; `trip_doctor` must treat
  `Tonu` as terminal.

## Tests

**Model-level.** Transition tables for `TripStatus::Tonu` and `LoadStatus::Tonu`;
`is_delivery_complete(Tonu) == false`; `load_trips_all_delivered` with a `Tonu` sibling;
string roundtrips for `Tonu` and `Hold`.

**Detention.** A TONU on a trip whose truncation stop has an `actual_arrive` and no
`actual_depart` sets the depart to `occurred_at`, and the resulting `driver_pay_snapshot`
bills the wait beyond `free_dwell_minutes` at the detention rate.

**`tests/tonu_test.rs`.** Arrived-at-shipper truncation — deadhead equals the full route,
`loaded_miles` is `None`, equipment returns to `Available`, load is `tonu` with `rate_items`
empty and `quoted_rate_items` populated. Never-arrived with `hold`. Never-arrived without
`hold` -> 422. Wrong-status rejections in both directions. `tonu -> invoiced -> settled`.
`tonu -> cancelled`. Settled-trip rejection. Multi-leg TONU with a live sibling leaves the
load alone.

Two of these carry the design:

- **Chain origin.** Create trip B with `previous_trip_id` = the TONU'd trip and assert B's
  deadhead origin resolves to the shipper (or the `Hold`), not the phantom delivery. This is
  the regression test for the entire reason we truncate.
- **No auto-dispatch.** With trip B `assigned` to the same driver, a TONU on A leaves B
  `assigned`.

**`tests/divert_test.rs`.** Divert with hold + destination — stops become
`[kept pickup, Hold, new delivery]`, `diverted_at` set, `rate_items` untouched.
`bol_correction` sets no `Hold` and no `diverted_at`. Empty-`stops` divert followed by
departing the `Hold` does **not** mark the trip delivered. Replacing an arrived-at stop
-> 422. `dispatched` -> 409 naming `tonu_trip`. Trip completes normally to the new
destination and the load invoices.

**Regression.** `load_doctor` clean on a `tonu` load; `tonu` load excluded from
`list_loads_needing_routing` (mirroring the #411 zombie test); migration test that
pre-existing load rows deserialize with the three new columns defaulted.

## Explicitly out of scope

- **Deadhead from home.** A trip with no `previous_trip_id` and no `Origin` stop has no
  waypoint before the shipper, so there is no deadhead figure to pay. Pre-existing for every
  trip, not introduced here, but it caps what TONU can pay a driver who started from home.
- **A flat driver TONU fee.** `DriverPay` has slots for loaded, deadhead, extra-stop and
  detention only. Pay here is deadhead + detention, per the stated requirement. A fixed TONU
  amount would be a new pay component and separate work.
- Fleet SPA actions for either verb.
- Driver PWA changes.
