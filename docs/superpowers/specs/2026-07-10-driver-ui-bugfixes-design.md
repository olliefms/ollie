# Driver UI bugfixes — trailer picker scroll/typeahead + stop arrival clear/sequencing

Date: 2026-07-10

Two driver-PWA bugs reported from the field:

1. The equipment (trailer) picker list can grow taller than the viewport; the page
   does not scroll, stranding the **Update** button below the fold so a selection
   can't be saved.
2. Stop arrival times can't be cleared or corrected once entered, and can be
   entered out of sequence (arriving on a not-yet-dispatched trip, or arriving at a
   later stop before completing an earlier one).

## Bug #1 — Trailer equipment picker

### A. Scroll fix (root cause)

`.equipment-page` is a flex child of `.page-with-nav` (`position: fixed; inset: 0;
flex-direction: column`) but — unlike the sibling `.account-body` — lacks
`flex: 1; overflow-y: auto; min-height: 0`. When the truck card + trailer chips +
picker exceed viewport height, nothing scrolls and the submit button is
unreachable.

**Fix:** add `flex: 1; overflow-y: auto; min-height: 0` to `.equipment-page`.

### B. Typeahead redesign (`buildTrailerSection`, `static/driver/pages/equipment.js`)

Replace the always-visible checkbox list with a type-to-complete control:

- Text input; on input (≥1 char) show a short, capped (~8) suggestion dropdown of
  trailers matching by `unit_number` or `owner_name`.
- Tapping a suggestion adds it as a **removable chip**. Multi-select preserved.
  Currently-attached trailers pre-populate as chips on load.
- No match for the typed value → dropdown offers **"Hook new trailer '<X>'"**,
  adding a pending-new chip (marked "· new"). Preserves the create-on-submit path.
- Suggestions already chosen (in the chip set) are excluded from the dropdown.
- Submit payload unchanged: `trailer_ids` for known selections, or
  `trailer_unit_numbers` when any pending-new units are present. Success and
  `trip_cascade` messaging unchanged.

New CSS: `.trailer-typeahead`, `.trailer-suggestions` (short positioned dropdown),
`.trailer-chips`, `.trailer-chip`. Because the dropdown is capped and short, the
tall-list overflow cannot recur.

## Bug #2 — Stop arrival: clear + sequencing

### A. Clear support (escape hatch)

`UpdateStopTimesRequest` (`src/api/driver_portal/data.rs`) gains
`clear_arrive: bool` and `clear_depart: bool` (both `#[serde(default)]` false) —
explicit booleans avoid serde's null-vs-absent ambiguity on `Option<String>`.

Semantics:
- `clear_arrive` clears **both** arrival and departure (departure requires an
  arrival) and cascades the clear to the linked load stop.
- `clear_depart` clears departure only and cascades.

New DB method `DbClient::clear_trip_stop_times(id, seq, clear_arrive, clear_depart)`
sets the fields to `None` via the existing `upsert_trip` merge_insert path.
New service helper `cascade_load_stop_clear(...)` mirrors the existing set
cascades. Clear is exposed through the shared `trip_stops` service for symmetry,
but only the driver handler calls it here.

Frontend (`static/driver/pages/stop-detail.js`, `renderActualLine`):
- Add a **trash-can (🗑) icon button** next to the ✎ pencil on any recorded
  Arrived/Departed line → clears that entry (arrive clears both; depart clears
  depart). Inline confirm (button flips to a confirm state) — no JS `confirm()`
  dialog.
- Fix the native picker Clear: the `datetime-local` `change` handler currently
  does `if (!dt.value) return;`, silently swallowing the browser picker's built-in
  **Clear**. Change so an emptied value routes to the same clear path.
- Both surfaces call a new `onClear` callback → PATCH with `clear_arrive` /
  `clear_depart`.

### B. Sequencing guards — driver portal only, NEW arrivals only

In `update_stop_times`, when the request sets an arrival on a stop that currently
has none (`req.actual_arrive.is_some() && existing.actual_arrive.is_none()`),
enforce via a pure, unit-testable `check_arrival_allowed(trip, seq) -> Result<(),
AppError>`:

1. Trip status must be `Dispatched` or `InTransit` → else 422
   "trip not yet dispatched to you."
2. Every stop with `sequence < seq` must have `actual_depart` set → else 422
   "complete the previous stop first."

Guards are **skipped** when editing an existing arrival (stop already had
`actual_arrive`) or when clearing — this is the correction escape hatch that
unsticks a mistake. Departure keeps existing rules (requires same-stop arrival,
`depart >= arrive`); no new depart sequencing needed since the arrival guard
already gates on prior-stop departure.

Guards live in the driver handler, **not** the shared `trip_stops` service, so
fleet-side backfill/correction stays unrestricted.

### Scope note — status is not auto-rewound

Clearing a time does **not** reverse trip/load status transitions (e.g. an
`InTransit` trip whose pickup departure is cleared stays `InTransit`). Cascading
status reversal is risky and out of scope; clearing is treated as a data
correction, with status adjusted fleet-side if needed.

## Testing

- Pure-fn `check_arrival_allowed` tests: reject new arrival when trip `Assigned`;
  reject stop-2 arrival before stop-1 depart; allow when dispatched + prior
  departed; (guard-skip paths are exercised at the handler level by construction).
- DB/service test: `clear_trip_stop_times` clears fields; `clear_arrive` clears
  both arrive and depart.
- Frontend: match existing driver test style where equipment/stop-detail coverage
  exists; keep parity with the current node:test suite (Vitest migration deferred).

## Files touched

- `static/driver/css/components.css` — `.equipment-page` scroll; trailer chip/typeahead + trash-can styles.
- `static/driver/pages/equipment.js` — typeahead + chips picker.
- `static/driver/pages/stop-detail.js` — trash-can clear, native-picker clear fix, `onClear` wiring.
- `src/api/driver_portal/data.rs` — request fields, `check_arrival_allowed`, clear routing.
- `src/services/trip_stops.rs` — clear + `cascade_load_stop_clear`.
- `src/db/trip_ops.rs` — `clear_trip_stop_times`.
