# TONU and Diversion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a dispatched-but-never-loaded trip end as TONU with its deadhead and detention payable, and let an in-transit trip be re-targeted to a new destination without erasing the miles already driven.

**Architecture:** Two new verbs in `src/services/trip_lifecycle.rs` alongside `cancel`/`complete`/`dispatch`, each surfaced identically over Fleet REST and Fleet MCP. `TripStatus::Tonu` and `LoadStatus::Tonu` are terminal outcomes; diversion introduces no status and instead rewrites the undeparted stops of a trip that keeps running. A new `TripStopType::Waypoint` anchors both to the last position the truck actually reached, so recomputed mileage reflects miles driven rather than miles planned.

**Tech Stack:** Rust, axum, LanceDB (Arrow record batches), utoipa, tokio, `axum_test::TestServer`.

Design spec: [`docs/superpowers/specs/2026-08-07-tonu-and-diversion-design.md`](../specs/2026-08-07-tonu-and-diversion-design.md)

## Global Constraints

- **DCO required.** Every commit uses `git commit -s`. CI enforces it.
- **Never run `cargo fmt`.** The repo is hand-formatted and there is no CI fmt check. Match the surrounding style: compact, 4-space indent, `match` arms on one line where they fit.
- **`cargo clippy` must pass.** CI runs it.
- **LanceDB SQL casts use SQL type keywords** — `string`, `double`, `bigint` — never Arrow names like `Utf8`. This is a recurring bug; see AGENTS.md.
- **New Arrow columns are appended to the END of the schema.** `load_to_batch`, `empty_load_batch` and `row_to_load` build columns positionally; inserting mid-list silently corrupts every field after it.
- **Cascades are best-effort.** Load-status and resource-status updates in `trip_lifecycle` log failures via `tracing::warn!` and do not fail the caller's operation.
- **Integration tests have no ORS.** `tests/*` construct `RoutingClient::new("")`, so every mileage computation fails. Verbs must degrade to a warning, never an error, and tests must assert on stop structure rather than mile counts.
- **No fleet SPA changes.** Backend and MCP only; static assets are untouched so no `?v=` cache-stamp bump is needed at release.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `src/models/trip.rs` | `TripStatus::Tonu`, `TripStopType::Waypoint`, `is_service_stop()` | 1 |
| `src/models/load.rs` | `LoadStatus::Tonu`, four new `LoadRecord` fields | 1, 3 |
| `src/models/pay.rs` | `PayStopInput.is_service_stop`, extra-stop count filter | 2 |
| `src/api/fleet_portal/data.rs` | pay read path; REST handlers for both verbs | 2, 5, 6 |
| `src/db/mod.rs` | load schema + migration + empty batch | 3 |
| `src/db/load_ops.rs` | load record ↔ batch; routing requeue filter | 3, 4 |
| `src/services/trip_lifecycle.rs` | `tonu`, `divert`, shared position resolution | 5, 6 |
| `src/services/trip_stops.rs` | exclude `Waypoint` from the delivered cascade | 6 |
| `src/events/mod.rs` | `on_trip_tonu`, `on_trip_diverted` | 5, 6 |
| `src/api/fleet_portal/mod.rs` | route registration | 5, 6 |
| `src/api/fleet_portal/mcp.rs` | tool schema, scope map, annotations, dispatch | 5, 6 |
| `tests/tonu_test.rs` | TONU integration coverage | 5 |
| `tests/divert_test.rs` | diversion integration coverage | 6 |
| `AGENTS.md` | new invariants | 7 |

---

### Task 1: Enum variants and state transitions

Pure model work. No I/O, no persistence — this task only teaches the state machines that the new outcomes exist.

**Files:**
- Modify: `src/models/trip.rs` (`TripStopType`, `TripStatus`)
- Modify: `src/models/load.rs` (`LoadStatus`)
- Test: same files, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `TripStopType::Waypoint`, string form `"waypoint"`
  - `TripStopType::is_service_stop(&self) -> bool`
  - `TripStatus::Tonu`, string form `"tonu"`
  - `LoadStatus::Tonu`, string form `"tonu"`

- [ ] **Step 1: Write the failing tests**

In `src/models/trip.rs`, inside `mod tests`, replace the body of `test_trip_stop_type_roundtrip` and `test_trip_status_transitions`, and add two tests:

```rust
    #[test]
    fn test_trip_stop_type_roundtrip() {
        for s in ["origin", "fuel", "pickup", "delivery", "relay", "empty_move",
                  "maintenance", "terminal", "waypoint"] {
            let t: TripStopType = s.parse().unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn test_is_service_stop() {
        for t in [TripStopType::Pickup, TripStopType::Delivery,
                  TripStopType::Relay, TripStopType::EmptyMove] {
            assert!(t.is_service_stop(), "{t:?} is freight work");
        }
        // An empty move counts: it is a dispatched movement with its own BOL and
        // POD whose commodity happens to be nothing.
        for t in [TripStopType::Origin, TripStopType::Fuel, TripStopType::Maintenance,
                  TripStopType::Terminal, TripStopType::Waypoint] {
            assert!(!t.is_service_stop(), "{t:?} affects mileage but is not freight work");
        }
    }

    #[test]
    fn test_tonu_is_terminal_and_reachable_only_from_dispatched() {
        assert!(TripStatus::Dispatched.can_transition_to(&TripStatus::Tonu));
        for s in [TripStatus::Planned, TripStatus::Assigned, TripStatus::InTransit,
                  TripStatus::Delivered, TripStatus::Completed, TripStatus::Cancelled] {
            assert!(!s.can_transition_to(&TripStatus::Tonu), "{s:?} must not reach tonu");
        }
        for s in [TripStatus::Planned, TripStatus::Assigned, TripStatus::Dispatched,
                  TripStatus::InTransit, TripStatus::Delivered, TripStatus::Completed,
                  TripStatus::Cancelled] {
            assert!(!TripStatus::Tonu.can_transition_to(&s), "tonu is terminal, not -> {s:?}");
        }
        // A TONU'd trip delivered nothing.
        assert!(!TripStatus::Tonu.is_delivery_complete());
    }

    #[test]
    fn test_tonu_leg_blocks_the_load_delivery_cascade() {
        use TripStatus::*;
        // Unlike Cancelled, a Tonu leg is a live outcome that must hold the load
        // back rather than being filtered out as a dead record.
        assert!(!all_delivered(&[Tonu]));
        assert!(!all_delivered(&[Delivered, Tonu]));
        assert!(!all_delivered(&[Cancelled, Tonu]));
    }
```

Also extend the existing `test_trip_status_roundtrip` string list with `"tonu"`.

In `src/models/load.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn test_load_tonu_transitions() {
        assert!(LoadStatus::Assigned.can_transition_to(&LoadStatus::Tonu));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::Tonu));
        // in_transit means freight is aboard — that is diversion territory.
        assert!(!LoadStatus::InTransit.can_transition_to(&LoadStatus::Tonu));
        assert!(!LoadStatus::Planned.can_transition_to(&LoadStatus::Tonu));

        assert!(LoadStatus::Tonu.can_transition_to(&LoadStatus::Invoiced));
        // A broker who ultimately pays nothing needs somewhere to put it that
        // is not a $0 invoice.
        assert!(LoadStatus::Tonu.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Tonu.can_transition_to(&LoadStatus::Delivered));
        assert!(!LoadStatus::Tonu.can_transition_to(&LoadStatus::Planned));
        assert!(!LoadStatus::Tonu.can_transition_to(&LoadStatus::Settled));
    }
```

And extend `test_load_status_roundtrip`'s list with `"tonu"`.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib models:: 2>&1 | tail -30
```

Expected: compile errors — `no variant named Waypoint`, `no variant named Tonu`, `no method named is_service_stop`.

- [ ] **Step 3: Add the trip enum variants**

In `src/models/trip.rs`, add `Waypoint` to `TripStopType`:

```rust
pub enum TripStopType {
    Origin,
    Fuel,
    Pickup,
    Delivery,
    Relay,
    EmptyMove,
    Maintenance,
    Terminal,
    /// A non-service stop that affects mileage: a hold awaiting instructions, a
    /// company-mandated routing point, anything the router must pass through
    /// where no freight is serviced.
    Waypoint,
}
```

Add `Self::Waypoint => "waypoint",` to `as_str`, and `"waypoint" => Ok(Self::Waypoint),` to `from_str`.

Then add the predicate to the same `impl TripStopType` block:

```rust
    /// Whether this stop is freight work the driver is paid an extra-stop fee
    /// for. Non-service stops still route and still accrue detention — they
    /// simply are not an "extra stop".
    pub fn is_service_stop(&self) -> bool {
        matches!(self, Self::Pickup | Self::Delivery | Self::Relay | Self::EmptyMove)
    }
```

Add `Tonu` to `TripStatus`:

```rust
pub enum TripStatus {
    Planned,
    Assigned,
    Dispatched,
    InTransit,
    Delivered,
    Completed,
    Cancelled,
    /// Truck Ordered Not Used: dispatched and rolled, released before loading.
    /// Terminal — real deadhead, no freight.
    Tonu,
}
```

Add `Self::Tonu => "tonu",` to `as_str` and `"tonu" => Ok(Self::Tonu),` to `from_str`.

Add exactly one edge to `can_transition_to`, inside the existing `matches!`:

```rust
            | (Self::Dispatched, Self::Tonu)
```

`is_delivery_complete` is unchanged — `Tonu` is not `Delivered | Completed`, so it already returns false, and `load_trips_all_delivered` therefore already refuses to cascade a load whose leg was TONU'd.

- [ ] **Step 4: Add the load enum variant**

In `src/models/load.rs`, add `Tonu` to `LoadStatus`:

```rust
pub enum LoadStatus {
    Planned, Assigned, Dispatched, InTransit, Delivered, Invoiced, Settled, Cancelled, Tonu,
}
```

Add `Self::Tonu => "tonu",` to `as_str` and `"tonu" => Ok(Self::Tonu),` to `from_str`.

In `LoadStatus::can_transition_to`, add these arms just above the final `_ => false`:

```rust
            // TONU: the truck was ordered and rolled but never loaded. The entry
            // edge is looser than the trip-side gate (which requires Dispatched)
            // because load status is a best-effort denormalization whose cascades
            // log-and-swallow; a load lagging at Assigned must still be able to
            // follow its trip.
            (Self::Assigned | Self::Dispatched, Self::Tonu) => true,
            (Self::Tonu, Self::Invoiced) => true,
            (Self::Tonu, Self::Cancelled) => true,
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test --lib models:: 2>&1 | tail -20
```

Expected: PASS. If other match arms in the crate now warn about non-exhaustive patterns, fix them by adding the new variant explicitly rather than a catch-all.

```bash
cargo clippy --all-targets 2>&1 | grep -E "^(error|warning: unused)" | head
```

Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add src/models/trip.rs src/models/load.rs
git commit -s -m "feat(models): TripStatus::Tonu, TripStopType::Waypoint, LoadStatus::Tonu"
```

---

### Task 2: Service-stop classification reaches driver pay

Two defects fixed together because they share one function. Both are **pre-existing and live**, not introduced by this feature.

1. `compute_driver_pay` charges `extra_stop_fee` per stop beyond two with no notion of stop kind, so a trip that stops for fuel pays the driver an extra-stop fee for fueling.
2. `driver_pay_for_record` early-returns `None` when `loaded_miles` is `None`. A TONU trip has zero loaded miles by construction, so without this fix **a TONU'd driver is paid nothing at all** — deadhead and detention included. This is the single change the whole feature's pay story rests on.

**Files:**
- Modify: `src/models/pay.rs` (`PayStopInput`, `compute_driver_pay`, `mod pay_tests`)
- Modify: `src/api/fleet_portal/data.rs:810-863` (`driver_pay_for_record`)

**Interfaces:**
- Consumes: `TripStopType::is_service_stop()` from Task 1.
- Produces: `PayStopInput { detention_free_minutes, actual_arrive_utc, actual_depart_utc, is_service_stop }` — a fourth field every construction site must now set.

- [ ] **Step 1: Write the failing tests**

In `src/models/pay.rs`, inside `mod pay_tests`, replace the `stop` helper and add tests:

```rust
    fn stop(free: Option<u32>, arrive: Option<&str>, depart: Option<&str>) -> PayStopInput {
        PayStopInput {
            detention_free_minutes: free,
            actual_arrive_utc: arrive.map(|s| s.to_string()),
            actual_depart_utc: depart.map(|s| s.to_string()),
            is_service_stop: true,
        }
    }

    fn nonservice(arrive: Option<&str>, depart: Option<&str>) -> PayStopInput {
        PayStopInput { is_service_stop: false, ..stop(None, arrive, depart) }
    }

    #[test]
    fn non_service_stops_do_not_earn_extra_stop_pay() {
        // Pickup, fuel, delivery: two service stops, so no extra-stop fee. Before
        // the fix this billed 30.00 for stopping at a truck stop.
        let pay = compute_driver_pay(Some(100.0), None,
            &[stop(None,None,None), nonservice(None,None), stop(None,None,None)],
            &sched());
        assert_eq!(pay.extra_stop_pay, 0.0);
    }

    #[test]
    fn non_service_stops_still_accrue_detention() {
        // A driver held three hours at a waypoint is owed detention even though
        // the waypoint is not an extra stop.
        let pay = compute_driver_pay(Some(0.0), Some(10.0),
            &[nonservice(Some("2026-05-30T12:00:00+00:00"), Some("2026-05-30T15:00:00+00:00"))],
            &sched());
        assert_eq!(pay.detention_pay, 20.0);
        assert_eq!(pay.extra_stop_pay, 0.0);
    }

    #[test]
    fn service_stops_beyond_two_still_earn_extra_stop_pay() {
        let pay = compute_driver_pay(Some(0.0), None,
            &[stop(None,None,None), stop(None,None,None),
              stop(None,None,None), nonservice(None,None)],
            &sched());
        assert_eq!(pay.extra_stop_pay, 30.0); // 3 service stops, not 4 total
    }
```

Also add `is_service_stop: true` to the two `PayStopInput` literals inside `mod tests` if any exist (there are none today, but `cargo test` will say so).

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib pay 2>&1 | tail -20
```

Expected: compile error — `struct PayStopInput has no field named is_service_stop`.

- [ ] **Step 3: Add the field and filter the count**

In `src/models/pay.rs`:

```rust
/// Minimal per-stop input for pay computation (decouples pay from TripStop).
#[derive(Debug, Clone)]
pub struct PayStopInput {
    pub detention_free_minutes: Option<u32>,
    /// RFC3339 UTC timestamps (use TripStop::actual_arrive_utc/actual_depart_utc).
    pub actual_arrive_utc: Option<String>,
    pub actual_depart_utc: Option<String>,
    /// Whether this stop is freight work. A fuel stop, origin, terminal,
    /// maintenance visit or waypoint affects mileage but is not an "extra stop"
    /// the driver is paid a fee for. Detention ignores this flag.
    pub is_service_stop: bool,
}
```

In `compute_driver_pay`, replace the extra-stop calculation:

```rust
    let service_stops = stops.iter().filter(|s| s.is_service_stop).count();
    let extra_stops = (service_stops as i64 - 2).max(0) as f64;
    let extra_stop_pay = extra_stops * rates.extra_stop_fee;
```

Update the doc comment on `compute_driver_pay`: change `extra_stop_pay = extra_stop_fee * max(0, stop_count - 2)` to `extra_stop_pay = extra_stop_fee * max(0, service_stop_count - 2)`.

The detention loop below is unchanged — it must keep iterating **every** stop.

- [ ] **Step 4: Fix the pay read path**

In `src/api/fleet_portal/data.rs`, in `driver_pay_for_record`, replace this line:

```rust
    record.loaded_miles?; // no loaded miles -> no pay
```

with:

```rust
    // A TONU trip has zero loaded miles by construction but real deadhead and
    // real detention. Gating on loaded_miles alone paid such a driver nothing.
    if record.loaded_miles.is_none() && record.deadhead_miles.is_none() {
        return None;
    }
```

And in the same function, set the new field when building the stop list:

```rust
    let stops: Vec<PayStopInput> = record.stops.iter().map(|s| {
        let mut s2 = s.clone();
        s2.fill_utc_fields();
        PayStopInput {
            detention_free_minutes: s2.detention_free_minutes,
            actual_arrive_utc: s2.actual_arrive_utc,
            actual_depart_utc: s2.actual_depart_utc,
            is_service_stop: s2.stop_type.is_service_stop(),
        }
    }).collect();
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test --lib pay 2>&1 | tail -20
cargo build 2>&1 | grep -E "^error" | head
```

Expected: pay tests PASS, no build errors. If `cargo build` reports other `PayStopInput` literals missing the field, add `is_service_stop: true` unless the stop is a `Waypoint`/`Fuel`/`Origin`/`Terminal`/`Maintenance`, in which case derive it with `stop_type.is_service_stop()`.

```bash
cargo test 2>&1 | tail -20
```

Expected: whole suite PASS. `tests/terminals_pay_settlement_test.rs` exercises pay end-to-end; if a trip there has a non-service stop, its expected `extra_stop_pay` will legitimately drop — update the expectation and note in the commit that the old number was the defect.

- [ ] **Step 6: Commit**

```bash
git add src/models/pay.rs src/api/fleet_portal/data.rs
git commit -s -m "fix(pay): only service stops earn extra-stop pay, and pay deadhead-only trips

compute_driver_pay counted every stop toward extra_stop_fee, so a trip
that stopped for fuel paid the driver an extra-stop fee for fueling.
Separately, driver_pay_for_record returned None whenever loaded_miles
was None, which would pay a TONU'd driver nothing at all despite real
deadhead and detention.

Detention still iterates every stop; only the extra-stop count filters."
```

---

### Task 3: Persist the four new load fields

**Files:**
- Modify: `src/models/load.rs` (`LoadRecord`, `LoadListItem`, `LoadDetailResponse`, `From<LoadRecord>`)
- Modify: `src/db/mod.rs` (`load_schema`, `open_or_create_load`, `empty_load_batch`)
- Modify: `src/db/load_ops.rs` (`load_to_batch`, `row_to_load`, `sample_load` in tests)
- Modify: `src/api/fleet_portal/data.rs:318`, `:1801`, `:1968`; `src/api/fleet_portal/mcp.rs:2171`; `src/pipeline/routing.rs:69`; `tests/load_delivery_cascade_test.rs:122`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: on `LoadRecord` — `quoted_rate_items: Vec<RateLineItem>`, `diverted_at: Option<String>`, `diversion_reason: Option<String>`, `diversion_notes: Option<String>`.

- [ ] **Step 1: Write the failing test**

In `src/db/load_ops.rs`, inside `mod tests`:

```rust
    #[tokio::test]
    async fn test_load_diversion_and_quoted_rates_round_trip() {
        let (db, _dir) = test_db().await;
        let mut load = sample_load();
        load.quoted_rate_items = vec![crate::models::RateLineItem {
            description: "Line Haul".into(), amount_usd: 1800.0,
        }];
        load.rate_items = vec![];
        load.diverted_at = Some("2026-08-07T14:30:00Z".into());
        load.diversion_reason = Some("reconsigned".into());
        load.diversion_notes = Some("broker renominated to Salina".into());
        db.insert_load(&load).await.unwrap();

        let got = db.get_load_by_id(load.id).await.unwrap();
        assert_eq!(got.quoted_rate_items.len(), 1);
        assert_eq!(got.quoted_rate_items[0].amount_usd, 1800.0);
        assert!(got.rate_items.is_empty());
        assert_eq!(got.diverted_at.as_deref(), Some("2026-08-07T14:30:00Z"));
        assert_eq!(got.diversion_reason.as_deref(), Some("reconsigned"));
        assert_eq!(got.diversion_notes.as_deref(), Some("broker renominated to Salina"));
    }

    #[tokio::test]
    async fn test_load_defaults_when_new_columns_absent() {
        // Mirrors a row written before this migration: serde defaults must make
        // the record usable rather than erroring the whole list query.
        let json = serde_json::json!({
            "id": uuid::Uuid::new_v4(), "load_number": "LD-2026-0009", "owner_id": 0,
            "status": "planned", "customer_name": "ACME", "customer_ref": null,
            "stops": [], "rate_items": [], "commodity": null, "weight_lbs": null,
            "miles": null, "notes": null, "tags": [], "blob_ids": [],
            "invoice_number": null, "invoice_date": null, "cancellation_reason": null,
            "created_at": "2026-08-07T00:00:00Z", "updated_at": "2026-08-07T00:00:00Z"
        });
        let record: LoadRecord = serde_json::from_value(json).unwrap();
        assert!(record.quoted_rate_items.is_empty());
        assert!(record.diverted_at.is_none());
        assert!(record.diversion_reason.is_none());
        assert!(record.diversion_notes.is_none());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --lib load_ops 2>&1 | tail -20
```

Expected: `struct LoadRecord has no field named quoted_rate_items`.

- [ ] **Step 3: Add the model fields**

In `src/models/load.rs`, in `LoadRecord`, immediately after `pub cancellation_reason: Option<String>,`:

```rust
    /// The rate the load was booked at, archived when a TONU clears `rate_items`
    /// so revenue reporting does not count money that will never be earned.
    #[serde(default)]
    pub quoted_rate_items: Vec<RateLineItem>,
    /// Set when the load was diverted or reconsigned mid-transit. Absent for a
    /// `bol_correction`, which corrects a plan that was wrong from the start
    /// rather than diverting anything.
    #[serde(default)]
    pub diverted_at: Option<String>,
    /// `diverted` or `reconsigned` — the enum value, so "which loads were
    /// diverted, and why" is a filter rather than a text search.
    #[serde(default)]
    pub diversion_reason: Option<String>,
    #[serde(default)]
    pub diversion_notes: Option<String>,
```

Add the same four fields (same position, same `#[serde(default)]`) to `LoadListItem` and `LoadDetailResponse`, and wire them through `impl From<LoadRecord> for LoadListItem`:

```rust
            cancellation_reason: r.cancellation_reason,
            quoted_rate_items: r.quoted_rate_items,
            diverted_at: r.diverted_at,
            diversion_reason: r.diversion_reason,
            diversion_notes: r.diversion_notes,
            created_at: r.created_at, score: None,
```

- [ ] **Step 4: Add the Arrow columns**

In `src/db/mod.rs`, at the **end** of the `load_schema` field list (after `kind`):

```rust
        Field::new("quoted_rate_items", DataType::Utf8, false),
        Field::new("diverted_at", DataType::Utf8, true),
        Field::new("diversion_reason", DataType::Utf8, true),
        Field::new("diversion_notes", DataType::Utf8, true),
```

In `open_or_create_load`, after the existing `kind` transform:

```rust
            // SQL keyword type `string`, never the Arrow name `Utf8` — see AGENTS.md.
            if existing.field_with_name("quoted_rate_items").is_err() {
                transforms.push(("quoted_rate_items".into(), "CAST('[]' AS string)".into()));
            }
            for col in ["diverted_at", "diversion_reason", "diversion_notes"] {
                if existing.field_with_name(col).is_err() {
                    transforms.push((col.into(), "CAST(NULL AS string)".into()));
                }
            }
```

In `empty_load_batch`, append four more entries to the vec, matching the existing style:

```rust
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
```

- [ ] **Step 5: Wire the batch conversions**

In `src/db/load_ops.rs`, in `load_to_batch`, add near the other JSON serialisations:

```rust
    let quoted_rate_items_json = serde_json::to_string(&record.quoted_rate_items)
        .map_err(|e| AppError::Internal(e.to_string()))?;
```

and append to the end of the `RecordBatch::try_new` column vec, after `record.kind.as_str()`:

```rust
        Arc::new(StringArray::from(vec![quoted_rate_items_json.as_str()])),
        Arc::new(StringArray::from(vec![record.diverted_at.as_deref()])),
        Arc::new(StringArray::from(vec![record.diversion_reason.as_deref()])),
        Arc::new(StringArray::from(vec![record.diversion_notes.as_deref()])),
```

In `row_to_load`, before the `Ok(LoadRecord {`:

```rust
    let quoted_rate_items: Vec<crate::models::RateLineItem> =
        serde_json::from_str(&str_col("quoted_rate_items")).unwrap_or_default();
```

and inside the struct literal, after `cancellation_reason`:

```rust
        quoted_rate_items,
        diverted_at: opt_str("diverted_at"),
        diversion_reason: opt_str("diversion_reason"),
        diversion_notes: opt_str("diversion_notes"),
```

- [ ] **Step 6: Fix every remaining struct literal**

```bash
cargo build 2>&1 | grep -E "missing field|^error" | head -30
```

Add these four lines to each `LoadRecord { .. }` literal the compiler names:

```rust
            quoted_rate_items: vec![], diverted_at: None,
            diversion_reason: None, diversion_notes: None,
```

Known sites: `src/db/load_ops.rs` (`sample_load`), `src/models/load.rs` (`load_of_kind` and two test literals), `src/api/fleet_portal/data.rs:318` and `:1801`, `src/api/fleet_portal/mcp.rs:2171`, `src/pipeline/routing.rs:69`, `tests/load_delivery_cascade_test.rs:122`. For the `LoadDetailResponse` literal at `src/api/fleet_portal/data.rs:1968`, copy from the record instead:

```rust
        quoted_rate_items: record.quoted_rate_items.clone(),
        diverted_at: record.diverted_at.clone(),
        diversion_reason: record.diversion_reason.clone(),
        diversion_notes: record.diversion_notes.clone(),
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cargo test --lib load 2>&1 | tail -20
cargo test 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/models/load.rs src/db/mod.rs src/db/load_ops.rs src/api src/pipeline tests
git commit -s -m "feat(loads): persist quoted_rate_items and diversion fields"
```

---

### Task 4: Keep TONU'd loads out of the routing requeue

`list_loads_needing_routing` selects on `miles IS NULL AND status NOT IN (terminal)`. A TONU'd load has no miles and, without this change, is not terminal — so it re-enters the requeue on **every startup, forever**. This is the silent permanent zombie AGENTS.md documents from #411, and it is invisible until someone reads the startup logs.

The load doctor should need no change: `check_status_matches_trips` early-returns unless the status is `Dispatched | InTransit`, so a `tonu` load is already skipped. The trip doctor's `check_status_actuals` falls through its `_ => {}` arm for `Tonu`. Both are *claims* until tested — write the tests, and if either fires a finding, fix the doctor rather than the test.

**Files:**
- Modify: `src/db/load_ops.rs` (`list_loads_needing_routing`, `list_unrouted_loads_for_facility`)
- Test: `src/db/load_ops.rs` `mod tests`, `tests/tonu_test.rs`

**Interfaces:**
- Consumes: `LoadStatus::Tonu` from Task 1.
- Produces: nothing new.

- [ ] **Step 1: Write the failing test**

In `src/db/load_ops.rs`, inside `mod tests`:

```rust
    #[tokio::test]
    async fn test_tonu_load_is_excluded_from_routing_requeue() {
        let (db, _dir) = test_db().await;
        let mut load = sample_load();
        load.miles = None;
        load.status = LoadStatus::Tonu;
        db.insert_load(&load).await.unwrap();

        let needing = db.list_loads_needing_routing().await.unwrap();
        assert!(
            !needing.contains(&load.id),
            "a TONU'd load has no miles and never will; leaving it in the requeue \
             makes it a permanent startup zombie",
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --lib test_tonu_load_is_excluded_from_routing_requeue 2>&1 | tail -20
```

Expected: FAIL — assertion fires because the load is still selected.

- [ ] **Step 3: Add `tonu` to both terminal filters**

In `src/db/load_ops.rs`, `list_loads_needing_routing`:

```rust
        let stream = self.load_table.query()
            .only_if("miles IS NULL AND kind != 'administrative' AND status NOT IN ('delivered','invoiced','settled','cancelled','tonu')")
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
```

and `list_unrouted_loads_for_facility`:

```rust
        let filter = format!(
            "miles IS NULL AND kind != 'administrative' AND status NOT IN ('delivered','invoiced','settled','cancelled','tonu') AND stops LIKE '%\"{}\"%'",
            fac_str
        );
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test --lib test_tonu_load_is_excluded_from_routing_requeue 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 5: Add the doctor tests**

These belong in `tests/tonu_test.rs`, so add them once Task 5 has created that file — or create the file now with only this test and the helpers, and let Task 5 add the rest. Note the dependency in whichever order you take.

```rust
#[tokio::test]
async fn test_doctors_are_clean_on_a_tonu_load_and_trip() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver) = dispatched_trip(&server, &token, "4581488").await;

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;

    // The doctors are MCP-only tools with no REST route, so call the services
    // directly rather than going through the server.
    let load_uuid: uuid::Uuid = load_id.parse().unwrap();
    let report = ollie::services::doctors::load::run(&state, load_uuid, false).await.unwrap();
    assert!(
        !report.findings.iter().any(|f| f.check == "load.status_matches_trips"),
        "a tonu load is not stranded — it invoices from tonu: {:?}", report.findings,
    );

    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let report = ollie::services::doctors::trip::run(&state, trip_uuid, false).await.unwrap();
    assert!(
        !report.findings.iter().any(|f| f.check == "trip.status.actuals_consistent"),
        "a TONU'd trip legitimately has stops with no actuals: {:?}", report.findings,
    );
}
```

This test binds `state`, so use `let (server, state, _d1, _d2, _rx) = setup().await;`. Confirm `doctors::trip::run`'s signature matches `doctors::load::run` before running; if it differs, adapt the call rather than the assertion.

- [ ] **Step 6: Run the doctor test**

```bash
cargo test --test tonu_test test_doctors_are_clean 2>&1 | tail -20
```

Expected: PASS with no doctor changes. If a finding fires, change the doctor to recognise `Tonu` as terminal — do not weaken the assertion.

- [ ] **Step 7: Commit**

```bash
git add src/db/load_ops.rs tests/tonu_test.rs
git commit -s -m "fix(db): exclude tonu loads from the routing requeue

A TONU'd load has no miles and never will. Without tonu in the terminal
set it re-enters list_loads_needing_routing on every startup, forever."
```

---

### Task 5: The `tonu` verb, its surfaces, and its tests

**Files:**
- Modify: `src/services/trip_lifecycle.rs` (add `TonuRequest`, `PositionInput`, `resolve_position`, `tonu`)
- Modify: `src/events/mod.rs` (`on_trip_tonu`)
- Modify: `src/api/fleet_portal/data.rs` (`tonu_trip` handler)
- Modify: `src/api/fleet_portal/mod.rs` (route)
- Modify: `src/api/fleet_portal/mcp.rs` (scope map, tool schema, id alias, destructive annotation, dispatch arm, `tool_tonu_trip`)
- Create: `tests/tonu_test.rs`

**Interfaces:**
- Consumes: `TripStatus::Tonu`, `TripStopType::Waypoint`, `LoadStatus::Tonu` (Task 1); `LoadRecord.quoted_rate_items` (Task 3).
- Produces:
  - `pub struct PositionInput { facility_id: Option<Uuid>, facility_name: Option<String>, address: Option<String>, timezone: String, actual_arrive: Option<String>, actual_depart: Option<String>, notes: Option<String> }`
  - `pub struct TonuRequest { waypoint: Option<PositionInput>, occurred_at: Option<String>, reason: Option<String> }`
  - `pub struct TonuResult { trip: TripRecord, mileage_recompute_warning: Option<String> }`
  - `pub async fn tonu(state: &AppState, trip_id: Uuid, req: TonuRequest) -> Result<TonuResult, AppError>`
  - `pub(crate) async fn resolve_position(state: &AppState, pos: PositionInput, sequence: u32) -> Result<TripStop, AppError>`

- [ ] **Step 1: Write the failing integration tests**

Create `tests/tonu_test.rs`. Copy the `setup`, `setup_owner`, `create_test_facility`, `create_driver`, `create_truck` and `stop_json` helpers verbatim from `tests/administrative_loads_test.rs:22-130` — the suite duplicates them per file rather than sharing, so follow that convention.

Then add the module doc and tests:

```rust
// tests/tonu_test.rs
//
// TONU (Truck Ordered Not Used): a load is dispatched, the truck rolls to the
// shipper, and the driver is released before loading. The truck was ordered and
// it moved, so the deadhead and any dock wait are payable and the broker owes a
// fee — none of which survives a plain cancel.
//
// Integration tests run with RoutingClient::new(""), so ORS is always
// unavailable. Mileage must degrade to a warning, never an error; these tests
// assert on stop structure and status, not mile counts.

async fn dispatched_trip(
    server: &TestServer, token: &str, load_number: &str,
) -> (String, String, String) {
    let fac_id = create_test_facility(server, token, &format!("{load_number} Dock"), "Chicago, IL").await;
    let driver_id = create_driver(server, token, &format!("{load_number} Driver")).await;
    let truck_id = create_truck(server, token, &format!("T-{load_number}")).await;

    let resp = server.post("/fleet/api/v1/loads")
        .authorization_bearer(token)
        .json(&serde_json::json!({
            "load_number": load_number,
            "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")],
            "rate_items": [{ "description": "Line Haul", "amount_usd": 1800.0 }],
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let load_id = resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_resp = server.post("/fleet/api/v1/trips")
        .authorization_bearer(token)
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [{
                "sequence": 0, "stop_type": "pickup", "facility_id": fac_id,
                "name": "Dock", "scheduled_arrive": "2026-06-01T08:00:00",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(trip_resp.status_code(), 201, "trip create failed: {}", trip_resp.text());
    let trip_id = trip_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let assign = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .authorization_bearer(token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200, "assign failed: {}", assign.text());

    let dispatch = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .authorization_bearer(token).await;
    assert_eq!(dispatch.status_code(), 200, "dispatch failed: {}", dispatch.text());

    (load_id, trip_id, driver_id)
}

#[tokio::test]
async fn test_tonu_after_arrival_truncates_and_clears_rate_items() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, driver_id) = dispatched_trip(&server, &token, "4581480").await;

    // Driver reaches the dock and waits. Arrival does not move the status: only
    // departure from the pickup starts transit, so this is still `dispatched`.
    let arrive = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" }))
        .await;
    assert_eq!(arrive.status_code(), 200, "arrive failed: {}", arrive.text());

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "reason": "shipper cancelled while waiting to load" }))
        .await;
    assert_eq!(resp.status_code(), 200, "tonu failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["status"], "tonu");
    assert_eq!(trip["stops"].as_array().unwrap().len(), 1, "truncated to the reached stop");
    // The release time is what makes detention payable.
    assert!(trip["stops"][0]["actual_depart"].is_string(),
            "release time must be stamped so the dock wait is billable");
    assert!(trip["loaded_miles"].is_null(), "a TONU trip has zero loaded miles");

    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(load["status"], "tonu");
    assert_eq!(load["rate_items"].as_array().unwrap().len(), 0,
               "line haul will never be earned");
    assert_eq!(load["quoted_rate_items"].as_array().unwrap().len(), 1,
               "but the booked rate must not be lost");
    assert_eq!(load["cancellation_reason"], "shipper cancelled while waiting to load");

    // Equipment is released.
    let driver: serde_json::Value = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(driver["status"], "available");
}

/// The regression test for the entire reason TONU truncates.
///
/// A follow-on trip's deadhead origin is `previous_trip.stops.last().facility_id`
/// (`src/api/trips.rs:227`). If a TONU'd trip kept its unreached delivery stop,
/// every downstream deadhead would be measured from a city the truck never
/// visited — and the resulting number is plausible, so nobody would catch it.
#[tokio::test]
async fn test_follow_on_trip_deadheads_from_where_the_truck_actually_is() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    // A two-stop trip: shipper in Chicago, consignee in Denver.
    let shipper = create_test_facility(&server, &token, "Chicago Shipper", "Chicago, IL").await;
    let consignee = create_test_facility(&server, &token, "Denver Consignee", "Denver, CO").await;
    let driver_id = create_driver(&server, &token, "Chain Driver").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN").await;

    let load = server.post("/fleet/api/v1/loads").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581489", "customer_name": "Landstar",
            "stops": [stop_json(&shipper, "2026-06-01T08:00:00")]
        })).await;
    let load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "facility_id": shipper,
                  "name": "Chicago Shipper", "scheduled_arrive": "2026-06-01T08:00:00",
                  "timezone": "America/Chicago" },
                { "sequence": 1, "stop_type": "delivery", "facility_id": consignee,
                  "name": "Denver Consignee", "scheduled_arrive": "2026-06-02T08:00:00",
                  "timezone": "America/Denver" }
            ]
        })).await;
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id })).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .authorization_bearer(&token).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" })).await;

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "reason": "shipper cancelled at the dock" })).await;

    // The dispatcher chains the next trip off the TONU'd one.
    let next_fac = create_test_facility(&server, &token, "Next Shipper", "Peoria, IL").await;
    let next = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "previous_trip_id": trip_id,
            "stops": [{ "sequence": 0, "stop_type": "pickup", "facility_id": next_fac,
                        "name": "Next Shipper", "scheduled_arrive": "2026-06-02T08:00:00",
                        "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(next.status_code(), 201, "next trip create failed: {}", next.text());
    let next_id: uuid::Uuid = next.json::<serde_json::Value>()["id"]
        .as_str().unwrap().parse().unwrap();

    // Assert the resolved origin directly rather than the mileage, since these
    // tests run without ORS.
    let next_trip = state.db.get_trip(next_id).await.unwrap();
    let summary = ollie::api::mileage_summary::build_mileage_summary(&state, &next_trip).await;
    let origin = summary.origin.expect("a chained trip must resolve a deadhead origin");
    assert_eq!(
        origin.facility_name.as_deref(), Some("Chicago Shipper"),
        "the follow-on must deadhead from the dock the truck is sitting at, \
         not from the Denver consignee it never reached",
    );
}

#[tokio::test]
async fn test_tonu_before_arrival_requires_a_waypoint() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581481").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(resp.status_code(), 422, "no stop was reached, so the truck's \
        position is unknown and the deadhead cannot be measured: {}", resp.text());
}

#[tokio::test]
async fn test_tonu_before_arrival_replaces_stop_zero_with_the_waypoint() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581482").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": {
                "facility_name": "Pilot Effingham", "address": "Effingham, IL",
                "timezone": "America/Chicago",
                "actual_arrive": "2026-06-01T06:30:00"
            },
            "reason": "cancelled en route"
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "tonu failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0]["stop_type"], "waypoint");
    assert_eq!(stops[0]["name"], "Pilot Effingham",
               "the dock the truck never saw must not remain the trip's endpoint");
}

#[tokio::test]
async fn test_tonu_rejects_wrong_statuses_and_names_the_right_verb() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    // Planned: never dispatched, so this is an ordinary cancellation.
    let fac_id = create_test_facility(&server, &token, "Planned Dock", "Chicago, IL").await;
    let load = server.post("/fleet/api/v1/loads").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581483", "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")]
        })).await;
    let load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let trip = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({ "load_id": load_id })).await;
    let planned_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/fleet/api/v1/trips/{planned_id}/tonu"))
        .authorization_bearer(&token).json(&serde_json::json!({})).await;
    assert_eq!(resp.status_code(), 409);
    assert!(resp.text().contains("cancel_trip"),
            "the error must point at the right verb: {}", resp.text());

    // In transit: freight is aboard, so this is a diversion.
    let (_l, rolling_id, _d) = dispatched_trip(&server, &token, "4581484").await;
    server.post(&format!("/fleet/api/v1/trips/{rolling_id}/stops/0/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" })).await;
    server.post(&format!("/fleet/api/v1/trips/{rolling_id}/stops/0/depart"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_depart": "2026-06-01T09:00:00" })).await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{rolling_id}/tonu"))
        .authorization_bearer(&token).json(&serde_json::json!({})).await;
    assert_eq!(resp.status_code(), 409);
    assert!(resp.text().contains("divert_trip"),
            "the error must point at the right verb: {}", resp.text());
}

#[tokio::test]
async fn test_tonu_does_not_auto_dispatch_the_next_trip() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, driver_id) = dispatched_trip(&server, &token, "4581485").await;

    // A follow-on already staged for the same driver.
    let fac_b = create_test_facility(&server, &token, "Next Dock", "Peoria, IL").await;
    let truck_b = create_truck(&server, &token, "T-NEXT").await;
    let trip_b = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "stops": [{
                "sequence": 0, "stop_type": "pickup", "facility_id": fac_b,
                "name": "Next Dock", "scheduled_arrive": "2026-06-02T08:00:00",
                "timezone": "America/Chicago"
            }]
        })).await;
    let trip_b_id = trip_b.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/fleet/api/v1/trips/{trip_b_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_b })).await;

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;

    let b: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_b_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(b["status"], "assigned",
        "after a TONU the truck is not where the plan assumed, and auto-dispatch \
         does not recompute mileage — the dispatcher must decide");
}

#[tokio::test]
async fn test_tonu_load_invoices_and_settles() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581486").await;

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;

    // The agreed fee arrives days later.
    let update = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "rate_items": [{ "description": "TONU", "amount_usd": 250.0 }]
        })).await;
    assert_eq!(update.status_code(), 200, "rate update failed: {}", update.text());

    let inv = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "invoice_number": "JQL-4581486" })).await;
    assert_eq!(inv.status_code(), 200, "invoice failed: {}", inv.text());

    let settle = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .authorization_bearer(&token).json(&serde_json::json!({})).await;
    assert_eq!(settle.status_code(), 200, "settle failed: {}", settle.text());

    let uuid: uuid::Uuid = load_id.parse().unwrap();
    assert_eq!(state.db.get_load_by_id(uuid).await.unwrap().status,
               ollie::models::LoadStatus::Settled);
}

#[tokio::test]
async fn test_tonu_leg_does_not_strand_a_load_with_a_live_sibling() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581487").await;

    // A second leg still holds the load.
    let fac_b = create_test_facility(&server, &token, "Relay Dock", "Gary, IN").await;
    let driver_b = create_driver(&server, &token, "Relay Driver").await;
    let truck_b = create_truck(&server, &token, "T-RELAY").await;
    let trip_b = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [{
                "sequence": 0, "stop_type": "delivery", "facility_id": fac_b,
                "name": "Relay Dock", "scheduled_arrive": "2026-06-02T08:00:00",
                "timezone": "America/Chicago"
            }]
        })).await;
    let trip_b_id = trip_b.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/fleet/api/v1/trips/{trip_b_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_b, "truck_id": truck_b })).await;

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;

    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert_ne!(load["status"], "tonu",
        "a live sibling still holds the load; only the last leg out takes it terminal");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --test tonu_test 2>&1 | tail -20
```

Expected: every test FAILs with 404 or 405 — the route does not exist.

- [ ] **Step 3: Add the event emitter**

In `src/events/mod.rs`, after `on_trip_cancelled`:

```rust
pub async fn on_trip_tonu(db: &DbClient, trip_id: Uuid, reason: Option<String>) {
    let payload = serde_json::json!({ "reason": reason });
    let _ = db.append_event("trip", trip_id, "trip.tonu", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip tonu");
}
```

- [ ] **Step 4: Implement position resolution and the verb**

In `src/services/trip_lifecycle.rs`, add after the existing request structs:

```rust
/// A position the dispatcher supplies for a stop that was not in the plan:
/// either an existing facility, or a name + address resolved (or created)
/// through the same path `StopInput` uses.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PositionInput {
    pub facility_id: Option<Uuid>,
    pub facility_name: Option<String>,
    pub address: Option<String>,
    /// IANA timezone, required so the naive local arrive/depart strings parse.
    pub timezone: String,
    pub actual_arrive: Option<String>,
    pub actual_depart: Option<String>,
    pub notes: Option<String>,
    /// Overrides the caller's default. A diversion destination is normally a
    /// `delivery`, but a cross-dock hand-off is a `relay`.
    #[serde(default)]
    pub stop_type: Option<crate::models::TripStopType>,
}

#[derive(Debug, Deserialize, ToSchema, Default)]
pub struct TonuRequest {
    /// Required when no stop was reached; optional (and appended) otherwise.
    #[serde(default)]
    pub waypoint: Option<PositionInput>,
    /// When the driver was released. Naive local in the truncation stop's own
    /// timezone; defaults to now in that zone.
    #[serde(default)]
    pub occurred_at: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// The trip plus an optional warning when mileage could not be recomputed.
/// A TONU is an operational fact that must be recordable with ORS down, so a
/// routing failure degrades to this field rather than failing the call.
#[derive(Debug, serde::Serialize, ToSchema)]
pub struct TonuResult {
    #[serde(flatten)]
    pub trip: TripRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mileage_recompute_warning: Option<String>,
}

/// Resolve a `PositionInput` into a trip stop at `sequence`, typed
/// `default_stop_type` unless the input overrides it. Runs before any mutation
/// so a geocode failure cannot leave a half-applied outcome behind.
pub(crate) async fn resolve_position(
    state: &AppState,
    pos: PositionInput,
    sequence: u32,
    default_stop_type: crate::models::TripStopType,
) -> Result<crate::models::TripStop, AppError> {
    use crate::models::TripStop;

    let _: chrono_tz::Tz = pos.timezone.parse().map_err(|_| {
        AppError::UnprocessableEntity(format!("'{}' is not a valid IANA timezone", pos.timezone))
    })?;
    for (field, value) in [("actual_arrive", &pos.actual_arrive), ("actual_depart", &pos.actual_depart)] {
        if let Some(v) = value {
            crate::models::load::validate_stop_time_str(v, &pos.timezone, field)?;
        }
    }

    let (facility_id, name, address) = match pos.facility_id {
        Some(id) => {
            let f = state.db.get_facility_by_id(id).await?;
            (id, f.name, f.address)
        }
        None => {
            let name = pos.facility_name.ok_or_else(|| AppError::UnprocessableEntity(
                "waypoint must provide either facility_id or facility_name + address".into()
            ))?;
            let address = pos.address.ok_or_else(|| AppError::UnprocessableEntity(
                "waypoint must provide address when facility_id is not given".into()
            ))?;
            let id = crate::api::facilities::resolve_or_create_facility(
                state, &name, &address, false,
            ).await?;
            (id, name, address)
        }
    };

    Ok(TripStop {
        sequence,
        stop_type: pos.stop_type.unwrap_or(default_stop_type),
        facility_id: Some(facility_id),
        name: Some(name),
        address: Some(address),
        load_stop_index: None,
        scheduled_arrive: None,
        scheduled_arrive_end: None,
        actual_arrive: pos.actual_arrive,
        actual_depart: pos.actual_depart,
        expected_dwell_minutes: None,
        detention_free_minutes: None,
        detention_grace_minutes: None,
        notes: pos.notes,
        timezone: Some(pos.timezone),
        actual_arrive_utc: None,
        actual_depart_utc: None,
    })
}

/// Recompute mileage and reassign the whole figure to deadhead.
///
/// `compute_trip_mileage` calls a leg deadhead only when it originates from a
/// `previous_trip_id`; every other leg is loaded. On a TONU that empty run *is*
/// the entire trip, so delegating the split would pay 100% of the miles at the
/// loaded rate. Stale planned mileage is cleared first, so a routing failure
/// leaves an honest "unknown" rather than the never-driven loaded figure.
async fn recompute_as_all_deadhead(state: &AppState, trip_id: Uuid) -> Option<String> {
    let _ = state.db.update_trip_mileage(trip_id, None, None, None, vec![]).await;
    if let Err(e) = crate::api::trips::compute_and_persist_mileage(state, trip_id).await {
        return Some(format!("mileage not recomputed: {e}"));
    }
    let Ok(t) = state.db.get_trip(trip_id).await else { return None };
    let total = t.total_miles
        .or_else(|| match (t.deadhead_miles, t.loaded_miles) {
            (None, None) => None,
            (d, l) => Some(d.unwrap_or(0.0) + l.unwrap_or(0.0)),
        });
    let _ = state.db.update_trip_mileage(trip_id, total, None, total, t.segment_miles).await;
    None
}

/// TONU — Truck Ordered Not Used. Valid only from `Dispatched`: the truck rolled
/// but never departed a pickup, so no freight was ever aboard.
pub async fn tonu(
    state: &AppState,
    trip_id: Uuid,
    req: TonuRequest,
) -> Result<TonuResult, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    match existing.status {
        TripStatus::Dispatched => {}
        TripStatus::Planned | TripStatus::Assigned => {
            return Err(AppError::Conflict(
                "trip has not been dispatched; no truck was used — use cancel_trip".into()));
        }
        TripStatus::InTransit => {
            return Err(AppError::Conflict(
                "trip has departed its pickup and is carrying freight — use divert_trip".into()));
        }
        s => {
            return Err(AppError::Conflict(format!(
                "cannot TONU a trip with status '{}'", s.as_str())));
        }
    }
    if existing.settlement_ref.is_some() {
        return Err(AppError::Conflict("trip is settled; miles and pay are frozen".into()));
    }

    // Everything that can be rejected is decided before the first write.
    let last_reached = existing.stops.iter()
        .filter(|s| s.actual_arrive.is_some())
        .max_by_key(|s| s.sequence)
        .map(|s| s.sequence);
    if last_reached.is_none() && req.waypoint.is_none() {
        return Err(AppError::UnprocessableEntity(
            "no stop was reached, so the truck's position is unknown: supply `waypoint` \
             with where the driver stopped, or the deadhead cannot be measured".into()));
    }

    let mut stops: Vec<crate::models::TripStop> = match last_reached {
        Some(seq) => existing.stops.iter().filter(|s| s.sequence <= seq).cloned().collect(),
        None => vec![],
    };

    // Stamp the release time on the truncation stop so the dock wait is billable.
    if let Some(last) = stops.last_mut() {
        if last.actual_arrive.is_some() && last.actual_depart.is_none() {
            let tz = last.timezone.as_deref().unwrap_or("UTC");
            let released = match &req.occurred_at {
                Some(v) => {
                    crate::models::load::validate_stop_time_str(v, tz, "occurred_at")?;
                    v.clone()
                }
                None => now_local_naive(tz),
            };
            last.actual_depart = Some(released);
        }
    }

    if let Some(pos) = req.waypoint {
        let seq = stops.len() as u32;
        stops.push(resolve_position(state, pos, seq, crate::models::TripStopType::Waypoint).await?);
    }
    for (i, s) in stops.iter_mut().enumerate() { s.sequence = i as u32; }

    // --- writes ---
    state.db.update_trip_metadata(trip_id, None, None, Some(stops), None, None, None).await?;
    state.db.transition_trip_status(trip_id, TripStatus::Tonu).await?;
    let warning = recompute_as_all_deadhead(state, trip_id).await;

    if let Some(load_id) = existing.load_id {
        let holding = state.db.count_load_holding_trips(load_id).await.unwrap_or(1);
        if holding == 0 {
            if let Ok(load) = state.db.get_load_by_id(load_id).await {
                if matches!(load.status, LoadStatus::Assigned | LoadStatus::Dispatched) {
                    if let Err(e) = state.db.transition_load_status(
                        load_id, LoadStatus::Tonu, None, None, req.reason.clone(),
                    ).await {
                        tracing::warn!(%load_id, error = %e, "load not moved to tonu");
                    } else {
                        archive_quoted_rates(state, load_id).await;
                    }
                }
            }
        }
    }

    release_resources(state, &existing).await;
    events::on_trip_tonu(&state.db, trip_id, req.reason).await;

    let trip = state.db.get_trip(trip_id).await?;
    Ok(TonuResult { trip, mileage_recompute_warning: warning })
}

/// Move `rate_items` into `quoted_rate_items` and clear them. The line haul will
/// never be earned, and a `tonu` load still reporting it is exactly the
/// false-positive class administrative loads were built to kill.
async fn archive_quoted_rates(state: &AppState, load_id: Uuid) {
    let Ok(load) = state.db.get_load_by_id(load_id).await else { return };
    if load.rate_items.is_empty() { return; }
    if let Err(e) = state.db.archive_load_rate_items(load_id).await {
        tracing::warn!(%load_id, error = %e, "quoted rate items not archived");
    }
}

/// Release driver, truck and trailers, skipping any already rebound to another
/// active trip. Shared with `complete`.
async fn release_resources(state: &AppState, existing: &TripRecord) {
    let active = list_active_trips(state).await.unwrap_or_default();
    if let Some(driver_id) = existing.driver_id {
        if !resource_on_other_active_trip(&active, existing.id, Some(driver_id), None, None) {
            let _ = state.db.update_driver_status(driver_id, DriverStatus::Available).await;
        }
    }
    if let Some(truck_id) = existing.truck_id {
        if !resource_on_other_active_trip(&active, existing.id, None, Some(truck_id), None) {
            let _ = state.db.update_truck_status(truck_id, TruckStatus::Available).await;
        }
    }
    for &trailer_id in &existing.trailer_ids {
        if !resource_on_other_active_trip(&active, existing.id, None, None, Some(trailer_id)) {
            let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Available).await;
        }
    }
}

/// Now, as a naive local datetime string in `tz` — the format every stop time
/// in this system uses.
fn now_local_naive(tz: &str) -> String {
    let zone: chrono_tz::Tz = tz.parse().unwrap_or(chrono_tz::UTC);
    chrono::Utc::now().with_timezone(&zone).format("%Y-%m-%dT%H:%M:%S").to_string()
}
```

Refactor `complete` to call `release_resources(state, &existing).await;` in place of its inline release block, so the guard lives in one place.

Add `archive_load_rate_items` to `src/db/load_ops.rs`:

```rust
    /// Move `rate_items` into `quoted_rate_items` and clear them. Idempotent:
    /// a load whose rate items are already empty is left untouched.
    pub async fn archive_load_rate_items(&self, id: Uuid) -> Result<LoadRecord, AppError> {
        let mut record = self.get_load_by_id(id).await?;
        if record.rate_items.is_empty() { return Ok(record); }
        record.quoted_rate_items = std::mem::take(&mut record.rate_items);
        record.updated_at = Utc::now();
        self.upsert_load(&record).await?;
        Ok(record)
    }
```

Check that `resolve_or_create_facility` is public from `src/api/facilities.rs`; if it is private, make it `pub(crate)`.

- [ ] **Step 5: Add the REST handler and route**

In `src/api/fleet_portal/data.rs`, next to `cancel_trip`:

```rust
#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/tonu",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = TonuRequest, description = "Optional waypoint, release time and reason"),
    responses(
        (status = 200, description = "Trip ended as TONU", body = TonuResult),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip is not dispatched, or is settled"),
        (status = 422, description = "Waypoint required or unresolvable"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn tonu_trip(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    id: Path<Uuid>,
    body: Option<Json<crate::services::trip_lifecycle::TonuRequest>>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let result = crate::services::trip_lifecycle::tonu(&state, id.0, req).await?;
    Ok(Json(result))
}
```

In `src/api/fleet_portal/mod.rs`, after the `cancel` route:

```rust
        .route("/fleet/api/v1/trips/{id}/tonu", post(data::tonu_trip))
```

Register `tonu_trip` in the utoipa `paths(...)` list in `src/api/mod.rs` alongside the other trip endpoints.

- [ ] **Step 6: Add the MCP tool**

In `src/api/fleet_portal/mcp.rs`, four edits:

1. Scope map (~line 284) — add `tonu_trip` to the `"trips:write"` arm:

```rust
        | "dispatch_trip" | "undispatch_trip" | "cancel_trip" | "complete_trip" | "tonu_trip"
```

2. Destructive annotation (~line 773) — add `| "tonu_trip"` to the `destructive` list.

3. Id alias (~line 1876) — add `tonu_trip` to the `("trip_id", "id")` arm.

4. Tool schema, next to `cancel_trip`:

```rust
            {
                "name": "tonu_trip",
                "description": "End a dispatched trip as TONU (Truck Ordered Not Used): the truck rolled but was released before loading. Truncates the trip to the last stop actually reached, stamps the release time so the dock wait bills as detention, assigns all miles to deadhead, moves the load to 'tonu' and archives its rate items. Valid only from 'dispatched' — use cancel_trip before dispatch, divert_trip once the pickup has been departed. Supply 'waypoint' with where the driver stopped when no stop was reached; it is required in that case.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "waypoint": {
                            "type": "object",
                            "description": "Where the truck actually stopped. Required when no stop was reached.",
                            "properties": {
                                "facility_id": { "type": "string", "format": "uuid" },
                                "facility_name": { "type": "string" },
                                "address": { "type": "string" },
                                "timezone": { "type": "string", "description": "IANA timezone, e.g. America/Chicago" },
                                "actual_arrive": { "type": "string", "description": "Naive local datetime, e.g. 2026-06-01T06:30:00" },
                                "actual_depart": { "type": "string" },
                                "notes": { "type": "string" }
                            },
                            "required": ["timezone"]
                        },
                        "occurred_at": { "type": "string", "description": "When the driver was released; naive local in the truncation stop's timezone. Defaults to now." },
                        "reason": { "type": "string" }
                    },
                    "required": ["trip_id"]
                }
            },
```

5. Dispatch arm, next to `"cancel_trip"`:

```rust
        "tonu_trip" => tool_tonu_trip(state, args).await,
```

6. Handler, next to `tool_cancel_trip`:

```rust
async fn tool_tonu_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    let req: crate::services::trip_lifecycle::TonuRequest =
        serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
    let result = crate::services::trip_lifecycle::tonu(state, trip_id, req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(result))
}
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cargo test --test tonu_test 2>&1 | tail -30
```

Expected: all 7 tests PASS.

```bash
cargo test 2>&1 | tail -20
cargo clippy --all-targets 2>&1 | grep -E "^error" | head
```

Expected: full suite PASS, clippy clean.

- [ ] **Step 8: Commit**

```bash
git add src/services/trip_lifecycle.rs src/db/load_ops.rs src/events/mod.rs \
        src/api/fleet_portal/data.rs src/api/fleet_portal/mod.rs \
        src/api/fleet_portal/mcp.rs src/api/mod.rs src/api/facilities.rs tests/tonu_test.rs
git commit -s -m "feat(trips): TONU outcome for a dispatched trip released before loading

Truncates the trip to the last stop actually reached, stamps the release
time so the dock wait bills as detention, assigns every mile to deadhead
(a TONU trip has zero loaded miles by construction), moves the load to
tonu and archives its rate items. Never auto-dispatches: after a TONU the
truck is not where the plan assumed, and auto-dispatch does not recompute
mileage."
```

---

### Task 6: The `divert` verb, its surfaces, and its tests

**Files:**
- Modify: `src/services/trip_lifecycle.rs` (`DivertReason`, `DivertRequest`, `DivertResult`, `divert`)
- Modify: `src/services/trip_stops.rs` (`cascade_final_stop_delivered`)
- Modify: `src/db/load_ops.rs` (`mark_load_diverted`)
- Modify: `src/events/mod.rs` (`on_trip_diverted`)
- Modify: `src/api/fleet_portal/data.rs`, `mod.rs`, `mcp.rs`
- Create: `tests/divert_test.rs`

**Interfaces:**
- Consumes: `PositionInput`, `resolve_position` (Task 5); `LoadRecord.diverted_at` etc. (Task 3).
- Produces:
  - `pub enum DivertReason { Diverted, Reconsigned, BolCorrection }` (serde `snake_case`)
  - `pub struct DivertRequest { waypoint: PositionInput, stops: Vec<TripStop>, reason: DivertReason, notes: Option<String> }`
  - `pub async fn divert(state: &AppState, trip_id: Uuid, req: DivertRequest) -> Result<DivertResult, AppError>`

- [ ] **Step 1: Write the failing integration tests**

Create `tests/divert_test.rs` with the same helpers copied from `tests/tonu_test.rs` (`setup`, `setup_owner`, `create_test_facility`, `create_driver`, `create_truck`, `stop_json`, `dispatched_trip`), plus:

```rust
/// A trip that has departed its pickup — freight aboard, status `in_transit`.
/// Departure from the first `Pickup` is exactly what promotes the status, so
/// this is the boundary between TONU territory and diversion territory.
async fn in_transit_trip(
    server: &TestServer, token: &str, load_number: &str,
) -> (String, String, String) {
    let (load_id, trip_id, driver_id) = dispatched_trip(server, token, load_number).await;

    let arrive = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/arrive"))
        .authorization_bearer(token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" }))
        .await;
    assert_eq!(arrive.status_code(), 200, "arrive failed: {}", arrive.text());

    let depart = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/depart"))
        .authorization_bearer(token)
        .json(&serde_json::json!({ "actual_depart": "2026-06-01T09:00:00" }))
        .await;
    assert_eq!(depart.status_code(), 200, "depart failed: {}", depart.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(token).await.json();
    assert_eq!(trip["status"], "in_transit", "departing the pickup starts transit");

    (load_id, trip_id, driver_id)
}
```

Then:

```rust
// tests/divert_test.rs
//
// Diversion: a load is cancelled, reconsigned, or corrected after the driver
// departs the shipper with freight aboard. The trip keeps running to a new
// destination; only the plan changes.
//
// The `waypoint` is mandatory because ORS routes waypoint to waypoint. A trip
// recomputed as stop0 -> new_destination draws the path you would have taken
// had you known at the dock, and every mile of backtracking disappears —
// silently, because the result still looks plausible.

#[tokio::test]
async fn test_divert_inserts_the_waypoint_between_history_and_the_new_destination() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581490").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "notes": "consignee refused; returning to shipper",
            "waypoint": {
                "facility_name": "Salina Truck Stop", "address": "Salina, KS",
                "timezone": "America/Chicago",
                "actual_arrive": "2026-06-01T14:00:00",
                "actual_depart": "2026-06-01T17:00:00"
            },
            "stops": [{
                "stop_type": "delivery",
                "facility_name": "Return Dock", "address": "Kansas City, MO",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 3, "kept pickup, waypoint, new delivery");
    assert_eq!(stops[0]["stop_type"], "pickup", "departed history is immutable");
    assert_eq!(stops[1]["stop_type"], "waypoint");
    assert_eq!(stops[1]["name"], "Salina Truck Stop");
    assert_eq!(stops[2]["name"], "Return Dock");
    // The ordering IS the backtrack fix: routing walks pickup -> Salina ->
    // Kansas City, so the 200 miles out and 200 back are both counted. Routing
    // itself cannot be asserted here (tests run without ORS).
    assert_eq!(trip["status"], "in_transit", "the trip keeps running");

    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert!(load["diverted_at"].is_string());
    assert_eq!(load["diversion_reason"], "diverted");
    assert_eq!(load["diversion_notes"], "consignee refused; returning to shipper");
    assert_eq!(load["rate_items"].as_array().unwrap().len(), 1,
               "unlike TONU, the line haul is at least partly earned");
}

#[tokio::test]
async fn test_bol_correction_does_not_flag_the_load_as_diverted() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581491").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "bol_correction",
            "waypoint": {
                "facility_name": "Divergence Point", "address": "Springfield, IL",
                "timezone": "America/Chicago"
            },
            "stops": [{
                "stop_type": "delivery",
                "facility_name": "BOL Consignee", "address": "St Louis, MO",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert!(load["diverted_at"].is_null(),
        "nothing was diverted — the plan was wrong from the start, and flagging it \
         would poison the query the field exists to answer");
}

#[tokio::test]
async fn test_divert_requires_a_waypoint() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581492").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "reason": "reconsigned", "stops": [] }))
        .await;
    assert_eq!(resp.status_code(), 422,
        "without the divergence point the recomputed route erases the backtrack: {}",
        resp.text());
}

#[tokio::test]
async fn test_departing_a_waypoint_does_not_deliver_the_trip() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581493").await;

    // Pulled over, disposition unknown: no destination yet.
    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": {
                "facility_name": "Holding Yard", "address": "Topeka, KS",
                "timezone": "America/Chicago",
                "actual_arrive": "2026-06-01T14:00:00"
            },
            "stops": []
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    // The waypoint is now the highest-sequence stop. Departing it must not fire
    // the delivered cascade — no freight came off the truck.
    let depart = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_depart": "2026-06-01T18:00:00" }))
        .await;
    assert_eq!(depart.status_code(), 200, "depart failed: {}", depart.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["status"], "in_transit", "a waypoint cannot deliver a load");
    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert_ne!(load["status"], "delivered");
}

#[tokio::test]
async fn test_divert_refuses_to_rewrite_an_arrived_at_stop() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581494").await;

    // First divert gives the trip a delivery stop, then the driver arrives at it.
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "waypoint": { "facility_name": "Point A", "address": "Salina, KS",
                          "timezone": "America/Chicago" },
            "stops": [{ "stop_type": "delivery", "facility_name": "Consignee B",
                        "address": "Wichita, KS", "timezone": "America/Chicago" }]
        })).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/2/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T20:00:00" })).await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "waypoint": { "facility_name": "Point C", "address": "Emporia, KS",
                          "timezone": "America/Chicago" },
            "stops": []
        })).await;
    assert_eq!(resp.status_code(), 422,
        "a stop the driver has already reached is history, not plan: {}", resp.text());
}

#[tokio::test]
async fn test_divert_rejects_a_dispatched_trip_and_names_tonu() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = dispatched_trip(&server, &token, "4581495").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": { "facility_name": "X", "address": "Joliet, IL",
                          "timezone": "America/Chicago" },
            "stops": []
        })).await;
    assert_eq!(resp.status_code(), 409);
    assert!(resp.text().contains("tonu_trip"),
            "no freight is aboard yet: {}", resp.text());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --test divert_test 2>&1 | tail -20
```

Expected: FAIL — route missing.

- [ ] **Step 3: Add the event and the load marker**

In `src/events/mod.rs`:

```rust
pub async fn on_trip_diverted(
    db: &DbClient, trip_id: Uuid, reason: &str, notes: Option<String>, new_stop_count: usize,
) {
    let payload = serde_json::json!({
        "reason": reason, "notes": notes, "new_stop_count": new_stop_count,
    });
    let _ = db.append_event("trip", trip_id, "trip.diverted", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, reason, "trip diverted");
}
```

In `src/db/load_ops.rs`:

```rust
    /// Flag a load as diverted. Only `diverted` and `reconsigned` reach here — a
    /// `bol_correction` corrects a plan that was wrong from the start and must
    /// not pollute "which loads were diverted".
    pub async fn mark_load_diverted(
        &self, id: Uuid, reason: &str, notes: Option<String>,
    ) -> Result<LoadRecord, AppError> {
        let mut record = self.get_load_by_id(id).await?;
        record.diverted_at = Some(Utc::now().to_rfc3339());
        record.diversion_reason = Some(reason.to_string());
        if notes.is_some() { record.diversion_notes = notes; }
        record.updated_at = Utc::now();
        self.upsert_load(&record).await?;
        Ok(record)
    }
```

- [ ] **Step 4: Guard the delivered cascade**

In `src/services/trip_stops.rs`, in `cascade_final_stop_delivered`, insert right after the `let Ok(current) = ... else { return };` line:

```rust
    // A waypoint is a routing point, not a delivery. After a hold-only divert it
    // is the highest-sequence stop, so without this guard the driver leaving the
    // truck stop would silently mark the load delivered.
    if current.stops.iter().any(|s| s.sequence == seq
        && s.stop_type == crate::models::TripStopType::Waypoint)
    {
        return;
    }
```

- [ ] **Step 5: Implement the verb**

In `src/services/trip_lifecycle.rs`:

```rust
#[derive(Debug, Clone, Copy, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DivertReason {
    /// The broker cancelled mid-transit.
    Diverted,
    /// The broker nominated a different consignee.
    Reconsigned,
    /// The BOL disagreed with the rate confirmation and the BOL wins. Nothing
    /// was diverted — the plan was wrong from the start.
    BolCorrection,
}

impl DivertReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Diverted => "diverted",
            Self::Reconsigned => "reconsigned",
            Self::BolCorrection => "bol_correction",
        }
    }
    /// Whether this reason represents a commercial diversion with a fee to
    /// negotiate, and therefore flags the load.
    fn flags_the_load(&self) -> bool { !matches!(self, Self::BolCorrection) }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DivertRequest {
    /// Where the old plan and the new plan diverged. Always required: an
    /// in-transit truck is by definition between the trip's existing points.
    pub waypoint: PositionInput,
    /// Replacement for every stop the driver has not reached. May be empty —
    /// "pulled over, disposition unknown".
    #[serde(default)]
    pub stops: Vec<PositionInput>,
    pub reason: DivertReason,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, serde::Serialize, ToSchema)]
pub struct DivertResult {
    #[serde(flatten)]
    pub trip: TripRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mileage_recompute_warning: Option<String>,
}

/// Re-target an in-transit trip. The trip keeps running; only the plan changes.
pub async fn divert(
    state: &AppState,
    trip_id: Uuid,
    req: DivertRequest,
) -> Result<DivertResult, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    match existing.status {
        TripStatus::InTransit => {}
        TripStatus::Dispatched => {
            return Err(AppError::Conflict(
                "trip has not departed its pickup, so no freight is aboard — use tonu_trip \
                 to end it, or update_trip to change a stop it has not reached".into()));
        }
        s => {
            return Err(AppError::Conflict(format!(
                "cannot divert a trip with status '{}'", s.as_str())));
        }
    }
    if existing.settlement_ref.is_some() {
        return Err(AppError::Conflict("trip is settled; miles and pay are frozen".into()));
    }

    // History is the contiguous prefix up to the last stop the driver reached —
    // NOT every stop that happens to have an actual_arrive. Filtering would let
    // a later arrived-at stop survive while dropping an earlier unreached one,
    // producing a stop list that never happened in that order.
    let last_reached = existing.stops.iter()
        .filter(|s| s.actual_arrive.is_some())
        .max_by_key(|s| s.sequence)
        .map(|s| s.sequence);
    let kept: Vec<crate::models::TripStop> = match last_reached {
        Some(seq) => existing.stops.iter().filter(|s| s.sequence <= seq).cloned().collect(),
        None => vec![],
    };
    if kept.len() == existing.stops.len() && !existing.stops.is_empty() {
        return Err(AppError::UnprocessableEntity(
            "every stop on this trip has been arrived at; clear the actuals on the stop \
             you mean to replace before diverting".into()));
    }

    // Resolve every position before the first write.
    let mut stops = kept;
    stops.push(resolve_position(
        state, req.waypoint, stops.len() as u32, crate::models::TripStopType::Waypoint,
    ).await?);
    for pos in req.stops {
        let seq = stops.len() as u32;
        // Destinations default to `delivery`; a cross-dock hand-off can override
        // to `relay` via the position's own `stop_type`.
        stops.push(resolve_position(
            state, pos, seq, crate::models::TripStopType::Delivery,
        ).await?);
    }
    for (i, s) in stops.iter_mut().enumerate() { s.sequence = i as u32; }

    state.db.update_trip_metadata(trip_id, None, None, Some(stops.clone()), None, None, None).await?;
    let warning = match crate::api::trips::compute_and_persist_mileage(state, trip_id).await {
        Ok(_) => None,
        Err(e) => Some(format!("mileage not recomputed: {e}")),
    };

    if req.reason.flags_the_load() {
        if let Some(load_id) = existing.load_id {
            if let Err(e) = state.db.mark_load_diverted(
                load_id, req.reason.as_str(), req.notes.clone(),
            ).await {
                tracing::warn!(%load_id, error = %e, "load not flagged as diverted");
            }
        }
    }

    events::on_trip_diverted(
        &state.db, trip_id, req.reason.as_str(), req.notes, stops.len(),
    ).await;

    let trip = state.db.get_trip(trip_id).await?;
    Ok(DivertResult { trip, mileage_recompute_warning: warning })
}
```

`resolve_position` already takes `default_stop_type` from Task 5, so no signature change is needed here.

- [ ] **Step 6: Add the REST handler and route**

In `src/api/fleet_portal/data.rs`, next to `tonu_trip`:

```rust
#[utoipa::path(
    post,
    path = "/fleet/api/v1/trips/{id}/divert",
    params(("id" = Uuid, Path, description = "Trip UUID")),
    request_body(content = DivertRequest, description = "Divergence waypoint, replacement stops and reason"),
    responses(
        (status = 200, description = "Trip re-targeted", body = DivertResult),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — trip is not in transit, or is settled"),
        (status = 422, description = "Waypoint unresolvable, or a reached stop would be replaced"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn divert_trip(
    state: State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    id: Path<Uuid>,
    Json(body): Json<crate::services::trip_lifecycle::DivertRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("trips:write")?;
    let result = crate::services::trip_lifecycle::divert(&state, id.0, body).await?;
    Ok(Json(result))
}
```

The body is **not** optional here: `waypoint` and `reason` are required, and axum's `Json` extractor rejects a request missing them with 422 — which is the status `test_divert_requires_a_waypoint` asserts.

Route in `src/api/fleet_portal/mod.rs`, next to the `tonu` route:

```rust
        .route("/fleet/api/v1/trips/{id}/divert", post(data::divert_trip))
```

Register `divert_trip` in the utoipa `paths(...)` list in `src/api/mod.rs`.

- [ ] **Step 7: Add the MCP tool**

Five edits in `src/api/fleet_portal/mcp.rs`.

1. Scope map — extend the `"trips:write"` arm again:

```rust
        | "dispatch_trip" | "undispatch_trip" | "cancel_trip" | "complete_trip"
        | "tonu_trip" | "divert_trip"
```

2. Destructive annotation — add `| "divert_trip"` to the `destructive` list.

3. Id alias — add `divert_trip` to the `("trip_id", "id")` arm.

4. Dispatch arm, next to `"tonu_trip"`:

```rust
        "divert_trip" => tool_divert_trip(state, args).await,
```

5. Handler, next to `tool_tonu_trip`:

```rust
async fn tool_divert_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    let req: crate::services::trip_lifecycle::DivertRequest =
        serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
    let result = crate::services::trip_lifecycle::divert(state, trip_id, req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(result))
}
```

6. Tool schema, next to `tonu_trip`:

```rust
            {
                "name": "divert_trip",
                "description": "Re-target an in-transit trip. Replaces every stop the driver has not reached with a new destination, keeping arrived-at stops as immutable history. 'waypoint' is REQUIRED and marks where the old plan and the new plan diverged: routing walks waypoint to waypoint, so without it any backtracking is silently erased from the recomputed miles. reason 'diverted' or 'reconsigned' flags the load for a diversion fee; 'bol_correction' does not. Valid only from 'in_transit' — use tonu_trip before the pickup is departed. 'stops' may be empty for 'pulled over, disposition unknown'.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "waypoint": {
                            "type": "object",
                            "properties": {
                                "facility_id": { "type": "string", "format": "uuid" },
                                "facility_name": { "type": "string" },
                                "address": { "type": "string" },
                                "timezone": { "type": "string" },
                                "actual_arrive": { "type": "string" },
                                "actual_depart": { "type": "string" },
                                "notes": { "type": "string" }
                            },
                            "required": ["timezone"]
                        },
                        "stops": {
                            "type": "array",
                            "description": "New destinations, in order. Same shape as waypoint.",
                            "items": { "type": "object" }
                        },
                        "reason": {
                            "type": "string",
                            "enum": ["diverted", "reconsigned", "bol_correction"]
                        },
                        "notes": { "type": "string" }
                    },
                    "required": ["trip_id", "waypoint", "reason"]
                }
            },
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cargo test --test divert_test 2>&1 | tail -30
cargo test 2>&1 | tail -20
cargo clippy --all-targets 2>&1 | grep -E "^error" | head
```

Expected: all 6 divert tests PASS, full suite PASS, clippy clean.

- [ ] **Step 8: Commit**

```bash
git add src/services src/db/load_ops.rs src/events/mod.rs src/api tests/divert_test.rs
git commit -s -m "feat(trips): divert_trip re-targets an in-transit trip

The waypoint is mandatory because ORS routes waypoint to waypoint: a trip
recomputed as stop0 -> new_destination draws the path you would have taken
had you known at the dock, erasing every mile of backtracking. Also guards
cascade_final_stop_delivered so departing a waypoint cannot mark a load
delivered."
```

---

### Task 7: Record the invariants

**Files:**
- Modify: `AGENTS.md`

**Interfaces:**
- Consumes: everything above.
- Produces: nothing.

- [ ] **Step 1: Add the invariants**

Append to the invariants list in `AGENTS.md`, matching the existing entry style (claim — Why — How to apply):

```markdown
- **A trip's stop list must end at the last position the truck actually reached.** ORS routes waypoint to waypoint, so any recompute that skips where the truck really got to draws the route you *would* have taken. A load diverted 200 miles out and turned back to the shipper drove 400; recomputed as `stop0 -> shipper` it claims whatever that pair happens to be, and the number still looks plausible, so nobody checks it. `TripStopType::Waypoint` exists to anchor this: `tonu_trip` truncates to the last stop with an `actual_arrive` (or records a `Waypoint` when none was reached), and `divert_trip` requires one on every call because an in-transit truck is by definition between the trip's existing points. — Why: mileage bugs that under-report are loud (a driver complains); mileage bugs that mis-route are silent, because a wrong-but-plausible number passes every check a human makes. — How to apply: whenever a plan changes mid-execution, ask what the router will now walk. If the answer skips a place the vehicle physically went, you need a waypoint, not a recompute.

- **"Loaded" means under dispatch performing the move, not cargo weight > 0.** Deadhead is getting to the work; loaded is doing it. That is why an empty move's haul leg is genuinely loaded miles — a dispatched movement with its own BOL and POD whose commodity happens to be nothing — while the run from your own terminal to a shipper is not. `compute_trip_mileage` approximates the rule by classifying a leg as deadhead only when it originates from a `previous_trip_id`, which holds for the empty move and fails for a trip that starts at an explicit `Origin` stop. `tonu` does not delegate the split at all: a TONU trip has zero loaded miles by construction, so it routes the truncated stop list and assigns the whole figure to deadhead. — Why: the words invite a cargo-weight reading, and under that reading both cases come out backwards. — How to apply: before classifying a leg, ask whether the truck was performing the customer's move or travelling to be able to.

- **Counting records is not the same as counting the thing the fee is for.** `compute_driver_pay` charged `extra_stop_fee` per stop beyond two with no notion of stop kind, so a trip that stopped for fuel paid the driver an extra-stop fee for fueling — live, for as long as fuel stops have been modelled. The fix is `TripStopType::is_service_stop()` (true for `Pickup`, `Delivery`, `Relay`, `EmptyMove`), applied to the extra-stop count only; detention still iterates every stop, because a driver held three hours at a waypoint is owed for it. — Why: the defect is invisible until a new stop *kind* is added for a non-work reason, at which point it looks like a new bug rather than an old one. — How to apply: when a fee is computed from `collection.len()`, name what the fee is actually for and filter to that, before adding a member that does not qualify.

- **A status-machine addition is not complete until the terminal-status lists know about it.** `LoadStatus::Tonu` had to be added to `list_loads_needing_routing` and `list_unrouted_loads_for_facility`, whose filters are `miles IS NULL AND status NOT IN (...)`. A TONU'd load has no miles and never will, so omitting it makes the load re-enter the routing requeue on every startup, forever — the same silent permanent zombie the administrative-loads work hit in #411. — Why: these filters enumerate terminal states positively rather than deriving them, so a new terminal state is invisible to them by default. — How to apply: `grep` for `status NOT IN` and `status IN` whenever you add a status, and add a test that the new state is excluded, not just a code change.
```

- [ ] **Step 2: Verify the whole suite one more time**

```bash
cargo test 2>&1 | tail -20
cargo clippy --all-targets 2>&1 | grep -E "^error" | head
```

Expected: PASS, clippy clean.

- [ ] **Step 3: Commit**

```bash
git add AGENTS.md
git commit -s -m "docs: record TONU/diversion invariants in AGENTS.md"
```

---

## Deploy notes

- **Driver pay changes on unsettled trips.** Trips carrying `Fuel`, `Origin`, `Terminal` or `Maintenance` stops will compute lower `extra_stop_pay` after this ships. Settled trips are frozen by `driver_pay_snapshot` and do not move. Announce it before deploy rather than letting it surface on a settlement sheet.
- **Trips with deadhead but no loaded miles now produce a pay row** where they previously produced none. This is the fix that makes TONU payable at all.
- **Load table migration** adds four columns on first startup; the log line is `migrating loads table: adding 4 column(s)`.
- No new environment variables. No Dockerfile change. No static assets touched, so PWA cache stamps stay where they are.
