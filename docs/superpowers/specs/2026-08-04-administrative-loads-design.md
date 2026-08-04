# Administrative (no-trip) loads

**Date:** 2026-08-04
**Status:** approved, ready for implementation plan

## Problem

Ollie's load status machine has no path from `planned` to `invoiced`/`settled` that
does not run through a trip. `LoadStatus::can_transition_to` allows only
`Delivered → Invoiced → Settled`, and every route to `Delivered` is a trip-side
cascade (`dispatch_trip` → `stop_arrive`/`stop_depart` → `complete_trip`).

That is correct for freight. It is wrong for revenue that was never driven. The
motivating case is a weekly revenue guarantee: it arrives as a real freight bill and
pays real money, but no truck moved. Such loads sit in `planned` permanently, so
status-based queries ("what is unsettled?") return false positives every week even
though the money was collected and posted.

The same shape covers TONU, detention-only billing, layover pay, and
lumper/accessorial-only freight bills.

Reaching `settled` today would require inventing a trip, a driver assignment, a
truck, and fabricated stop actuals for a move that never happened — putting false
mileage, false HOS-adjacent events, and a false driver assignment into the
operational record. That is not an acceptable workaround.

Affected records at time of writing (all `planned`):

| Load # | Ollie id | Note |
|---|---|---|
| 4581461 | `c8004007-ff0d-4133-a1dd-7dd86c0b4d48` | paid 2026-07-29, check 3541257 |
| 4581461 | `73a96e31-e760-4180-bb8b-97456bcdb114` | duplicate created 2026-07-22, holds the signed SBOL blob |
| 7379930 | `4f0338d8-39a5-4ea7-a399-e04d348c5061` | week of 2026-07-27, rate pending |

## Findings from codebase investigation

Three facts discovered during design that shape the work:

1. **`load_doctor` needs no change.** `src/services/doctors/load.rs` checks only
   facility geocoding, scheduled windows, actual ordering, timezones, and rate sums.
   It has no trip-related checks, and every stop check iterates `load.stops` and
   no-ops on an empty list. There is nothing to relax.

2. **These loads are re-queued for ORS routing on every startup.**
   `list_loads_needing_routing` (`src/db/load_ops.rs:259`) selects
   `miles IS NULL AND status NOT IN ('delivered','invoiced','settled','cancelled')`.
   All three guarantee loads match, permanently. This fix must also close that.

3. **The `loads` table has no schema-migration path.** It is opened through the
   generic `open_or_create` (`src/db/mod.rs:68`), unlike `fleet_users` and `trips`
   which have dedicated functions with `add_columns`. Adding any new load column
   requires building that migration first, or every existing install breaks on read.
   This is the largest hidden cost in the change.

## Design

### 1. Data model

Add `LoadKind` to `src/models/load.rs`, following the existing `StopType` /
`ServiceType` idiom (`as_str`, `FromStr`, `#[serde(rename_all = "snake_case")]`):

```rust
pub enum LoadKind { Freight, Administrative }
```

- `LoadRecord.kind: LoadKind`, `#[serde(default)]` → `Freight`
- `CreateLoadRequest.kind: Option<LoadKind>` (absent → `Freight`)
- `UpdateLoadRequest.kind: Option<LoadKind>`
- `kind` surfaced in `LoadListItem` and `LoadDetailResponse`

A kind enum rather than a `requires_trip` bool: this classifies the revenue, which
is what reporting keys off. The no-trip behavior follows from the classification
rather than being the thing itself, and future kinds need no additional field.

### 2. Persistence and migration

`load_schema` gains `Field::new("kind", DataType::Utf8, false)`.

Replace the generic `open_or_create(&conn, "loads", ...)` call with a dedicated
`open_or_create_load`, mirroring `open_or_create_fleet_user` (`src/db/mod.rs:362`).
On the `Ok` branch, when the `kind` field is absent:

```rust
transforms.push(("kind".into(), "CAST('freight' AS string)".into()));
```

Use the SQL keyword type `string`, never the Arrow name `Utf8` — see AGENTS.md.

`load_to_batch` writes the column; `row_to_load` parses it with `unwrap_or(Freight)`
so a row predating the migration cannot fail a read.

### 3. Transition policy

Kind-aware policy lives on the record, so `LoadStatus` stays a pure status machine
and its existing assertions keep passing unchanged:

```rust
impl LoadRecord {
    pub fn can_transition_to(&self, next: &LoadStatus) -> bool {
        if self.kind == LoadKind::Administrative
            && matches!((&self.status, next), (LoadStatus::Planned, LoadStatus::Invoiced)) {
            return true;
        }
        self.status.can_transition_to(next)
    }
}
```

`transition_load_status` (`src/db/load_ops.rs:94`) calls this instead of
`record.status.can_transition_to(...)`.

Administrative loads deliberately do **not** get `Planned → Delivered`. A load that
never moved was never delivered; `invoiced` is the honest first stop.
`Invoiced → Settled` and `Planned → Cancelled` already work and are unchanged.

### 4. Keeping the two models from tangling

- Creating a trip whose `load_id` points at an administrative load returns 422 with
  a message naming the kind. Assign and dispatch guards then follow by construction,
  since no trip can exist against such a load.
- `update_load` may change `kind` only while the load is `planned` **and** has no
  trips (`list_trips_for_load` empty). Otherwise 409. This is what allows the three
  existing guarantee loads to be converted in place rather than recreated.

### 5. Routing

- `list_loads_needing_routing` and `list_unrouted_loads_for_facility` gain
  `AND kind != 'administrative'`.
- `create_load` and `update_load` skip the `routing_tx.try_send` for administrative
  loads.

This closes the permanent startup-requeue described in finding 2.

### 6. Load lifecycle events

There are currently no load status events at all — `src/events/mod.rs` covers trips,
drivers, and equipment only.

Add `events::on_load_status_changed(db, load_id, from, to)`, emitting
`append_event("load", id, "load.{to}", {from, to}, None, now, None)`. It is called
from inside `transition_load_status`, so no code path can skip it.

Fleet surface additions:

- a `"load"` arm in `subject_for` (`src/api/fleet_portal/data.rs:1501`) returning
  `Load {load_number}`
- `load: 'loads'` in `ROUTE_BASE` (`static/fleet/pages/events.js:5`), so the
  "Go to load →" link renders
- a `.badge--load` rule in `static/fleet/css/components.css`

**Volume:** emitting on every transition means a normal freight load adds roughly
five load events alongside its existing trip events. Accepted deliberately: the
load's own history is the thing that is missing, and a partial history is the same
class of defect being fixed one layer down.

**Actor:** events are emitted with `actor: None`. `transition_load_status` has no
actor parameter, and the useful human-vs-cascade distinction belongs to the deferred
`set_load_status` work (see Out of scope). Threading an actor is that issue's job.

### 7. `list_loads` filter passthrough

`tool_list_loads` (`src/api/fleet_portal/mcp.rs:2056`) hardcodes `None` / `&[]` for
customer, tags, from, and to, so those filters are silently ignored. Its tool schema
additionally advertises `facility_id`, which the handler also ignores — an actively
false advertisement.

- Pass through `status`, `customer`, `tags`, `from`, `to`; all are already supported
  by `build_load_filter` (`src/db/load_ops.rs:400`).
- Implement `facility_id` rather than retracting it, as
  `stops LIKE '%"{uuid}"%'` — the technique `list_unrouted_loads_for_facility`
  already uses. Add it to `ListLoadsQuery` as well, for REST and OpenAPI parity.
- Update the tool description to name every filter.

Multiple `tags` are ANDed together, matching the existing `build_load_filter`
behavior.

### 8. Fleet SPA

`static/fleet/pages/load-detail.js:198` becomes:

```js
const canInvoice = hasScope('loads:invoice')
  && (load.status === 'delivered'
      || (load.status === 'planned' && load.kind === 'administrative'));
```

Plus a kind badge on the load detail view.

The load *form* does not gain a kind selector. The form is stop-centric and
reshaping it for stopless loads is a larger UI job than this change warrants.
Creating administrative loads is an MCP/API operation for now. See Follow-ups.

### 9. `stops` relaxation

`CreateLoadRequest.stops` is required at both the serde and MCP-schema level, which
forces a meaningless `stops: []` on every administrative load. Add
`#[serde(default)]` and drop `stops` from the MCP `required` list.

No compensating "freight loads must have at least one stop" validation is added.
None exists today, so adding one could reject flows that currently succeed. The
accepted cost is that omitting stops on a freight load stays silent.

## Testing

- Transition matrix unit tests: administrative `planned → invoiced` allowed; freight
  `planned → invoiced` rejected; administrative `planned → delivered` rejected;
  administrative `invoiced → settled` allowed; administrative `planned → cancelled`
  allowed.
- `kind` round-trips through LanceDB, and a row written without it reads as
  `freight`.
- Tangle guards: creating a trip against an administrative load fails; changing
  `kind` fails once the load is past `planned` or has trips.
- `build_load_filter` coverage for each filter including `facility_id`.
- `tool_list_loads` passes each argument through to the DB layer.
- A status transition emits the corresponding `load.*` event.
- Vitest for the `canInvoice` gate and `ROUTE_BASE.load`.

## Acceptance

Replay the reported sequence against `c8004007-ff0d-4133-a1dd-7dd86c0b4d48`, which
has no trip:

```
update_load(id=..., kind="administrative")
invoice_load(id=..., invoice_number="JQL-4581461", invoice_date="2026-07-29")
settle_load(id=...)
→ load.status == "settled"
```

`list_loads` filtered to unsettled statuses no longer returns the paid guarantee.
`list_loads(tags=["guarantee"])` returns only tagged loads.

## Out of scope

- **Issue #395** — load stranded in `in_transit` when a sibling trip is cancelled or
  completed. Already filed, already in progress, and load 4819063 has already been
  repaired by manual LanceDB surgery.
- **`set_load_status`** — a guarded, forward-only, actor-logged status setter for
  back-office correction. Becomes its own issue stacked on #395; it cannot be added
  to #395 while that work is in flight.
- **Issue #396** — carrier settlement statement entity. Adjacent to the bookkeeping
  motivation but a different record type.
- **`load_doctor`** — needs no change, per finding 1.

## Follow-ups

- Fleet UI for creating and editing administrative loads (kind selector, stopless
  form path). Wanted eventually, explicitly deferred here.
- Threading an actor through `transition_load_status`, as part of the
  `set_load_status` issue.

## Versioning

New MCP and REST surface plus a new persisted field, so this is a minor bump. The
bump itself is owned by `cut-release`, not by this work.
