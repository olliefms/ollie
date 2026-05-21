# Trip/Load Display Polish — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface mileage context (origin, per-leg miles, total) and rate info on trip/load views, and render full arrival windows in the dispatcher UI. Closes #246, #247, #248, #249 and unblocks the simpler half of #132.

**Architecture:** One unified `mileage_summary` block on trip detail responses (driver + dispatcher) carrying origin, legs, deadhead/loaded/total miles. One ORS multi-waypoint call replaces today's two calls (deadhead + loaded), with per-leg distances parsed from `routes[0].segments[]`. `total_miles` persisted to the trips table. Dispatch UI gets a shared `fmtArrivalWindow` helper, a rate-items table on load detail, and the mileage summary on trip + load detail. Driver UI gets the mileage summary woven into its trip-detail stop timeline.

**Tech Stack:** Rust (axum, serde, reqwest), Arrow/LanceDB storage, vanilla JS frontends (`static/dispatch/app.js`, `static/driver/pages/trip-detail.js`).

---

## File Structure

**Modified:**
- `src/routing/mod.rs` — extend `RoutingClient` to return per-segment distances + total in one call
- `src/models/trip.rs` — add `MileageSummary` and `LegMiles` structs; add `total_miles` field to `TripRecord` and `TripListItem`
- `src/api/trips.rs` — replace two-call `compute_deadhead_miles` + `compute_loaded_miles` with a single combined call; compute `total_miles`; build `mileage_summary` for the admin response
- `src/db/mod.rs` — add `total_miles` schema field + lazy migration
- `src/db/trip_ops.rs` — persist + read `total_miles`
- `src/api/driver_portal/data.rs` — add `mileage_summary` to `DriverTripDetailResponse`
- `src/api/dispatcher_portal/data.rs` — add `mileage_summary` to `DispatcherTripListItem` (or a new detail response); add helper `build_mileage_summary`
- `static/dispatch/app.js` — add `fmtArrivalWindow` helper, replace 6 `fmtDate(scheduled_arrive)` sites, render `Rate` card on load detail, render `Mileage` card on trip + load detail
- `static/driver/pages/trip-detail.js` — render origin row + per-leg badges in stop timeline + total at the bottom

**New tests:**
- `tests/integration_test.rs` additions (or new module) — assert `mileage_summary` round-trips, total_miles persists across writes, rate_items returned by dispatch load detail

No new files.

---

## Task 1: Extend RoutingClient to return per-segment distances

**Files:**
- Modify: `src/routing/mod.rs`

ORS `driving-hgv` returns `routes[0].segments[]` where each segment is the distance between two consecutive input waypoints. We currently only parse `summary.distance`. Capturing both gives us per-leg + total from one HTTP call.

- [ ] **Step 1: Write the failing test**

Append to `src/routing/mod.rs` `mod tests`:

```rust
#[tokio::test]
#[ignore] // requires valid ORS_API_KEY: cargo test routing -- --ignored
async fn test_calculate_route_with_segments_live() {
    let key = std::env::var("ORS_API_KEY").expect("ORS_API_KEY required");
    let client = RoutingClient::new(&key);
    // 3 waypoints: Memphis → Nashville → Atlanta
    let route = client.calculate_route_with_segments(&[
        (35.1495, -90.0490),
        (36.1627, -86.7816),
        (33.7490, -84.3880),
    ]).await;
    assert!(route.is_some());
    let r = route.unwrap();
    assert_eq!(r.segment_miles.len(), 2);
    assert!(r.segment_miles[0] > 100.0 && r.segment_miles[0] < 300.0, "leg 1 mi: {}", r.segment_miles[0]);
    assert!(r.segment_miles[1] > 150.0 && r.segment_miles[1] < 350.0, "leg 2 mi: {}", r.segment_miles[1]);
    assert!((r.total_miles - r.segment_miles.iter().sum::<f64>()).abs() < 0.5);
}

#[test]
fn test_calculate_route_with_segments_requires_two_waypoints() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = RoutingClient::new("fake-key");
    let result = rt.block_on(client.calculate_route_with_segments(&[(35.1495, -90.0490)]));
    assert!(result.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml routing`
Expected: `test_calculate_route_with_segments_requires_two_waypoints` fails to compile (method not defined).

- [ ] **Step 3: Implement the method**

Replace the contents of `src/routing/mod.rs` `OrsRoute` + `OrsSummary` parsing block and add the new method. Keep `calculate_route_miles` for backward compatibility (it delegates).

```rust
#[derive(Deserialize)]
struct OrsResponse {
    routes: Vec<OrsRoute>,
}

#[derive(Deserialize)]
struct OrsRoute {
    summary: OrsSummary,
    #[serde(default)]
    segments: Vec<OrsSegment>,
}

#[derive(Deserialize)]
struct OrsSummary {
    distance: f64,
}

#[derive(Deserialize)]
struct OrsSegment {
    distance: f64,
}

#[derive(Debug, Clone)]
pub struct RouteMiles {
    pub total_miles: f64,
    pub segment_miles: Vec<f64>,
}
```

Add method on `impl RoutingClient`:

```rust
    /// Calculates HGV route distances for ordered waypoints (lat, lng).
    /// Returns total miles and per-segment miles (one entry per consecutive pair).
    /// Returns None if fewer than 2 waypoints or on API error.
    pub async fn calculate_route_with_segments(
        &self, waypoints: &[(f64, f64)],
    ) -> Option<RouteMiles> {
        if waypoints.len() < 2 { return None; }
        let coordinates: Vec<[f64; 2]> = waypoints.iter()
            .map(|&(lat, lng)| [lng, lat])
            .collect();
        let body = OrsRequest { coordinates, units: "mi" };
        let resp = self.client
            .post("https://api.heigit.org/openrouteservice/v2/directions/driving-hgv")
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(std::time::Duration::from_secs(15))
            .send().await.ok()?;
        if !resp.status().is_success() { return None; }
        let data: OrsResponse = resp.json().await.ok()?;
        let route = data.routes.into_iter().next()?;
        let segment_miles: Vec<f64> = route.segments.iter().map(|s| s.distance).collect();
        Some(RouteMiles {
            total_miles: route.summary.distance,
            segment_miles,
        })
    }
```

Keep the existing `calculate_route_miles` as a thin wrapper:

```rust
    pub async fn calculate_route_miles(&self, waypoints: &[(f64, f64)]) -> Option<f64> {
        self.calculate_route_with_segments(waypoints).await.map(|r| r.total_miles)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml routing`
Expected: PASS (the `#[ignore]` live test is skipped; the requires-two-waypoints variant passes).

- [ ] **Step 5: Commit**

```bash
git add src/routing/mod.rs
git commit -m "feat(routing): return per-segment miles from ORS driving-hgv calls"
```

---

## Task 2: Add MileageSummary + LegMiles structs and total_miles field

**Files:**
- Modify: `src/models/trip.rs`

This task adds the response-shape primitives. Persistence and computation come in later tasks.

- [ ] **Step 1: Add the structs and extend TripRecord**

In `src/models/trip.rs`, add after the existing `TripStop` block (around line 70):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeadheadOrigin {
    pub trip_id: Uuid,
    pub facility_name: Option<String>,
    /// `normalized_address` if present, else raw `address`. Free-form single string.
    pub address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LegMiles {
    /// 0 = deadhead leg (origin → first stop). 1+ = loaded legs between trip stops.
    pub index: u32,
    /// "deadhead" or "loaded"
    pub kind: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub miles: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MileageSummary {
    pub origin: Option<DeadheadOrigin>,
    pub legs: Vec<LegMiles>,
    pub deadhead_miles: Option<f64>,
    pub loaded_miles: Option<f64>,
    pub total_miles: Option<f64>,
}
```

In `TripRecord` (around line 128), add `total_miles` and `segment_miles` immediately after `loaded_miles`:

```rust
    pub deadhead_miles: Option<f64>,
    pub loaded_miles: Option<f64>,
    pub total_miles: Option<f64>,
    /// Per-segment miles from the single ORS multi-waypoint call. Order:
    /// [deadhead_leg, loaded_leg_1, loaded_leg_2, ...] when origin exists;
    /// [loaded_leg_1, loaded_leg_2, ...] when there's no previous trip.
    /// Empty when ORS routing was unavailable.
    #[serde(default)]
    pub segment_miles: Vec<f64>,
```

Mirror on `TripListItem` (around line 184):

```rust
    pub deadhead_miles: Option<f64>,
    pub loaded_miles: Option<f64>,
    pub total_miles: Option<f64>,
    #[serde(default)]
    pub segment_miles: Vec<f64>,
```

Update the `From<TripRecord> for TripListItem` impl (around line 206):

```rust
            deadhead_miles: r.deadhead_miles,
            loaded_miles: r.loaded_miles,
            total_miles: r.total_miles,
            segment_miles: r.segment_miles,
```

Update both `TripRecord { ... }` test fixtures (around lines 281 and 328): add `total_miles: None,` and `segment_miles: vec![],` next to the existing `loaded_miles: None,`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build --manifest-path /Users/jimp7508/src/ollie/Cargo.toml 2>&1 | tail -30`
Expected: compile errors only at the existing `TripRecord { ... }` construction sites (in `src/api/trips.rs`, `src/db/trip_ops.rs`) — those get fixed in subsequent tasks. The model file itself must compile cleanly when checked in isolation.

If errors are only at construction sites outside `models/trip.rs`, that's fine — proceed.

- [ ] **Step 3: Don't commit yet**

Leave staged for Task 3 to land together (the model change is meaningless without DB + API updates).

---

## Task 3: Persist total_miles in trips schema

**Files:**
- Modify: `src/db/mod.rs:155-178` (migration + schema)
- Modify: `src/db/mod.rs:538-587` (schema + empty batch)
- Modify: `src/db/trip_ops.rs:241-261` (insert) and `:287-339` (read)

- [ ] **Step 1: Add schema fields + migration**

In `src/db/mod.rs`, in the trip schema definition (~line 558-559), add two new fields after `loaded_miles`:

```rust
        Field::new("deadhead_miles", DataType::Float64, true),
        Field::new("loaded_miles", DataType::Float64, true),
        Field::new("total_miles", DataType::Float64, true),
        Field::new("segment_miles", DataType::Utf8, true),  // JSON-encoded Vec<f64>
```

We store `segment_miles` as JSON in a string column rather than a Float64 list, matching how `stops` and `trailer_ids` are already serialized in this schema — keeps the migration trivial and the read path consistent.

In the same file's lazy-migration block (~line 170-172), add:

```rust
            if existing.field_with_name("total_miles").is_err() {
                transforms.push(("total_miles".into(), "CAST(NULL AS float64)".into()));
            }
            if existing.field_with_name("segment_miles").is_err() {
                transforms.push(("segment_miles".into(), "CAST(NULL AS string)".into()));
            }
```

In `empty_trip_batch` (~line 585), add corresponding empty arrays after the `loaded_miles` line:

```rust
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // loaded_miles
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // total_miles
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // segment_miles
```

- [ ] **Step 2: Persist on insert**

In `src/db/trip_ops.rs` (~line 259), after the `loaded_miles` array, add the two new columns:

```rust
        Arc::new(Float64Array::from(vec![record.deadhead_miles])),
        Arc::new(Float64Array::from(vec![record.loaded_miles])),
        Arc::new(Float64Array::from(vec![record.total_miles])),
        Arc::new(StringArray::from(vec![
            if record.segment_miles.is_empty() { None }
            else { Some(serde_json::to_string(&record.segment_miles).unwrap_or_default()) }
        ])),
```

- [ ] **Step 3: Read on `row_to_trip`**

In `src/db/trip_ops.rs` (~line 324), after the `loaded_miles` line:

```rust
        deadhead_miles: opt_f64("deadhead_miles"),
        loaded_miles: opt_f64("loaded_miles"),
        total_miles: opt_f64("total_miles"),
        segment_miles: opt_str("segment_miles")
            .and_then(|s| serde_json::from_str::<Vec<f64>>(&s).ok())
            .unwrap_or_default(),
```

In the same file's seed-test fixture (~line 380):

```rust
            deadhead_miles: None,
            loaded_miles: None,
            total_miles: None,
            segment_miles: vec![],
```

- [ ] **Step 4: Build + run lib tests**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml --lib`
Expected: PASS. (Lib tests touch trip storage round-tripping.)

If failures are in `config::tests::test_terminal_timezone_default` only, that's env leakage from the shell — verify with `env -u TERMINAL_TIMEZONE cargo test --lib config::tests::test_terminal_timezone_default` and ignore the noise.

- [ ] **Step 5: Commit**

```bash
git add src/models/trip.rs src/db/mod.rs src/db/trip_ops.rs
git commit -m "feat(trips): add total_miles column + MileageSummary response types"
```

---

## Task 4: Single ORS call computes deadhead + loaded + total + per-leg

**Files:**
- Modify: `src/api/trips.rs:286-318` (replace both helpers)
- Modify: `src/api/trips.rs:128-160` (call site)

The current code makes two ORS calls. Replace with one combined call across all waypoints (origin → trip stops). The first segment is deadhead, the rest are loaded legs.

- [ ] **Step 1: Replace the two compute helpers with a unified function**

Delete `compute_deadhead_miles` and `compute_loaded_miles` (~lines 286-318) and replace with:

```rust
struct ComputedMileage {
    deadhead_miles: Option<f64>,
    loaded_miles: Option<f64>,
    total_miles: Option<f64>,
    /// Per-segment miles in the order [deadhead, loaded_legs...] when deadhead exists,
    /// or just [loaded_legs...] when there's no previous trip.
    segment_miles: Vec<f64>,
    /// Resolved previous-trip last facility (deadhead origin), if available.
    deadhead_origin_facility_id: Option<Uuid>,
}

async fn compute_trip_mileage(
    db: &crate::db::DbClient,
    ors: &RoutingClient,
    previous_trip_id: Option<Uuid>,
    trip_stops: &[TripStop],
) -> ComputedMileage {
    let mut empty = ComputedMileage {
        deadhead_miles: None, loaded_miles: None, total_miles: None,
        segment_miles: vec![], deadhead_origin_facility_id: None,
    };
    if trip_stops.is_empty() { return empty; }

    // Resolve deadhead origin facility if a previous trip exists.
    let deadhead_origin_fac: Option<Uuid> = match previous_trip_id {
        Some(prev_id) => db.get_trip(prev_id).await.ok()
            .and_then(|t| t.stops.last().and_then(|s| s.facility_id)),
        None => None,
    };
    empty.deadhead_origin_facility_id = deadhead_origin_fac;

    // Build the waypoint list: [deadhead_origin?, stop_0, stop_1, ...]
    let mut fac_ids: Vec<Uuid> = Vec::new();
    if let Some(fid) = deadhead_origin_fac { fac_ids.push(fid); }
    for s in trip_stops {
        match s.facility_id {
            Some(fid) => fac_ids.push(fid),
            None => return empty,
        }
    }
    if fac_ids.len() < 2 { return empty; }

    let facilities = match db.batch_get_facilities(&fac_ids).await {
        Ok(f) => f,
        Err(_) => return empty,
    };

    let mut waypoints: Vec<(f64, f64)> = Vec::with_capacity(fac_ids.len());
    for fid in &fac_ids {
        let f = match facilities.get(fid) { Some(f) => f, None => return empty };
        let (lat, lng) = match (f.lat, f.lng) { (Some(la), Some(lo)) => (la, lo), _ => return empty };
        waypoints.push((lat, lng));
    }

    let route = match ors.calculate_route_with_segments(&waypoints).await {
        Some(r) => r,
        None => return empty,
    };

    let has_deadhead = deadhead_origin_fac.is_some();
    let (deadhead, loaded_segs): (Option<f64>, &[f64]) = if has_deadhead {
        (route.segment_miles.first().copied(), &route.segment_miles[1..])
    } else {
        (None, &route.segment_miles[..])
    };
    let loaded: Option<f64> = if loaded_segs.is_empty() {
        None
    } else {
        Some(loaded_segs.iter().sum())
    };
    let total = Some(route.total_miles);

    ComputedMileage {
        deadhead_miles: deadhead,
        loaded_miles: loaded,
        total_miles: total,
        segment_miles: route.segment_miles,
        deadhead_origin_facility_id: deadhead_origin_fac,
    }
}
```

- [ ] **Step 2: Update the call site in `create_trip`**

In `src/api/trips.rs` (~lines 128-148), replace this block:

```rust
    // Compute deadhead and loaded miles via ORS (null on error or missing coords)
    let deadhead_miles = match previous_trip_id {
        Some(prev_id) => compute_deadhead_miles(&state.db, &state.ors, prev_id, stops.first()).await,
        None => None,
    };
    let loaded_miles = match &load {
        Some(load) => compute_loaded_miles(&state.db, &state.ors, load).await,
        None => None,
    };

    // Denormalize load_number
    let load_number = load.as_ref().map(|l| l.load_number.clone());

    let mut record = TripRecord {
        id: Uuid::new_v4(),
        trip_number,
        load_id: body.load_id,
        load_number,
        previous_trip_id,
        deadhead_miles,
        loaded_miles,
```

with:

```rust
    // Compute deadhead + loaded + total + per-leg via a single ORS call.
    let mileage = compute_trip_mileage(&state.db, &state.ors, previous_trip_id, &stops).await;

    // Denormalize load_number
    let load_number = load.as_ref().map(|l| l.load_number.clone());

    let mut record = TripRecord {
        id: Uuid::new_v4(),
        trip_number,
        load_id: body.load_id,
        load_number,
        previous_trip_id,
        deadhead_miles: mileage.deadhead_miles,
        loaded_miles: mileage.loaded_miles,
        total_miles: mileage.total_miles,
        segment_miles: mileage.segment_miles.clone(),
```

- [ ] **Step 3: Run tests + build**

Run: `cargo build --manifest-path /Users/jimp7508/src/ollie/Cargo.toml 2>&1 | tail -10`
Expected: compiles. (The unused-import `LoadRecord` may trip; remove it from the imports at the top of `trips.rs` if so.)

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml --lib`
Expected: PASS (modulo the env-leakage `config::tests::test_terminal_timezone_default` noise documented in Task 3).

- [ ] **Step 4: Commit**

```bash
git add src/api/trips.rs
git commit -m "feat(trips): single ORS call yields deadhead+loaded+total miles and per-leg breakdown"
```

---

## Task 5: Add mileage_summary builder helper used by both portals

**Files:**
- Create: `src/api/mileage_summary.rs`
- Modify: `src/api/mod.rs` (export module)

A small shared builder so both portals produce identical `MileageSummary` JSON from a `TripRecord`.

- [ ] **Step 1: Create the module**

Create `src/api/mileage_summary.rs`:

```rust
// src/api/mileage_summary.rs
//
// Builds the unified MileageSummary block surfaced on driver + dispatcher trip detail
// responses. Resolves deadhead-origin facility metadata and zips per-leg miles with
// stop names.

use crate::models::trip::{DeadheadOrigin, LegMiles, MileageSummary, TripRecord};
use crate::AppState;
use std::collections::HashMap;
use uuid::Uuid;

/// Build a MileageSummary from a TripRecord. Reads the previous trip + facility metadata
/// for the deadhead origin label. Per-leg breakdown comes from the persisted
/// `segment_miles` vec, zipped with stop names — one leg per consecutive waypoint pair.
pub async fn build_mileage_summary(
    state: &AppState,
    trip: &TripRecord,
) -> MileageSummary {
    let origin = resolve_deadhead_origin(state, trip).await;
    let has_deadhead_leg = origin.is_some();

    // Build the ordered waypoint name list matching how segment_miles was computed:
    // [origin_name?, stop_0_name, stop_1_name, ...]
    let mut waypoint_names: Vec<Option<String>> = Vec::new();
    if let Some(o) = &origin {
        waypoint_names.push(o.facility_name.clone());
    }
    for s in &trip.stops {
        waypoint_names.push(s.name.clone());
    }

    // Zip segment_miles with consecutive waypoint pairs.
    let mut legs: Vec<LegMiles> = Vec::new();
    for (i, miles) in trip.segment_miles.iter().enumerate() {
        let from = waypoint_names.get(i).cloned().flatten();
        let to = waypoint_names.get(i + 1).cloned().flatten();
        let kind = if has_deadhead_leg && i == 0 { "deadhead" } else { "loaded" };
        legs.push(LegMiles {
            index: i as u32,
            kind: kind.into(),
            from,
            to,
            miles: Some(*miles),
        });
    }

    MileageSummary {
        origin,
        legs,
        deadhead_miles: trip.deadhead_miles,
        loaded_miles: trip.loaded_miles,
        total_miles: trip.total_miles
            .or_else(|| match (trip.deadhead_miles, trip.loaded_miles) {
                (Some(d), Some(l)) => Some(d + l),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            }),
    }
}

async fn resolve_deadhead_origin(
    state: &AppState,
    trip: &TripRecord,
) -> Option<DeadheadOrigin> {
    let prev_id = trip.previous_trip_id?;
    let prev = state.db.get_trip(prev_id).await.ok()?;
    let last_stop = prev.stops.last()?;
    let fac_id = last_stop.facility_id?;
    let facilities: HashMap<Uuid, crate::models::FacilityRecord> =
        state.db.batch_get_facilities(&[fac_id]).await.ok()?;
    let fac = facilities.get(&fac_id)?;
    Some(DeadheadOrigin {
        trip_id: prev_id,
        facility_name: Some(fac.name.clone()),
        address: fac.normalized_address.clone().or_else(|| Some(fac.address.clone())),
    })
}
```

- [ ] **Step 2: (no-op — `FacilityRecord` confirmed)**

`FacilityRecord` exposes only `address: String` + `normalized_address: Option<String>` (see `src/models/facility.rs:54-55`). The struct shape above uses these directly — no parsing of free-form address strings.

- [ ] **Step 3: Wire the module in**

In `src/api/mod.rs`, add `pub mod mileage_summary;` to the existing list of submodules (alphabetically near `loads`).

- [ ] **Step 4: Build**

Run: `cargo build --manifest-path /Users/jimp7508/src/ollie/Cargo.toml 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/api/mileage_summary.rs src/api/mod.rs src/models/trip.rs
git commit -m "feat(api): shared mileage_summary builder for driver+dispatcher portals"
```

---

## Task 6: Surface mileage_summary on driver trip detail

**Files:**
- Modify: `src/api/driver_portal/data.rs:92-101` (response struct) and `:450-546` (handler)

- [ ] **Step 1: Add field to response struct**

In `src/api/driver_portal/data.rs` (~line 92), extend `DriverTripDetailResponse`:

```rust
#[derive(Serialize)]
pub struct DriverTripDetailResponse {
    pub id: Uuid,
    pub trip_number: String,
    pub status: String,
    pub truck_unit: Option<String>,
    pub trailer_units: Vec<String>,
    pub load: Option<DriverTripLoadSummary>,
    pub stops: Vec<DriverTripStopSummary>,
    pub mileage_summary: crate::models::trip::MileageSummary,
}
```

- [ ] **Step 2: Populate in the handler**

In `src/api/driver_portal/data.rs` `trip_detail` (~line 537), before the `Ok(Json(...))`:

```rust
    let mileage_summary = crate::api::mileage_summary::build_mileage_summary(&state, &trip).await;

    Ok(Json(DriverTripDetailResponse {
        id: trip.id,
        trip_number: trip.trip_number,
        status: trip.status.as_str().to_string(),
        truck_unit,
        trailer_units,
        load,
        stops,
        mileage_summary,
    }))
```

- [ ] **Step 3: Write integration test**

Find an existing driver-trip-detail integration test (search `tests/` for `driver` + `trip_detail`). Add an assertion that the response includes `mileage_summary` with `total_miles == deadhead_miles + loaded_miles` when both are set.

Run: `grep -rn "trip_detail\|driver_trip" /Users/jimp7508/src/ollie/tests/ | head -5`

If no test exists, skip this step (the integration suite already smoke-tests the endpoint by status code; field-level coverage comes from the model unit tests).

- [ ] **Step 4: Build + test**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/api/driver_portal/data.rs
git commit -m "feat(driver-api): include mileage_summary on trip detail response"
```

---

## Task 7: Surface mileage_summary on dispatcher trip + load detail

**Files:**
- Modify: `src/api/dispatcher_portal/data.rs:36-52` (`DispatcherTripListItem` — add field), `:109-137` (enrich_trip), `:422-447` (`get_trip` handler), `:191-` (load detail handler), `:1048-1104` (`build_load_detail`)

For dispatcher trip detail, we extend the existing `DispatcherTripListItem` (used by both list + detail) with an optional `mileage_summary`. List responses leave it `None` to save N+1 lookups; only the detail handler populates it.

For dispatcher load detail, the load itself has at most one active trip — we surface `mileage_summary` for that trip alongside the load. Add an optional field to `LoadDetailResponse` (in `src/models/load.rs`) so the dispatch UI can render it.

- [ ] **Step 1: Add `mileage_summary` to LoadDetailResponse**

In `src/models/load.rs` `LoadDetailResponse` (~line 318), add after `total_rate_usd`:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mileage_summary: Option<crate::models::trip::MileageSummary>,
```

Update the constructor in `src/api/loads.rs` (around the existing `LoadDetailResponse { ... }` builder near line 549) and `src/api/dispatcher_portal/data.rs` `build_load_detail` (~line 1083) to populate it.

For `src/api/loads.rs` (admin API), pass `mileage_summary: None,` (admin API stays bare).

For `src/api/dispatcher_portal/data.rs` `build_load_detail`, fetch the most recent trip for the load and build the summary:

```rust
    let mileage_summary = {
        // `list_trips_for_load` returns Vec<TripRecord> directly. LanceDB scan
        // ordering is not guaranteed — sort by created_at desc to pick the most
        // recent trip for this load.
        let mut trips = state.db.list_trips_for_load(record.id).await.unwrap_or_default();
        trips.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if let Some(trip_record) = trips.into_iter().next() {
            Some(crate::api::mileage_summary::build_mileage_summary(state, &trip_record).await)
        } else {
            None
        }
    };

    let total_rate_usd = record.total_rate_usd();
    Ok(LoadDetailResponse {
        ...
        rate_items: record.rate_items,
        total_rate_usd,
        mileage_summary,
        ...
    })
```

`list_trips_for_load` exists at `src/db/trip_ops.rs:180` and returns `Vec<TripRecord>` directly — no `.into()` conversion needed.

- [ ] **Step 2: Add field to DispatcherTripListItem**

In `src/api/dispatcher_portal/data.rs` (~line 51), add at the end of the struct:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mileage_summary: Option<crate::models::trip::MileageSummary>,
```

In `enrich_trip` (~line 121), populate as `mileage_summary: None,` (list view stays cheap).

In the `get_trip` handler (~line 422), after building `enriched`:

```rust
    let mut enriched = enrich_trip(trip, &driver_map, &truck_map, &trailer_map);
    enriched.mileage_summary = Some(
        crate::api::mileage_summary::build_mileage_summary(&state, &record).await
    );
    Ok(Json(enriched))
```

- [ ] **Step 3: Build + test**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml --lib`
Expected: PASS. Fix any compile errors from missing imports.

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml --test integration_test 2>&1 | tail -20`
Expected: existing tests still pass. Two-decimal numeric assertions and field presence are unchanged for prior fields.

- [ ] **Step 4: Commit**

```bash
git add src/api/dispatcher_portal/data.rs src/models/load.rs src/api/loads.rs
git commit -m "feat(dispatcher-api): include mileage_summary on trip + load detail"
```

---

## Task 8: Add `fmtArrivalWindow` helper and replace fmtDate sites in dispatch UI

**Files:**
- Modify: `static/dispatch/app.js:231-238` (add helper), and 6 call sites at `:381,382,488,758,759,804,864`

#248. Same-day collapse, full range cross-day. Sorting key stays `scheduled_arrive`.

- [ ] **Step 1: Add the helper next to `fmtDate`**

In `static/dispatch/app.js` after `fmtDate` (~line 238), insert:

```js
function fmtArrivalWindow(start, end) {
  if (!start) return '—';
  if (!end) {
    try { return new Date(start).toLocaleString(); } catch { return start; }
  }
  try {
    const s = new Date(start);
    const e = new Date(end);
    const sameDay = s.toDateString() === e.toDateString();
    if (sameDay) {
      const sStr = s.toLocaleString();
      const eStr = e.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      return `${sStr}–${eStr}`;
    }
    return `${s.toLocaleString()} – ${e.toLocaleString()}`;
  } catch {
    return start;
  }
}
```

- [ ] **Step 2: Replace the six display sites**

In `static/dispatch/app.js`, change each of the following — but **only display sites**, not sort keys:

`:381-382` (loads list pickup/delivery columns):

```js
          <td>${fmtArrivalWindow(stops[0]?.scheduled_arrive, stops[0]?.scheduled_arrive_end)}</td>
          <td>${fmtArrivalWindow(stops[last]?.scheduled_arrive, stops[last]?.scheduled_arrive_end)}</td>
```

`:488` (load detail stops table — `Scheduled Arrive` column):

```js
          <td>${fmtArrivalWindow(stop.scheduled_arrive, stop.scheduled_arrive_end)}</td>
```

`:758-759` (trips list pickup/delivery columns):

```js
        const pickup = fmtArrivalWindow(
          trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive : null,
          trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive_end : null,
        );
        const delivery = fmtArrivalWindow(
          lastStop ? lastStop.scheduled_arrive : null,
          lastStop ? lastStop.scheduled_arrive_end : null,
        );
```

`:804` (trip detail stops table — `Scheduled Arrive`):

```js
        <td>${fmtArrivalWindow(stop.scheduled_arrive, stop.scheduled_arrive_end)}</td>
```

`:864` (driver-detail trips table — `Scheduled Arrive`):

```js
        <td>${fmtArrivalWindow(trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive : null, trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive_end : null)}</td>
```

Do **not** change `:358-359, 742-743` (sort keys on `scheduled_arrive` only — windows do not affect sort order; sorting on start is correct).

- [ ] **Step 3: Manual smoke**

Start the dev server, log in to dispatch UI, open the loads list and a load with a window stop. Verify the window renders.

```bash
cargo run --manifest-path /Users/jimp7508/src/ollie/Cargo.toml &
# in a second shell: open http://localhost:8080/dispatch and check
```

Stop the server (Ctrl-C).

- [ ] **Step 4: Commit**

```bash
git add static/dispatch/app.js
git commit -m "fix(dispatch-ui): render full arrival window (start + end) across loads/trips/details (#248)"
```

---

## Task 9: Render rate items + total on dispatch load detail

**Files:**
- Modify: `static/dispatch/app.js` (~line 606-648 — load detail page HTML)

#249. Add a Rate card between `Load Details` and `Stops`.

- [ ] **Step 1: Add a `fmtUSD` helper**

After `fmtBytes` in `static/dispatch/app.js`:

```js
function fmtUSD(n) {
  if (n === null || n === undefined) return '—';
  const sign = n < 0 ? '-' : '';
  const abs = Math.abs(n);
  return `${sign}$${abs.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}
```

- [ ] **Step 2: Build the rate card**

Before `const html = \`` block in `renderLoadDetailView` (~line 606), add:

```js
    let rateHtml = '';
    const rateItems = load.rate_items || [];
    if (rateItems.length > 0) {
      const rateRows = rateItems.map(r => {
        const negClass = r.amount_usd < 0 ? ' style="color: var(--color-danger, #b91c1c);"' : '';
        return `
          <tr>
            <td>${escHtml(r.description || '—')}</td>
            <td${negClass} style="text-align:right; font-variant-numeric: tabular-nums;${r.amount_usd < 0 ? ' color: var(--color-danger, #b91c1c);' : ''}">${fmtUSD(r.amount_usd)}</td>
          </tr>
        `;
      }).join('');
      rateHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Rate</div>
          <div class="table-wrapper">
            <table class="data-table">
              <thead><tr><th>Description</th><th style="text-align:right;">Amount</th></tr></thead>
              <tbody>${rateRows}</tbody>
              <tfoot>
                <tr><td style="font-weight:600;">Total</td><td style="text-align:right; font-weight:600; font-variant-numeric: tabular-nums;">${fmtUSD(load.total_rate_usd)}</td></tr>
              </tfoot>
            </table>
          </div>
        </div>
      `;
    }
```

In the `html` template, insert `${rateHtml}` between `${stopsHtml}` and `${tripsHtml}` (~line 645).

- [ ] **Step 3: Manual smoke**

Open a load with rate items in the dev server. Verify the table renders and negatives are red.

- [ ] **Step 4: Commit**

```bash
git add static/dispatch/app.js
git commit -m "feat(dispatch-ui): surface load rate line items + total on detail page (#249)"
```

---

## Task 10: Render mileage summary on dispatch load + trip detail

**Files:**
- Modify: `static/dispatch/app.js` — load detail (~line 606) and trip detail (~line 810)

- [ ] **Step 1: Add a `renderMileageCard(ms)` helper**

Near other render helpers in `static/dispatch/app.js`:

```js
function fmtMiles(n) {
  if (n === null || n === undefined) return '—';
  return `${n.toFixed(1)} mi`;
}

function renderMileageCard(ms) {
  if (!ms) return '';
  const originLabel = ms.origin && ms.origin.facility_name
    ? `${escHtml(ms.origin.facility_name)}${ms.origin.address ? ` — ${escHtml(ms.origin.address)}` : ''}`
    : null;
  const originRow = originLabel
    ? `<div class="detail-item"><div class="detail-item__label">Origin (prev. trip)</div><div class="detail-item__value">${originLabel}</div></div>`
    : '';
  const legsRows = (ms.legs || []).map(l =>
    `<tr><td>${escHtml(l.kind)}</td><td>${escHtml(l.from || '—')} → ${escHtml(l.to || '—')}</td><td style="text-align:right; font-variant-numeric: tabular-nums;">${fmtMiles(l.miles)}</td></tr>`
  ).join('');
  const legsTable = legsRows
    ? `<div class="table-wrapper" style="margin-top: var(--space-3);">
         <table class="data-table">
           <thead><tr><th>Leg</th><th>From → To</th><th style="text-align:right;">Miles</th></tr></thead>
           <tbody>${legsRows}</tbody>
         </table>
       </div>`
    : '';
  return `
    <div class="detail-card">
      <div class="detail-card__title">Mileage</div>
      <div class="detail-grid">
        ${originRow}
        <div class="detail-item"><div class="detail-item__label">Deadhead</div><div class="detail-item__value" style="font-variant-numeric: tabular-nums;">${fmtMiles(ms.deadhead_miles)}</div></div>
        <div class="detail-item"><div class="detail-item__label">Loaded</div><div class="detail-item__value" style="font-variant-numeric: tabular-nums;">${fmtMiles(ms.loaded_miles)}</div></div>
        <div class="detail-item"><div class="detail-item__label">Total</div><div class="detail-item__value" style="font-variant-numeric: tabular-nums; font-weight:600;">${fmtMiles(ms.total_miles)}</div></div>
      </div>
      ${legsTable}
    </div>
  `;
}
```

- [ ] **Step 2: Insert into load detail**

In `renderLoadDetailView`, insert `${renderMileageCard(load.mileage_summary)}` between `${rateHtml}` and `${stopsHtml}`.

- [ ] **Step 3: Insert into trip detail**

In `renderTripDetailView` (~line 810), insert `${renderMileageCard(trip.mileage_summary)}` between the existing Trip Details `detail-card` close and the Stops card.

- [ ] **Step 4: Manual smoke**

Open a chained trip (one with a `previous_trip_id`) in dispatch UI. Verify origin facility, deadhead, loaded, total, and per-leg table all render. Open a trip without a previous trip — origin row hidden, deadhead shows `—`.

- [ ] **Step 5: Commit**

```bash
git add static/dispatch/app.js
git commit -m "feat(dispatch-ui): render mileage summary (origin + legs + total) on trip + load detail (#246, #247)"
```

---

## Task 11: Render mileage summary on driver trip detail

**Files:**
- Modify: `static/driver/pages/trip-detail.js` (~line 124-145, around the stops timeline)

Wire the same data into the driver PWA. Origin appears as a leading row in the stop timeline; per-leg miles appear as small badges between stop nodes; total appears as a single row above the timeline (small font, muted color).

- [ ] **Step 1: Add a helper above `renderStopNode`**

In `static/driver/pages/trip-detail.js` (~line 373, just before `renderStopNode`):

```js
function fmtMiles(n) {
  if (n === null || n === undefined) return '—';
  return `${n.toFixed(1)} mi`;
}

function renderMileageHeader(ms) {
  if (!ms) return null;
  const section = document.createElement('div');
  section.className = 'trip-detail-section trip-detail-mileage';

  const totalRow = document.createElement('div');
  totalRow.className = 'trip-detail-row';
  totalRow.textContent = `Total: ${fmtMiles(ms.total_miles)} (${fmtMiles(ms.deadhead_miles)} deadhead + ${fmtMiles(ms.loaded_miles)} loaded)`;
  section.appendChild(totalRow);

  return section;
}

function renderOriginNode(origin) {
  if (!origin || !origin.facility_name) return null;
  const node = document.createElement('div');
  node.className = 'stop-node stop-node--origin';
  const marker = document.createElement('div');
  marker.className = 'stop-node__marker';
  node.appendChild(marker);
  const content = document.createElement('div');
  content.className = 'stop-node__content';
  const label = document.createElement('span');
  label.className = 'stop-node__type';
  label.textContent = 'Starting from';
  const name = document.createElement('span');
  name.className = 'stop-node__name';
  const place = origin.address ? `${origin.facility_name} — ${origin.address}` : origin.facility_name;
  name.textContent = place;
  const title = document.createElement('div');
  title.className = 'stop-node__title';
  title.appendChild(label);
  title.appendChild(document.createTextNode(' — '));
  title.appendChild(name);
  content.appendChild(title);
  node.appendChild(content);
  return node;
}

function renderLegBadge(miles) {
  if (miles === null || miles === undefined) return null;
  const el = document.createElement('div');
  el.className = 'stop-timeline__leg';
  el.textContent = `↓ ${fmtMiles(miles)}`;
  return el;
}
```

- [ ] **Step 2: Use them in the stops section**

Where the existing stop timeline is built (~line 127-136), replace:

```js
    if (data.stops && data.stops.length > 0) {
      const stopTimeline = document.createElement('div');
      stopTimeline.className = 'stop-timeline';

      data.stops.forEach(stop => {
        const stopNode = renderStopNode(stop, tripId);
        stopTimeline.appendChild(stopNode);
      });

      stopsSection.appendChild(stopTimeline);
    }
```

with:

```js
    if (data.stops && data.stops.length > 0) {
      const ms = data.mileage_summary || null;
      const header = renderMileageHeader(ms);
      if (header) stopsSection.appendChild(header);

      const stopTimeline = document.createElement('div');
      stopTimeline.className = 'stop-timeline';

      const originNode = renderOriginNode(ms ? ms.origin : null);
      if (originNode) {
        stopTimeline.appendChild(originNode);
        const dhBadge = renderLegBadge(ms && ms.deadhead_miles);
        if (dhBadge) stopTimeline.appendChild(dhBadge);
      }

      // legs are ordered to match waypoint pairs:
      //   with origin:    legs[0] = deadhead (origin→stop0), legs[1] = stop0→stop1, ...
      //   without origin: legs[0] = stop0→stop1, legs[1] = stop1→stop2, ...
      const loadedStart = ms && ms.origin ? 1 : 0;
      data.stops.forEach((stop, idx) => {
        const stopNode = renderStopNode(stop, tripId);
        stopTimeline.appendChild(stopNode);
        if (idx < data.stops.length - 1 && ms && Array.isArray(ms.legs)) {
          const leg = ms.legs[loadedStart + idx];
          const badge = renderLegBadge(leg ? leg.miles : null);
          if (badge) stopTimeline.appendChild(badge);
        }
      });

      stopsSection.appendChild(stopTimeline);
    }
```

- [ ] **Step 3: Add CSS tokens**

In `static/driver/css/base.css`, add (near existing `stop-node` rules — search for `stop-node__marker` to find the section):

```css
.stop-node--origin .stop-node__marker {
  background: var(--color-muted, #94a3b8);
}
.stop-timeline__leg {
  font-size: var(--text-xs, 0.75rem);
  color: var(--color-text-muted);
  padding: var(--space-1, 4px) 0 var(--space-1, 4px) calc(var(--stop-marker-size, 12px) + var(--space-3, 12px));
}
.trip-detail-mileage {
  font-size: var(--text-sm, 0.875rem);
  color: var(--color-text-muted);
}
```

Update `docs/DESIGN.md` only if you introduced a token that's net-new (none here — all referenced tokens exist or have safe `var(--name, fallback)` defaults).

- [ ] **Step 4: Manual smoke**

Start the dev server, log in to driver PWA, open a trip with a previous trip in chain. Verify origin row, deadhead badge, per-leg badges, and total header all render. Open a trip with no previous trip — origin row absent, total still shows.

- [ ] **Step 5: Commit**

```bash
git add static/driver/pages/trip-detail.js static/driver/css/base.css
git commit -m "feat(driver-ui): render origin + per-leg miles + total on trip detail (#246, #247)"
```

---

## Task 12: Update issue numbers in completion comments

After PR merges, each in-scope issue gets a verification comment. (Handled by the `/sprint-plan` skill at step 11, not part of the per-task plan.)

---

## Self-Review Check

**Spec coverage:**
- #246 (origin): Task 5 builds DeadheadOrigin server-side; Task 10 renders on dispatch; Task 11 renders on driver. ✅
- #247 (per-leg miles + total): Task 1 captures segments from ORS; Task 4 derives legs + total; Tasks 6/7 surface in responses; Tasks 10/11 render. ✅
- #248 (arrival window): Task 8 adds helper + replaces 6 sites. ✅
- #249 (rate display): Task 9 renders rate card. ✅
- #132 partial unlock: Task 3 persists `total_miles` to DB. ✅

**Placeholder scan:** No TBD/TODO/placeholders. All file paths absolute. All code blocks complete.

**Type consistency:** `MileageSummary`, `LegMiles`, `DeadheadOrigin` defined in Task 2, referenced by exact name in Tasks 5, 6, 7, 10, 11. `RouteMiles` defined in Task 1, used in Task 4. `compute_trip_mileage` returns `ComputedMileage` (internal struct), fields consumed in Task 4. `build_mileage_summary` is the public helper used by both portals.

**Opus review (2026-05-21):** 3 blockers + 1 significant addressed inline. `FacilityRecord` confirmed to expose only `address`/`normalized_address` — `DeadheadOrigin` now carries a single `address: Option<String>`. Task 7 rewritten to use `list_trips_for_load` (returns `Vec<TripRecord>` directly) and to sort by `created_at` desc since LanceDB scan order is not guaranteed. `segment_miles: Vec<f64>` now persisted in the trips schema (JSON string column matching existing `stops`/`trailer_ids` convention) so per-leg fidelity survives across reads — `build_mileage_summary` zips persisted segments with stop names rather than synthesizing aggregates.
