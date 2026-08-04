# Administrative (No-Trip) Loads Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a load that represents non-freight revenue — a weekly revenue guarantee, TONU, detention-only billing, layover pay, an accessorial-only freight bill — run `planned → invoiced → settled` without inventing a trip.

**Architecture:** A new `LoadKind` enum (`freight` | `administrative`) is persisted on the load. Kind-aware transition policy moves to `LoadRecord::can_transition_to`, which grants exactly one extra edge (`Planned → Invoiced`) and otherwise delegates to the untouched `LoadStatus` state machine. Administrative loads are then isolated from the trip and routing machinery so the two models cannot tangle.

**Tech Stack:** Rust (axum, LanceDB 0.29 / Arrow 58, tokio), vanilla-JS fleet SPA tested with Vitest + happy-dom.

**Spec:** [`docs/superpowers/specs/2026-08-04-administrative-loads-design.md`](../specs/2026-08-04-administrative-loads-design.md)

## Global Constraints

- **DCO is enforced.** Every commit needs `git commit -s`. CI blocks merge without a `Signed-off-by` trailer matching the author.
- **Co-author every commit** with `Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>`.
- **Commit prefixes:** `feat:`, `fix:`, `refactor:`, `test:`, `chore:`.
- **Never run `cargo fmt`.** This repo is hand-formatted and not rustfmt-compliant; there is no CI fmt check. Match the surrounding style — the codebase packs multiple short fields per line in struct literals and uses 4-space indent.
- **LanceDB SQL casts use the SQL keyword type `string`, never the Arrow name `Utf8`.** `CAST('freight' AS string)`. Using `Utf8` produces a DataFusion error and a crash loop on startup — this has regressed three times (v1.10.0, v1.13.0, v1.16.0).
- **Never pin `--manifest-path` to an absolute path.** Run `cargo test` from the checkout root you are working in. An absolute path silently tests the primary checkout while your worktree changes go unverified.
- **After every task:** `cargo test`, `cargo clippy --all-targets`, `cargo build`. For tasks touching `static/fleet/`, also `npm test`.
- **Do not bump any version or PWA cache stamp.** `Cargo.toml` version and the fleet `?v=` stamps are owned exclusively by the `cut-release` skill. Touching them here is a known past defect (PR #284).
- **Baseline commit:** `30b79ab` (PR #408, `fix(trips): load stranded in in_transit…`). All line numbers below are as of that commit and will drift as tasks land — the surrounding code is quoted so you can locate each site.

---

## File Structure

**Modified:**
- `src/models/load.rs` — `LoadKind` enum, `LoadRecord.kind`, `LoadRecord::can_transition_to`, request/response structs
- `src/db/mod.rs` — `load_schema` gains `kind`; new `open_or_create_load` with the migration
- `src/db/load_ops.rs` — batch write/read, `transition_load_status`, `update_load_kind`, `build_load_filter`, routing queries
- `src/events/mod.rs` — `on_load_status_changed`
- `src/api/fleet_portal/data.rs` — create/update handlers, `build_load_detail`, `subject_for`, `apply_load_kind_change`
- `src/api/fleet_portal/mcp.rs` — `create_load` / `update_load` / `list_loads` schemas and handlers
- `src/api/loads.rs` — `ListLoadsQuery.facility_id`
- `src/api/trips.rs` — administrative-load guard in `apply_trip_create`
- `src/pipeline/routing.rs` — test fixture only
- `static/fleet/pages/load-detail.js` — invoice gate, kind badge
- `static/fleet/pages/events.js` — `ROUTE_BASE.load`
- `static/fleet/css/components.css` — `.badge--load`
- `AGENTS.md` — invariant

**Created:**
- `tests/administrative_loads_test.rs` — end-to-end acceptance
- `tests/fleet/load-detail.test.js` — invoice gate unit tests

---

## Task 1: `LoadKind` field, persistence, and schema migration

Adds the field and makes it survive a round trip on both fresh and pre-existing databases. No behavior change yet — every load is `freight`.

The `loads` table is currently opened through the generic `open_or_create` (`src/db/mod.rs:68`), which has **no migration branch**. Adding a column to `load_schema` without also building that branch breaks every existing install on first read. That is the bulk of this task.

**Files:**
- Modify: `src/models/load.rs` (after `LoadStatus`'s `FromStr` impl, ~line 117; `LoadRecord` at 284; test fixtures at 495 and 515)
- Modify: `src/db/mod.rs` (`load_schema` 853, `empty_load_batch` 1562, call site 68)
- Modify: `src/db/load_ops.rs` (`load_to_batch` 290, `row_to_load` 345, `sample_load` 446)
- Modify: `src/api/fleet_portal/data.rs:317`, `src/api/fleet_portal/mcp.rs:2117`, `src/pipeline/routing.rs:69`
- Modify: `tests/load_delivery_cascade_test.rs:121`
- Test: `tests/migration_test.rs`, `src/db/load_ops.rs` tests module

**Interfaces:**
- Produces: `LoadKind` (enum, variants `Freight` | `Administrative`), `LoadKind::as_str() -> &'static str` (`"freight"` / `"administrative"`), `impl FromStr for LoadKind`, `impl Default for LoadKind` (→ `Freight`), field `LoadRecord.kind: LoadKind`, and `open_or_create_load(conn, embed_dim) -> Result<Table, AppError>`.

- [ ] **Step 1: Write the failing migration test**

Append to `tests/migration_test.rs`. This builds a `loads` table at the pre-`kind` schema, then reopens it with the current `DbClient::new` and asserts the migration added the column with the right default.

```rust
/// Pre-`kind` load schema: the current `load_schema` minus the trailing
/// `kind` column added for administrative loads.
fn load_schema_pre_kind(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("load_number", DataType::Utf8, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("customer_name", DataType::Utf8, false),
        Field::new("customer_ref", DataType::Utf8, true),
        Field::new("stops", DataType::Utf8, false),
        Field::new("rate_items", DataType::Utf8, false),
        Field::new("commodity", DataType::Utf8, true),
        Field::new("weight_lbs", DataType::Float64, true),
        Field::new("miles", DataType::Float64, true),
        Field::new("notes", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new("blob_ids", DataType::Utf8, false),
        Field::new("invoice_number", DataType::Utf8, true),
        Field::new("invoice_date", DataType::Utf8, true),
        Field::new("cancellation_reason", DataType::Utf8, true),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

fn load_pre_kind_row_batch(schema: Arc<Schema>, embed_dim: usize, id: &str) -> RecordBatch {
    let now = Utc::now().to_rfc3339();
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![None];
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![Some(id)])),
            Arc::new(StringArray::from(vec![Some("LD-2026-0001")])),
            Arc::new(Int64Array::from(vec![0_i64])),
            Arc::new(StringArray::from(vec![Some("planned")])),
            Arc::new(StringArray::from(vec![Some("ACME Logistics")])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(Float64Array::from(vec![None::<f64>])),
            Arc::new(Float64Array::from(vec![None::<f64>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(StringArray::from(vec![Some("[]")])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(FixedSizeListArray::from_iter_primitive::<
                arrow_array::types::Float32Type, _, _
            >(nulls, embed_dim as i32)),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
            Arc::new(StringArray::from(vec![Some(now.as_str())])),
        ],
    )
    .unwrap()
}

#[tokio::test]
async fn migration_opens_pre_kind_loads_table_and_adds_kind() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    let existing_id = Uuid::new_v4().to_string();

    // Build the old-shaped table directly, bypassing DbClient.
    {
        let conn = lancedb::connect(path).execute().await.unwrap();
        let schema = load_schema_pre_kind(EMBED_DIM);
        let batch = load_pre_kind_row_batch(schema.clone(), EMBED_DIM, &existing_id);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        conn.create_table("loads", reader).execute().await.unwrap();
    }

    // Opening through DbClient must migrate rather than fail.
    let db = DbClient::new(path, EMBED_DIM).await.unwrap();

    // The pre-existing row backfills to `freight`, not to a parse error.
    let migrated = db
        .get_load_by_id(existing_id.parse::<Uuid>().unwrap())
        .await
        .unwrap();
    assert_eq!(migrated.kind, ollie::models::LoadKind::Freight);
    assert_eq!(migrated.load_number, "LD-2026-0001");

    // And a fresh administrative load round-trips through the migrated table.
    let mut fresh = migrated.clone();
    fresh.id = Uuid::new_v4();
    fresh.load_number = "JQL-4581461".into();
    fresh.kind = ollie::models::LoadKind::Administrative;
    db.insert_load(&fresh).await.unwrap();
    let refetched = db.get_load_by_id(fresh.id).await.unwrap();
    assert_eq!(refetched.kind, ollie::models::LoadKind::Administrative);
}
```

- [ ] **Step 2: Run it to confirm it fails**

```bash
cargo test --test migration_test migration_opens_pre_kind_loads_table_and_adds_kind
```

Expected: compile error — `no variant or associated item named Freight found for enum` / `no field kind on type LoadRecord`. That is the correct red for a field that does not exist yet.

- [ ] **Step 3: Add the `LoadKind` enum**

In `src/models/load.rs`, immediately after the `impl std::str::FromStr for LoadStatus` block (ends ~line 117), matching the `StopType` / `ServiceType` idiom already in this file:

```rust
/// Whether a load represents freight that moves, or revenue with no truck
/// behind it.
///
/// `Administrative` covers weekly revenue guarantees, TONU, detention-only
/// billing, layover pay, and accessorial-only freight bills: real money on a
/// real freight bill, with no trip, no mileage, and no driver. Such a load
/// cannot reach `Delivered`, because every route there is a trip-side cascade,
/// so it invoices straight from `Planned` — see `LoadRecord::can_transition_to`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoadKind {
    #[default]
    Freight,
    Administrative,
}

impl LoadKind {
    pub fn as_str(&self) -> &'static str {
        match self { Self::Freight => "freight", Self::Administrative => "administrative" }
    }
}

impl std::str::FromStr for LoadKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "freight" => Ok(Self::Freight),
            "administrative" => Ok(Self::Administrative),
            other => Err(format!("unknown load kind: {other}")),
        }
    }
}
```

- [ ] **Step 4: Add the field to `LoadRecord`**

In `src/models/load.rs`, in `pub struct LoadRecord` (line 284), directly after `pub status: LoadStatus,`:

```rust
    #[serde(default)]
    pub kind: LoadKind,
```

- [ ] **Step 5: Add the column to the Arrow schema**

In `src/db/mod.rs`, `load_schema` (line 853) — append **at the end**, after `updated_at`:

```rust
        Field::new("kind", DataType::Utf8, false),
```

Append rather than insert mid-list, so a fresh table's physical layout matches a migrated one's (`add_columns` always appends). The `fleet_users` precedent shows LanceDB reconciles batch columns by name rather than position, so mid-list would probably work too — but matching layouts removes the question, and the Step 1 migration test is what actually proves it.

In `empty_load_batch` (line 1562), append one more empty column as the **last** entry in the `vec![...]`, after the two trailing `created_at` / `updated_at` arrays:

```rust
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
```

- [ ] **Step 6: Write and read the column**

In `src/db/load_ops.rs`, `load_to_batch` (line 290) — append as the **last** entry of the `vec![...]` passed to `RecordBatch::try_new`:

```rust
        Arc::new(StringArray::from(vec![record.kind.as_str()])),
```

In `row_to_load` (line 345), inside the `Ok(LoadRecord {` literal, directly after the `status:` line:

```rust
        kind: {
            let k = str_col("kind");
            if k.is_empty() {
                crate::models::LoadKind::Freight
            } else {
                k.parse().map_err(AppError::Internal)?
            }
        },
```

The empty-string branch is the one real case: a row written before the migration returns `""` from `str_col`, and a read must not fail on it. Anything else propagates, matching how `status` behaves two lines above — a blanket `unwrap_or` would silently coerce genuine corruption to `Freight`.

- [ ] **Step 7: Add the migration branch**

In `src/db/mod.rs`, add this function next to `open_or_create_fleet_user` (~line 362):

```rust
async fn open_or_create_load(conn: &lancedb::Connection, embed_dim: usize) -> Result<Table, AppError> {
    let schema = load_schema(embed_dim);
    match conn.open_table("loads").execute().await {
        Err(_) => {
            let batch = empty_load_batch(schema.clone(), embed_dim)?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("loads", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        Ok(table) => {
            let existing = table.schema().await.map_err(|e| AppError::Internal(e.to_string()))?;
            let mut transforms: Vec<(String, String)> = Vec::new();
            // SQL keyword type `string`, never the Arrow name `Utf8` — see AGENTS.md.
            if existing.field_with_name("kind").is_err() {
                transforms.push(("kind".into(), "CAST('freight' AS string)".into()));
            }
            if !transforms.is_empty() {
                tracing::info!("migrating loads table: adding {} column(s)", transforms.len());
                table.add_columns(NewColumnTransform::SqlExpressions(transforms), None).await
                    .map_err(|e| AppError::Internal(format!("load schema migration failed: {e}")))?;
            }
            Ok(table)
        }
    }
}
```

Replace the call site at `src/db/mod.rs:68` — currently:

```rust
        let load_table = open_or_create(&conn, "loads", load_schema(embed_dim), |schema| {
            empty_load_batch(schema, embed_dim)
        }).await?;
```

with:

```rust
        let load_table = open_or_create_load(&conn, embed_dim).await?;
```

- [ ] **Step 8: Fix the struct-literal construction sites**

Adding a field breaks every `LoadRecord { .. }` literal. Add `kind: LoadKind::Freight,` (adjusting the path per file's imports) directly after the `status:` field at each:

- `src/models/load.rs:495` (`test_total_rate_usd_sums_including_negatives`) — `kind: LoadKind::Freight,`
- `src/models/load.rs:515` (`test_load_record_embedding_skipped_in_json`) — `kind: LoadKind::Freight,`
- `src/db/load_ops.rs:448` (`sample_load`) — `kind: crate::models::LoadKind::Freight,`
- `src/pipeline/routing.rs:69` (test fixture) — `kind: crate::models::LoadKind::Freight,`
- `tests/load_delivery_cascade_test.rs:121` (`load_with_stops`) — `kind: ollie::models::LoadKind::Freight,` and add `LoadKind` to the existing `use ollie::models::{...}` list
- `src/api/fleet_portal/data.rs:317` (`create_load_handler`) — `kind: LoadKind::Freight,`
- `src/api/fleet_portal/mcp.rs:2117` (`tool_create_load`) — `kind: crate::models::LoadKind::Freight,`

The last two are hardcoded **on purpose** for now — `CreateLoadRequest` has no `kind` field until Task 5, which replaces both with `body.kind.unwrap_or_default()`. Do not add the request field early; Task 5 owns that boundary.

Add `LoadKind` to the `use crate::models::{...}` list in `src/api/fleet_portal/data.rs` if it is not pulled in by a glob.

- [ ] **Step 9: Add a DB round-trip test**

In `src/db/load_ops.rs`, in the `mod tests` block (line 435):

```rust
    #[tokio::test]
    async fn test_load_kind_round_trips() {
        let (db, _dir) = test_db().await;
        let mut load = sample_load();
        load.kind = crate::models::LoadKind::Administrative;
        db.insert_load(&load).await.unwrap();
        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert_eq!(fetched.kind, crate::models::LoadKind::Administrative);
    }

    #[tokio::test]
    async fn test_load_kind_defaults_to_freight() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert_eq!(fetched.kind, crate::models::LoadKind::Freight);
    }
```

- [ ] **Step 10: Run the tests**

```bash
cargo test --test migration_test migration_opens_pre_kind_loads_table_and_adds_kind
```

Expected: PASS.

```bash
cargo test load_kind
```

Expected: PASS — `test_load_kind_round_trips`, `test_load_kind_defaults_to_freight`.

- [ ] **Step 11: Full verification**

```bash
cargo test && cargo clippy --all-targets && cargo build
```

Expected: all green, no clippy warnings. If a `LoadRecord { .. }` literal was missed, the compile error names the file and line — fix and rerun.

- [ ] **Step 12: Commit**

```bash
git add -A && git commit -s -m "feat(loads): add LoadKind field with LanceDB migration

The loads table was opened through the generic open_or_create, which has no
migration branch, so any new column would break existing installs on read.
Adds a dedicated open_or_create_load mirroring open_or_create_fleet_user.

Every load is still freight; no behavior change yet.

Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>"
```

---

## Task 2: Kind-aware transition policy

Grants administrative loads exactly one extra edge. `LoadStatus::can_transition_to` is left untouched so its ~20 existing assertions keep passing.

**Files:**
- Modify: `src/models/load.rs` (`impl LoadRecord`, line 309)
- Modify: `src/db/load_ops.rs` (`transition_load_status`, line 94)
- Test: `src/models/load.rs` tests module, `src/db/load_ops.rs` tests module

**Interfaces:**
- Consumes: `LoadKind`, `LoadRecord.kind` (Task 1)
- Produces: `LoadRecord::can_transition_to(&self, next: &LoadStatus) -> bool`

- [ ] **Step 1: Write the failing tests**

In `src/models/load.rs`, in the `mod tests` block, add a helper and the matrix. Place the helper next to the existing test fixtures:

```rust
    fn load_of_kind(kind: LoadKind, status: LoadStatus) -> LoadRecord {
        LoadRecord {
            id: uuid::Uuid::new_v4(), load_number: "JQL-4581461".into(),
            owner_id: 0, status, kind,
            customer_name: "Landstar".into(), customer_ref: None,
            stops: vec![], rate_items: vec![], commodity: None,
            weight_lbs: None, miles: None, notes: None, tags: vec![],
            blob_ids: vec![], invoice_number: None, invoice_date: None,
            cancellation_reason: None, embedding: None,
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_administrative_load_invoices_from_planned() {
        let load = load_of_kind(LoadKind::Administrative, LoadStatus::Planned);
        assert!(load.can_transition_to(&LoadStatus::Invoiced));
    }

    #[test]
    fn test_freight_load_cannot_invoice_from_planned() {
        let load = load_of_kind(LoadKind::Freight, LoadStatus::Planned);
        assert!(!load.can_transition_to(&LoadStatus::Invoiced));
    }

    #[test]
    fn test_administrative_load_is_never_delivered() {
        // A load that never moved was never delivered. `invoiced` is the honest
        // first stop, so the trip-shaped edge stays closed.
        let load = load_of_kind(LoadKind::Administrative, LoadStatus::Planned);
        assert!(!load.can_transition_to(&LoadStatus::Delivered));
    }

    #[test]
    fn test_administrative_load_settles_from_invoiced() {
        let load = load_of_kind(LoadKind::Administrative, LoadStatus::Invoiced);
        assert!(load.can_transition_to(&LoadStatus::Settled));
    }

    #[test]
    fn test_administrative_load_can_still_be_cancelled() {
        let load = load_of_kind(LoadKind::Administrative, LoadStatus::Planned);
        assert!(load.can_transition_to(&LoadStatus::Cancelled));
    }

    #[test]
    fn test_administrative_load_cannot_skip_back_from_settled() {
        let load = load_of_kind(LoadKind::Administrative, LoadStatus::Settled);
        assert!(!load.can_transition_to(&LoadStatus::Invoiced));
        assert!(!load.can_transition_to(&LoadStatus::Planned));
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test --lib models::load::tests::test_administrative
```

Expected: compile error — `no method named can_transition_to found for struct LoadRecord`.

- [ ] **Step 3: Implement the policy**

In `src/models/load.rs`, extend the existing `impl LoadRecord` block (line 309, currently holding only `total_rate_usd`):

```rust
    /// Kind-aware transition policy.
    ///
    /// `LoadStatus::can_transition_to` stays a pure status machine; the one edge
    /// that depends on the *load* rather than on its status alone lives here.
    ///
    /// An administrative load has no trip and never will (see the guard in
    /// `apply_trip_create`), so the trip-driven route to `Delivered` is
    /// unreachable and the load would sit in `planned` forever. It invoices
    /// straight from `planned` instead. It deliberately does **not** get
    /// `Planned -> Delivered`: a load that never moved was never delivered, and
    /// `Delivered` has no reverse edge to walk back from.
    pub fn can_transition_to(&self, next: &LoadStatus) -> bool {
        if self.kind == LoadKind::Administrative
            && matches!((&self.status, next), (LoadStatus::Planned, LoadStatus::Invoiced))
        {
            return true;
        }
        self.status.can_transition_to(next)
    }
```

- [ ] **Step 4: Wire it into the DB transition**

In `src/db/load_ops.rs`, `transition_load_status` (line 94). Change:

```rust
        if !record.status.can_transition_to(&new_status) {
```

to:

```rust
        if !record.can_transition_to(&new_status) {
```

- [ ] **Step 5: Add the DB-level test**

In `src/db/load_ops.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn test_administrative_load_walks_planned_to_settled() {
        let (db, _dir) = test_db().await;
        let mut load = sample_load();
        load.kind = crate::models::LoadKind::Administrative;
        db.insert_load(&load).await.unwrap();

        db.transition_load_status(
            load.id, LoadStatus::Invoiced,
            Some("JQL-4581461".into()), Some("2026-07-29".into()), None,
        ).await.unwrap();
        db.transition_load_status(load.id, LoadStatus::Settled, None, None, None).await.unwrap();

        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert_eq!(fetched.status, LoadStatus::Settled);
        assert_eq!(fetched.invoice_number.as_deref(), Some("JQL-4581461"));
    }

    #[tokio::test]
    async fn test_freight_load_still_cannot_invoice_from_planned() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        let err = db.transition_load_status(load.id, LoadStatus::Invoiced, None, None, None).await;
        assert!(matches!(err, Err(AppError::Conflict(_))));
    }
```

- [ ] **Step 6: Run the tests**

```bash
cargo test administrative && cargo test --lib models::load::tests
```

Expected: PASS, including the pre-existing `LoadStatus` assertions.

- [ ] **Step 7: Full verification**

```bash
cargo test && cargo clippy --all-targets
```

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -s -m "feat(loads): let administrative loads invoice from planned

LoadRecord::can_transition_to adds the one edge that depends on the load
rather than its status alone. LoadStatus stays a pure status machine.

Planned -> Delivered stays closed: a load that never moved was never
delivered, and Delivered has no reverse edge.

Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>"
```

---

## Task 3: Load lifecycle events

There are currently no load status events at all — `src/events/mod.rs` covers trips, drivers, and equipment only. Emission goes inside `transition_load_status` so no code path can skip it.

**Files:**
- Modify: `src/events/mod.rs`
- Modify: `src/db/load_ops.rs` (`transition_load_status`)
- Test: `src/events/mod.rs` tests module

**Interfaces:**
- Consumes: `LoadRecord::can_transition_to` (Task 2), `DbClient::append_event(entity_type, entity_id, event_type, payload, actor, occurred_at, ai)`
- Produces: `events::on_load_status_changed(db: &DbClient, load_id: Uuid, from: &str, to: &str)`

- [ ] **Step 1: Write the failing test**

In `src/events/mod.rs`, in the `mod tests` block (the `test_db` helper is at line 158):

```rust
    #[tokio::test]
    async fn test_load_status_changed_records_both_ends() {
        let (db, _dir) = test_db().await;
        let load_id = Uuid::new_v4();

        on_load_status_changed(&db, load_id, "planned", "invoiced").await;

        let (_total, events) = db.query_events(
            Some(load_id), None, Some("load.invoiced"), None, None, 10, 0,
        ).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].entity_type, "load");
        let payload: serde_json::Value = serde_json::from_str(
            events[0].payload.as_deref().unwrap_or("{}"),
        ).unwrap();
        assert_eq!(payload["from"], serde_json::json!("planned"));
        assert_eq!(payload["to"], serde_json::json!("invoiced"));
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --lib events::tests::test_load_status_changed_records_both_ends
```

Expected: compile error — `cannot find function on_load_status_changed in this scope`.

- [ ] **Step 3: Implement the emitter**

In `src/events/mod.rs`, alongside the existing `on_trip_*` functions:

```rust
/// A load's status moved. Emitted from inside `transition_load_status` rather
/// than from its callers, so no path — API, MCP, trip cascade, or doctor fix —
/// can move a load without leaving a record.
///
/// `actor` is deliberately `None` for now: `transition_load_status` has no actor
/// parameter, and the human-vs-cascade distinction belongs to the `set_load_status`
/// work that will thread one through.
pub async fn on_load_status_changed(db: &DbClient, load_id: Uuid, from: &str, to: &str) {
    let payload = serde_json::json!({ "from": from, "to": to });
    let _ = db.append_event(
        "load", load_id, &format!("load.{to}"), Some(payload), None, &now_z(), None,
    ).await;
    tracing::info!(load_id = %load_id, from, to, "load status changed");
}
```

- [ ] **Step 4: Call it from the transition**

In `src/db/load_ops.rs`, `transition_load_status`. Capture the old status **before** it is overwritten, and emit after the upsert succeeds:

```rust
        let from = record.status.as_str().to_string();
        record.status = new_status;
        if let Some(v) = invoice_number { record.invoice_number = Some(v); }
        if let Some(v) = invoice_date { record.invoice_date = Some(v); }
        if let Some(v) = cancellation_reason { record.cancellation_reason = Some(v); }
        record.updated_at = Utc::now();
        self.upsert_load(&record).await?;
        crate::events::on_load_status_changed(self, id, &from, record.status.as_str()).await;
        Ok(record)
```

The `let from = ...` line goes immediately after the `can_transition_to` guard block. The `crate::events::` call goes between the existing `upsert_load` and `Ok(record)` lines.

- [ ] **Step 5: Add the end-to-end event test**

In `src/db/load_ops.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn test_transition_emits_load_event() {
        let (db, _dir) = test_db().await;
        let mut load = sample_load();
        load.kind = crate::models::LoadKind::Administrative;
        db.insert_load(&load).await.unwrap();

        db.transition_load_status(load.id, LoadStatus::Invoiced, None, None, None).await.unwrap();

        let (_total, events) = db.query_events(
            Some(load.id), None, Some("load.invoiced"), None, None, 10, 0,
        ).await.unwrap();
        assert_eq!(events.len(), 1);
    }
```

- [ ] **Step 6: Run the tests**

```bash
cargo test --lib events::tests::test_load_status_changed_records_both_ends
cargo test test_transition_emits_load_event
```

Expected: both PASS.

- [ ] **Step 7: Full verification**

```bash
cargo test && cargo clippy --all-targets
```

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -s -m "feat(events): emit an event on every load status change

Loads were the only lifecycle entity with no events at all. Emission lives
inside transition_load_status so no path — API, MCP, trip cascade, or the
load_doctor fix — can move a load silently.

Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>"
```

---

## Task 4: Surface load events in the fleet feed

Load events now exist but render without a subject label or a jump link. Three small additions.

**Files:**
- Modify: `src/api/fleet_portal/data.rs` (`subject_for`, line 1501)
- Modify: `static/fleet/pages/events.js` (`ROUTE_BASE`, line 5)
- Modify: `static/fleet/css/components.css` (badge rules, ~line 561)
- Test: `tests/fleet/events.test.js`, `src/api/fleet_portal/data.rs` tests module

**Interfaces:**
- Consumes: `events::on_load_status_changed` (Task 3)

- [ ] **Step 1: Write the failing JS test**

In `tests/fleet/events.test.js`, add to the existing `describe('jumpHref', ...)` block:

```js
  it('maps load events to the load detail route', () => {
    expect(jumpHref('load', 'l1')).toBe('/fleet/loads/l1');
  });
```

- [ ] **Step 2: Run to verify it fails**

```bash
npm test -- events
```

Expected: FAIL — `expected null to be '/fleet/loads/l1'`.

- [ ] **Step 3: Add the route mapping**

In `static/fleet/pages/events.js`, line 5:

```js
const ROUTE_BASE = {
  trip: 'trips', driver: 'drivers', truck: 'trucks', trailer: 'trailers', blob: 'documents',
  load: 'loads',
};
```

- [ ] **Step 4: Add the badge style**

In `static/fleet/css/components.css`, next to the existing `.badge--trip` / `.badge--driver` / `.badge--blob` rules (~line 561). Use existing tokens only — never inline a hex value:

```css
.badge--load    { background: var(--color-warning-soft);  color: var(--color-warning); }
```

Both tokens are confirmed present in `static/fleet/css/base.css:10-11` and `static/driver/css/base.css:10-11`. Use them as written; do not add a new token (that would require a `docs/DESIGN.md` entry and is out of scope).

- [ ] **Step 5: Write the failing Rust test**

In `src/api/fleet_portal/data.rs`, in the `mod tests` block, directly after the existing `test_subject_for_driver_found` (~line 1769). That module already has a `test_db() -> (DbClient, TempDir)` helper at line 1649, and `subject_for` takes `&DbClient` — there is no `test_state` here.

There is no load fixture in this module, so add one next to the existing `sample_driver` helper, following the `sample_load` shape in `src/db/load_ops.rs:446`:

```rust
    fn sample_load_record(load_number: &str) -> crate::models::LoadRecord {
        let now = chrono::Utc::now();
        crate::models::LoadRecord {
            id: Uuid::new_v4(),
            load_number: load_number.into(),
            owner_id: 0,
            status: crate::models::LoadStatus::Planned,
            kind: crate::models::LoadKind::Freight,
            customer_name: "Landstar".into(), customer_ref: None,
            stops: vec![], rate_items: vec![],
            commodity: None, weight_lbs: None, miles: None, notes: None,
            tags: vec![], blob_ids: vec![],
            invoice_number: None, invoice_date: None, cancellation_reason: None,
            embedding: None, created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_subject_for_load_uses_load_number() {
        let (db, _dir) = test_db().await;
        let load = sample_load_record("JQL-4581461");
        db.insert_load(&load).await.unwrap();
        let result = subject_for(&db, "load", load.id).await;
        assert_eq!(result.as_deref(), Some("Load JQL-4581461"));
    }
```

- [ ] **Step 6: Run to verify it fails**

```bash
cargo test test_subject_for_load_uses_load_number
```

Expected: FAIL — `assertion failed: left: None, right: Some("Load JQL-4581461")`, because `subject_for` returns `None` for unknown entity types.

- [ ] **Step 7: Add the subject arm**

In `src/api/fleet_portal/data.rs`, `subject_for` (line 1501), add before the `_ => None` arm:

```rust
        "load" => db.get_load_by_id(id).await.ok().map(|l| format!("Load {}", l.load_number)),
```

- [ ] **Step 8: Run the tests**

```bash
cargo test test_subject_for_load_uses_load_number && npm test -- events
```

Expected: both PASS.

- [ ] **Step 9: Full verification**

```bash
cargo test && cargo clippy --all-targets && npm test
```

- [ ] **Step 10: Commit**

```bash
git add -A && git commit -s -m "feat(fleet): render load events with a subject and jump link

Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>"
```

---

## Task 5: `kind` on the create/update API surface

Makes `kind` settable and visible. This is what lets the three existing guarantee loads be converted in place rather than recreated.

**Files:**
- Modify: `src/models/load.rs` (`CreateLoadRequest` 316, `UpdateLoadRequest` 334, `LoadListItem` 360 + its `From` impl 383, `LoadDetailResponse` 407)
- Modify: `src/db/load_ops.rs` (new `update_load_kind`)
- Modify: `src/api/fleet_portal/data.rs` (create handler 317, update handler ~417, `build_load_detail` ~1881)
- Modify: `src/api/fleet_portal/mcp.rs` (`create_load` schema 820, `update_load` schema ~1539, `tool_create_load` 2117, `tool_update_load` ~2207)
- Test: `src/db/load_ops.rs` tests module

**Interfaces:**
- Consumes: `LoadKind` (Task 1)
- Produces: `DbClient::update_load_kind(&self, id: Uuid, kind: LoadKind) -> Result<LoadRecord, AppError>`; `CreateLoadRequest.kind: Option<LoadKind>`; `UpdateLoadRequest.kind: Option<LoadKind>`; `LoadListItem.kind`; `LoadDetailResponse.kind`

- [ ] **Step 1: Write the failing test**

In `src/db/load_ops.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn test_update_load_kind() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        let updated = db.update_load_kind(load.id, crate::models::LoadKind::Administrative)
            .await.unwrap();
        assert_eq!(updated.kind, crate::models::LoadKind::Administrative);
        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert_eq!(fetched.kind, crate::models::LoadKind::Administrative);
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test test_update_load_kind
```

Expected: compile error — `no method named update_load_kind found`.

- [ ] **Step 3: Add the DB method**

In `src/db/load_ops.rs`, directly after `update_load_number` (line ~117). A dedicated method rather than a parameter on `update_load_metadata`, which has seven call sites — this mirrors the `update_load_number` precedent:

```rust
    pub async fn update_load_kind(&self, id: Uuid, kind: crate::models::LoadKind) -> Result<LoadRecord, AppError> {
        let mut record = self.get_load_by_id(id).await?;
        record.kind = kind;
        record.updated_at = Utc::now();
        self.upsert_load(&record).await?;
        Ok(record)
    }
```

- [ ] **Step 4: Add the request fields**

In `src/models/load.rs`, `CreateLoadRequest` (line 316) — add after `pub customer_ref`, and relax `stops` in the same edit so administrative loads need not pass a meaningless `stops: []`:

```rust
    #[serde(default)]
    pub kind: Option<LoadKind>,
```

and change:

```rust
    pub stops: Vec<StopInput>,
```

to:

```rust
    #[serde(default)]
    pub stops: Vec<StopInput>,
```

No compensating "freight loads must have at least one stop" validation is added. None exists today, so adding one could reject flows that currently succeed.

In `UpdateLoadRequest` (line 334), add:

```rust
    pub kind: Option<LoadKind>,
```

- [ ] **Step 5: Add the response fields**

In `LoadListItem` (line 360), after `pub status: LoadStatus,`:

```rust
    pub kind: LoadKind,
```

In its `From<LoadRecord>` impl (line 383), in the `Self { ... }` literal after `status: r.status,`:

```rust
            kind: r.kind,
```

In `LoadDetailResponse` (line 407), after `pub status: LoadStatus,`:

```rust
    pub kind: LoadKind,
```

In `src/api/fleet_portal/data.rs`, `build_load_detail`'s `Ok(LoadDetailResponse { ... })` literal (~line 1881), after `status: record.status,`:

```rust
        kind: record.kind,
```

- [ ] **Step 6: Wire the create handlers**

In `src/api/fleet_portal/data.rs:317` and `src/api/fleet_portal/mcp.rs:2117`, replace the `kind: LoadKind::Freight,` hardcode from Task 1 Step 8 with:

```rust
        kind: body.kind.unwrap_or_default(),
```

In `mcp.rs` the local is named `req`, not `body` — use `req.kind.unwrap_or_default()` there. Check the surrounding lines to confirm the binding name before editing.

- [ ] **Step 7: Wire the update handlers**

`kind` changes go through `update_load_kind`, applied the same way `load_number` already is. In `src/api/fleet_portal/data.rs`, after the existing block:

```rust
    if let Some(ln) = body.load_number {
        updated = state.db.update_load_number(id, ln).await?;
    }
```

add:

```rust
    if let Some(k) = body.kind {
        updated = state.db.update_load_kind(id, k).await?;
    }
```

Apply the equivalent addition in `tool_update_load` in `src/api/fleet_portal/mcp.rs` (~line 2207), matching that function's local binding names.

Task 6 adds the guard that restricts *when* this change is legal. Leave it unguarded here.

- [ ] **Step 8: Update the MCP tool schemas**

In `src/api/fleet_portal/mcp.rs`, `create_load` (line ~820). Add to `properties`:

```json
                        "kind": { "type": "string", "enum": ["freight", "administrative"], "description": "Default freight. 'administrative' marks revenue with no trip behind it (weekly guarantee, TONU, detention-only, layover, accessorial-only) — it invoices straight from planned and cannot be assigned to a trip." },
```

and change its `"required"` from `["customer_name", "stops"]` to:

```json
                    "required": ["customer_name"]
```

In `update_load` (line ~1539), add to `properties`:

```json
                        "kind": { "type": "string", "enum": ["freight", "administrative"], "description": "Only changeable while the load is 'planned' and has no trips." },
```

- [ ] **Step 9: Run the tests**

```bash
cargo test test_update_load_kind
```

Expected: PASS.

- [ ] **Step 10: Full verification**

```bash
cargo test && cargo clippy --all-targets && cargo build
```

- [ ] **Step 11: Commit**

```bash
git add -A && git commit -s -m "feat(api): set and surface load kind on create, update, and read

update_load_kind is a dedicated method rather than another parameter on
update_load_metadata, which has seven call sites — same shape as the
existing update_load_number.

stops is no longer required on create, so an administrative load need not
pass a meaningless empty array.

Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>"
```

---

## Task 6: Isolation guards and routing exclusion

Keeps the two models from tangling, and stops administrative loads being re-queued for ORS routing on every startup — an existing zombie that `list_loads_needing_routing` creates for any load with `miles IS NULL` in a non-terminal status.

**Files:**
- Modify: `src/api/trips.rs` (`apply_trip_create`, load fetch at line 45)
- Modify: `src/api/fleet_portal/data.rs` (new `apply_load_kind_change`, create/update routing enqueues at 342 and 409)
- Modify: `src/api/fleet_portal/mcp.rs` (routing enqueues at 2141 and 2223, `tool_update_load` kind change)
- Modify: `src/db/load_ops.rs` (`list_loads_needing_routing` 259, `list_unrouted_loads_for_facility` 269)
- Test: `src/db/load_ops.rs` tests module, `tests/administrative_loads_test.rs` (Task 9 extends it)

**Interfaces:**
- Consumes: `DbClient::update_load_kind` (Task 5), `LoadRecord.kind` (Task 1)
- Produces: `data::apply_load_kind_change(state: &AppState, id: Uuid, kind: LoadKind) -> Result<LoadRecord, AppError>`

- [ ] **Step 1: Write the failing routing test**

In `src/db/load_ops.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn test_administrative_loads_are_not_queued_for_routing() {
        let (db, _dir) = test_db().await;

        let freight = sample_load();          // miles: None, status: Planned
        db.insert_load(&freight).await.unwrap();
        let mut admin = sample_load();
        admin.id = uuid::Uuid::new_v4();
        admin.load_number = "JQL-4581461".into();
        admin.kind = crate::models::LoadKind::Administrative;
        db.insert_load(&admin).await.unwrap();

        let queued = db.list_loads_needing_routing().await.unwrap();
        assert!(queued.contains(&freight.id));
        assert!(!queued.contains(&admin.id), "administrative loads have no stops to route");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test test_administrative_loads_are_not_queued_for_routing
```

Expected: FAIL on the second assertion — the administrative load is currently queued.

- [ ] **Step 3: Exclude administrative loads from routing queries**

In `src/db/load_ops.rs`, `list_loads_needing_routing` (line 259) — change the filter to:

```rust
            .only_if("miles IS NULL AND kind != 'administrative' AND status NOT IN ('delivered','invoiced','settled','cancelled')")
```

In `list_unrouted_loads_for_facility` (line 269) — change the `format!` filter to:

```rust
        let filter = format!(
            "miles IS NULL AND kind != 'administrative' AND status NOT IN ('delivered','invoiced','settled','cancelled') AND stops LIKE '%\"{}\"%'",
            fac_str
        );
```

- [ ] **Step 4: Skip the routing enqueue on write paths**

Four sites send a load id to `routing_tx`. Guard each on kind.

`src/api/fleet_portal/data.rs:342` (create) — change:

```rust
    if record.miles.is_none() {
        let _ = state.routing_tx.try_send(record.id);
    }
```

to:

```rust
    if record.miles.is_none() && record.kind != LoadKind::Administrative {
        let _ = state.routing_tx.try_send(record.id);
    }
```

`src/api/fleet_portal/data.rs:409` (update) — change:

```rust
    if stops_provided && body.miles.is_none() {
```

to:

```rust
    if stops_provided && body.miles.is_none() && updated.kind != LoadKind::Administrative {
```

Apply the two equivalent guards in `src/api/fleet_portal/mcp.rs` at lines 2141 (create, local `record`) and 2223 (update, local `updated`), matching each function's binding names.

- [ ] **Step 5: Write the failing trip-guard test**

Create `tests/administrative_loads_test.rs`. Drive it through the real fleet HTTP API, not through internal functions: copy the `setup()` and `setup_owner()` helpers from `tests/fleet_pagination_test.rs:16-58` (an `axum_test::TestServer` over `api::router(state)`, an unreachable Ollama, and a bearer token from `POST /fleet/setup`). Keep the `rx` receiver binding alive — dropping it closes the pipeline channel and later writes start failing.

Do **not** widen `apply_trip_create`'s `pub(crate)` visibility to reach it from a test. Testing through the route is both closer to the reported defect and free of that smell.

Relevant routes, from `src/api/fleet_portal/mod.rs:62-70`:

```
POST   /fleet/api/v1/trips              (create, body carries load_id)
PUT    /fleet/api/v1/loads/{id}         (update, body carries kind)
POST   /fleet/api/v1/loads/{id}/invoice
POST   /fleet/api/v1/loads/{id}/settle
GET    /fleet/api/v1/loads              (list, query filters)
```

```rust
// tests/administrative_loads_test.rs
//
// Administrative (no-trip) loads: revenue with no truck behind it — a weekly
// revenue guarantee, TONU, detention-only billing, layover, an accessorial-only
// freight bill. Such a load has no path to `delivered`, because every route
// there is a trip-side cascade, so it invoices straight from `planned`.
//
// These tests guard the isolation that makes that safe: an administrative load
// must never acquire a trip, and its kind must not be changeable once it has
// moved past `planned`.

// ... the imports and setup()/setup_owner() helpers copied from
// tests/fleet_pagination_test.rs:16-58

/// Create a stopless load through the API and return its id.
async fn create_bare_load(
    server: &TestServer, token: &str, load_number: &str, kind: &str,
) -> String {
    let resp = server.post("/fleet/api/v1/loads")
        .authorization_bearer(token)
        .json(&serde_json::json!({
            "load_number": load_number,
            "customer_name": "Landstar",
            "stops": [],
            "kind": kind,
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "create failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_trip_cannot_be_created_against_an_administrative_load() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let load_id = create_bare_load(&server, &token, "4581461", "administrative").await;

    let resp = server.post("/fleet/api/v1/trips")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "load_id": load_id, "stops": [] }))
        .await;

    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert!(
        resp.text().contains("administrative"),
        "the error should name the kind so the caller knows why: {}",
        resp.text(),
    );
}
```

Match `setup_owner`'s bearer-token helper name and the request-builder methods to whatever `tests/fleet_pagination_test.rs` actually uses — read it rather than assuming `authorization_bearer` if that file does it differently.

- [ ] **Step 6: Run to verify it fails**

```bash
cargo test --test administrative_loads_test test_trip_cannot_be_created
```

Expected: FAIL — the trip is created successfully, so the `matches!` assertion fails.

- [ ] **Step 7: Add the trip guard**

In `src/api/trips.rs`, `apply_trip_create` (line 45), replace:

```rust
    let load = if let Some(load_id) = body.load_id {
        Some(state.db.get_load_by_id(load_id).await?)
    } else {
        None
    };
```

with:

```rust
    let load = if let Some(load_id) = body.load_id {
        let load = state.db.get_load_by_id(load_id).await?;
        // An administrative load represents revenue with no truck behind it.
        // Attaching a trip would put false mileage, false stop actuals and a
        // false driver assignment into the operational record — the exact thing
        // the kind exists to avoid.
        if load.kind == crate::models::LoadKind::Administrative {
            return Err(AppError::UnprocessableEntity(format!(
                "load {load_id} is administrative (no-trip) and cannot have a trip. \
                 Change its kind to 'freight' first, or invoice it directly."
            )));
        }
        Some(load)
    } else {
        None
    };
```

- [ ] **Step 8: Write the failing kind-change-guard test**

In `tests/administrative_loads_test.rs`:

```rust
#[tokio::test]
async fn test_kind_cannot_change_once_the_load_has_left_planned() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let load_id = create_bare_load(&server, &token, "4581461", "administrative").await;

    // planned -> invoiced, an edge only an administrative load has.
    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "invoice_number": "JQL-4581461" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Reclassifying it now would leave a status the freight machine can't explain.
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "kind": "freight" }))
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());
}
```

Confirm how `AppError::Conflict` maps to an HTTP status in `src/error.rs` before asserting `409`; use whatever that mapping actually produces.

- [ ] **Step 9: Run to verify it fails**

```bash
cargo test --test administrative_loads_test test_kind_cannot_change
```

Expected: compile error — `apply_load_kind_change` does not exist.

- [ ] **Step 10: Add the guarded kind-change helper**

In `src/api/fleet_portal/data.rs`, next to `build_load_detail`:

```rust
/// Change a load's kind, or explain why it can't move.
///
/// Kind is only mutable while the load is still `planned` and has no trips.
/// Past that, the two models have already diverged: a freight load with trips
/// cannot become administrative without orphaning them, and an administrative
/// load that has already invoiced from `planned` used an edge a freight load
/// never had, so relabelling it would leave a status the freight machine
/// cannot explain.
pub(crate) async fn apply_load_kind_change(
    state: &AppState, id: Uuid, kind: crate::models::LoadKind,
) -> Result<crate::models::LoadRecord, AppError> {
    let load = state.db.get_load_by_id(id).await?;
    if load.kind == kind {
        return Ok(load);
    }
    if load.status != LoadStatus::Planned {
        return Err(AppError::Conflict(format!(
            "cannot change kind of a load in '{}' — only a 'planned' load can be reclassified",
            load.status.as_str(),
        )));
    }
    let trips = state.db.list_trips_for_load(id).await.unwrap_or_default();
    if !trips.is_empty() {
        return Err(AppError::Conflict(format!(
            "cannot change kind: load has {} trip(s). Cancel or detach them first.",
            trips.len(),
        )));
    }
    state.db.update_load_kind(id, kind).await
}
```

Then route both update handlers through it. In `src/api/fleet_portal/data.rs`, replace the block added in Task 5 Step 7:

```rust
    if let Some(k) = body.kind {
        updated = state.db.update_load_kind(id, k).await?;
    }
```

with:

```rust
    if let Some(k) = body.kind {
        updated = apply_load_kind_change(&state, id, k).await?;
    }
```

Apply the same substitution in `tool_update_load` in `src/api/fleet_portal/mcp.rs`, calling `super::data::apply_load_kind_change(state, id, k).await.map_err(|e| e.to_string())?`.

- [ ] **Step 11: Run the tests**

```bash
cargo test --test administrative_loads_test
cargo test test_administrative_loads_are_not_queued_for_routing
```

Expected: all PASS.

- [ ] **Step 12: Full verification**

```bash
cargo test && cargo clippy --all-targets && cargo build
```

- [ ] **Step 13: Commit**

```bash
git add -A && git commit -s -m "feat(loads): isolate administrative loads from trips and routing

A trip against an administrative load is rejected: attaching one would put
false mileage, stop actuals and a driver assignment into the operational
record, which is what the kind exists to avoid.

Kind is only mutable while the load is planned and trip-free.

Also stops the ORS routing requeue: list_loads_needing_routing matches any
load with miles IS NULL in a non-terminal status, so every administrative
load was re-queued on every startup, forever.

Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>"
```

---

## Task 7: `list_loads` filter passthrough

`tool_list_loads` hardcodes `None` / `&[]` for customer, tags, from, and to, so those filters are silently ignored — passing `tags: "guarantee"` returns every load. Its schema additionally advertises `facility_id`, which the handler also ignores: an actively false advertisement. The DB layer already supports the rest.

**Files:**
- Modify: `src/db/load_ops.rs` (`build_load_filter` 400, `list_loads` 152, `search_loads` 174)
- Modify: `src/api/loads.rs` (`ListLoadsQuery`, line 13)
- Modify: `src/api/fleet_portal/data.rs` (list handler ~232)
- Modify: `src/api/fleet_portal/mcp.rs` (`list_loads` schema 798, `tool_list_loads` 2056)
- Test: `src/db/load_ops.rs` tests module

**Interfaces:**
- Produces: `build_load_filter(status, customer, tags, facility_id: Option<Uuid>, from, to)` — note `facility_id` is inserted as the **fourth** parameter; both `list_loads` and `search_loads` gain the same parameter in the same position.

- [ ] **Step 1: Write the failing tests**

In `src/db/load_ops.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn test_list_loads_filters_by_tag() {
        let (db, _dir) = test_db().await;
        let mut tagged = sample_load();
        tagged.tags = vec!["guarantee".into(), "no-trip".into()];
        db.insert_load(&tagged).await.unwrap();
        let mut other = sample_load();
        other.id = uuid::Uuid::new_v4();
        other.tags = vec!["flatbed".into()];
        db.insert_load(&other).await.unwrap();

        let (total, items) = db.list_loads(
            None, None, &["guarantee".to_string()], None, None, None, 20, 0,
        ).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, tagged.id);
    }

    #[tokio::test]
    async fn test_list_loads_filters_by_facility() {
        let (db, _dir) = test_db().await;
        let fac = uuid::Uuid::new_v4();
        let mut at_facility = sample_load();
        at_facility.stops = vec![crate::models::Stop {
            sequence: 1,
            stop_type: crate::models::StopType::Pickup,
            service_type: crate::models::ServiceType::LiveLoad,
            facility_id: fac,
            scheduled_arrive: "2026-07-29T08:00:00".into(),
            scheduled_arrive_end: None, actual_arrive: None, actual_depart: None,
            expected_dwell_minutes: None, detention_free_minutes: None,
            detention_grace_minutes: None, notes: None, blob_ids: vec![],
            timezone: Some("America/Chicago".into()),
            actual_arrive_utc: None, actual_depart_utc: None,
        }];
        db.insert_load(&at_facility).await.unwrap();
        let mut elsewhere = sample_load();
        elsewhere.id = uuid::Uuid::new_v4();
        db.insert_load(&elsewhere).await.unwrap();

        let (total, items) = db.list_loads(
            None, None, &[], Some(fac), None, None, 20, 0,
        ).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, at_facility.id);
    }
```

If `Stop`'s field list has drifted, read `src/models/load.rs:126-156` and match it exactly rather than guessing.

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test test_list_loads_filters_by
```

Expected: compile error — `list_loads` takes 7 arguments, 8 supplied.

- [ ] **Step 3: Add `facility_id` to the filter builder**

In `src/db/load_ops.rs`, `build_load_filter` (line 400) — change the signature and add the clause. `facility_id` is a `Uuid`, so it needs no quote escaping:

```rust
fn build_load_filter(
    status: Option<&str>, customer: Option<&str>,
    tags: &[String], facility_id: Option<Uuid>,
    from: Option<&str>, to: Option<&str>,
) -> Result<Option<String>, AppError> {
```

and after the `for tag in tags { ... }` loop:

```rust
    if let Some(f) = facility_id {
        parts.push(format!("stops LIKE '%\"{f}\"%'"));
    }
```

This is the same technique `list_unrouted_loads_for_facility` already uses on the same column.

- [ ] **Step 4: Thread it through both callers**

In `list_loads` (line 152), add the parameter after `tag_filter` and pass it through:

```rust
    pub async fn list_loads(
        &self,
        status_filter: Option<&str>,
        customer_filter: Option<&str>,
        tag_filter: &[String],
        facility_filter: Option<Uuid>,
        from_date: Option<&str>,
        to_date: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<LoadListItem>), AppError> {
        let filter = build_load_filter(
            status_filter, customer_filter, tag_filter, facility_filter, from_date, to_date,
        )?;
```

In `search_loads` (line 174), add the same parameter in the same position and pass it:

```rust
        let filter = build_load_filter(
            status_filter, customer_filter, tag_filter, facility_filter, None, None,
        )?;
```

- [ ] **Step 5: Update the REST query struct and handler**

In `src/api/loads.rs`, `ListLoadsQuery` (line 13), after the `tag` field:

```rust
    /// Filter to loads with a stop at this facility
    pub facility_id: Option<Uuid>,
```

In `src/api/fleet_portal/data.rs`, the list handler (~line 232), add the argument between `&q.tag` and `q.from.as_deref()`:

```rust
        q.facility_id,
```

Fix any other `list_loads(` or `search_loads(` call sites the compiler flags — pass `None` for `facility_filter` where the caller has no facility. `src/api/fleet_portal/mcp.rs:520` is one such site.

- [ ] **Step 6: Wire the MCP tool**

In `src/api/fleet_portal/mcp.rs`, replace `tool_list_loads` (line 2056) entirely:

```rust
async fn tool_list_loads(state: &AppState, args: &Value) -> Result<Value, String> {
    let status = args["status"].as_str();
    let customer = args["customer"].as_str();
    let from = args["from"].as_str();
    let to = args["to"].as_str();
    let tags: Vec<String> = args["tags"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let facility_id = match args.get("facility_id") {
        Some(Value::String(s)) => Some(
            s.parse::<uuid::Uuid>().map_err(|_| "facility_id must be a UUID".to_string())?,
        ),
        _ => None,
    };
    let offset = cursor_offset(args)?;

    let (total, items) = state.db.list_loads(
        status, customer, &tags, facility_id, from, to, PAGE_SIZE, offset,
    ).await.map_err(|e| e.to_string())?;

    let returned = items.len();
    Ok(mcp_content(paged(items, returned, total, offset)))
}
```

- [ ] **Step 7: Update the tool schema**

In `src/api/fleet_portal/mcp.rs`, `list_loads` (line 798) — replace the whole entry's `description` and `inputSchema`:

```json
                "name": "list_loads",
                "description": "List loads. Optional filters, all ANDed together: status, customer (substring), tags (a load must carry every tag given), facility_id (loads with a stop at that facility), from/to (created_at bounds, ISO 8601).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "status": { "type": "string", "enum": ["planned","assigned","dispatched","in_transit","delivered","invoiced","settled","cancelled"] },
                        "customer": { "type": "string", "description": "Substring match on customer name." },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Load must carry every tag listed." },
                        "facility_id": { "type": "string", "format": "uuid" },
                        "from": { "type": "string", "description": "created_at >= this RFC 3339 datetime." },
                        "to": { "type": "string", "description": "created_at <= this RFC 3339 datetime." }
                    }
                }
```

There is deliberately no `kind` filter. The unsettled query keys off status, which this work corrects; a kind filter is speculative until asked for. Only status, customer, tags, facility_id, from, and to are wired, and the description names exactly those — never advertise a filter the handler ignores, which is the defect this task exists to fix.

- [ ] **Step 8: Run the tests**

```bash
cargo test test_list_loads_filters_by
```

Expected: both PASS.

- [ ] **Step 9: Full verification**

```bash
cargo test && cargo clippy --all-targets && cargo build
```

- [ ] **Step 10: Commit**

```bash
git add -A && git commit -s -m "fix(mcp): wire up the list_loads filters instead of ignoring them

tool_list_loads hardcoded None/&[] for customer, tags, from and to, so
passing tags returned every load. facility_id was worse: advertised in the
tool schema and silently dropped by the handler.

facility_id is now implemented against the stops column, the same technique
list_unrouted_loads_for_facility already uses, and added to the REST query
struct for OpenAPI parity.

Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>"
```

---

## Task 8: Fleet SPA invoice gate and kind badge

Without this, the MCP path works but the UI never offers Invoice on a planned administrative load.

**Files:**
- Modify: `static/fleet/pages/load-detail.js` (line 198)
- Create: `tests/fleet/load-detail.test.js`

**Interfaces:**
- Consumes: `LoadDetailResponse.kind` (Task 5)
- Produces: `export function invoiceableFromStatus(load)` from `static/fleet/pages/load-detail.js`

- [ ] **Step 1: Write the failing test**

Create `tests/fleet/load-detail.test.js`:

```js
import { describe, it, expect } from 'vitest';
import { invoiceableFromStatus } from '../../static/fleet/pages/load-detail.js';

describe('invoiceableFromStatus', () => {
  it('allows a delivered freight load', () => {
    expect(invoiceableFromStatus({ status: 'delivered', kind: 'freight' })).toBe(true);
  });

  it('allows a planned administrative load', () => {
    expect(invoiceableFromStatus({ status: 'planned', kind: 'administrative' })).toBe(true);
  });

  it('refuses a planned freight load', () => {
    expect(invoiceableFromStatus({ status: 'planned', kind: 'freight' })).toBe(false);
  });

  it('refuses an in_transit administrative load', () => {
    expect(invoiceableFromStatus({ status: 'in_transit', kind: 'administrative' })).toBe(false);
  });

  it('refuses an already-invoiced load', () => {
    expect(invoiceableFromStatus({ status: 'invoiced', kind: 'administrative' })).toBe(false);
  });

  it('treats a load with no kind as freight', () => {
    expect(invoiceableFromStatus({ status: 'delivered' })).toBe(true);
    expect(invoiceableFromStatus({ status: 'planned' })).toBe(false);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
npm test -- load-detail
```

Expected: FAIL — `invoiceableFromStatus is not a function`.

- [ ] **Step 3: Extract and export the predicate**

In `static/fleet/pages/load-detail.js`, add near the top, next to the existing `PRE_DELIVERY` constant (line 9):

```js
// A freight load earns `invoiced` by delivering. An administrative load has no
// trip and never reaches `delivered`, so it invoices straight from `planned`.
export function invoiceableFromStatus(load) {
  return load.status === 'delivered'
    || (load.status === 'planned' && load.kind === 'administrative');
}
```

- [ ] **Step 4: Use it in the gate**

At line 198, change:

```js
    const canInvoice = hasScope('loads:invoice') && load.status === 'delivered';
```

to:

```js
    const canInvoice = hasScope('loads:invoice') && invoiceableFromStatus(load);
```

- [ ] **Step 5: Show the kind on the detail card**

In the same file, find the `detail-item` block that renders `load.status` (search for `detail-item__value` near the status field) and add a sibling item immediately after it. Match the surrounding markup exactly — copy the neighbouring `detail-item` block's structure rather than inventing one:

```js
          ${load.kind === 'administrative' ? `
          <div class="detail-item">
            <div class="detail-item__label">Kind</div>
            <div class="detail-item__value"><span class="badge badge--load">Administrative</span></div>
          </div>` : ''}
```

Only rendered for administrative loads — a "Kind: Freight" row on every ordinary load is noise.

- [ ] **Step 6: Run the tests**

```bash
npm test -- load-detail
```

Expected: all six PASS.

- [ ] **Step 7: Full verification**

```bash
npm test && cargo test && cargo clippy --all-targets
```

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -s -m "feat(fleet): offer Invoice on a planned administrative load

Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>"
```

---

## Task 9: Acceptance test and invariant

Proves the reported sequence end-to-end through the real MCP surface, and records the lesson.

**Files:**
- Modify: `tests/administrative_loads_test.rs`
- Modify: `AGENTS.md`

**Interfaces:**
- Consumes: everything from Tasks 1-8

- [ ] **Step 1: Write the failing acceptance test**

Append to `tests/administrative_loads_test.rs`, reusing the `setup()` / `setup_owner()` / `create_bare_load()` helpers already in that file from Task 6.

```rust
/// The reported sequence, verbatim: a weekly revenue guarantee that was paid on
/// a contractor settlement but has no trip behind it. Before this shipped,
/// `invoice_load` answered `cannot transition from 'planned' to 'invoiced'` and
/// the load sat in `planned` forever, so every "what is unsettled?" query
/// returned it week after week.
#[tokio::test]
async fn test_guarantee_load_invoices_and_settles_with_no_trip() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    // Starts as an ordinary load, and is reclassified in place — not recreated.
    let load_id = create_bare_load(&server, &token, "4581461", "freight").await;
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "kind": "administrative" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "invoice_number": "JQL-4581461",
            "invoice_date": "2026-07-29",
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let uuid: uuid::Uuid = load_id.parse().unwrap();
    let settled = state.db.get_load_by_id(uuid).await.unwrap();
    assert_eq!(settled.status, LoadStatus::Settled);
    assert_eq!(settled.invoice_number.as_deref(), Some("JQL-4581461"));
    assert!(state.db.list_trips_for_load(uuid).await.unwrap().is_empty());

    // It drops out of the unsettled queue.
    let resp = server.get("/fleet/api/v1/loads?status=planned")
        .authorization_bearer(&token)
        .await;
    assert_eq!(resp.json::<serde_json::Value>()["total"], serde_json::json!(0));

    // And load_doctor does not flag it. The check gates on Dispatched|InTransit
    // and load_trips_all_delivered is false on an empty survivor set, so this is
    // regression insurance against a later loosening of either.
    let report = ollie::services::doctors::load::run(&state, uuid, false).await.unwrap();
    assert!(
        !report.findings.iter().any(|f| f.check == "load.status_matches_trips"),
        "administrative loads must not be flagged as status-mismatched: {:?}",
        report.findings,
    );
}
```

Check the list response's actual field name before asserting on `total` — `DispatchLoadListResponse` in `src/api/fleet_portal/data.rs` carries both `returned` and `total`.

- [ ] **Step 2: Run it**

```bash
cargo test --test administrative_loads_test test_guarantee_load_invoices_and_settles_with_no_trip
```

Expected: PASS. Everything it exercises landed in Tasks 1-8, so this is a green-on-first-run confirmation, not a red. If it fails, the failure names the gap — fix it in the owning task's file rather than patching around it here.

- [ ] **Step 3: Record the invariant**

Append to the invariants list in `AGENTS.md`, after the #395 entry added by PR #408. Follow that entry's `— Why:` / `— How to apply:` structure:

```markdown
- **A status machine denormalized from child records strands any parent that legitimately has no children.** `LoadStatus::can_transition_to` allowed only `Delivered → Invoiced → Settled`, and every route to `Delivered` is a trip-side cascade. That is right for freight and wrong for revenue that was never driven — a weekly revenue guarantee, TONU, detention-only billing, layover, an accessorial-only freight bill. All of them are billable, none has a trip, and all of them sat in `planned` forever while the money was collected, so every status-based "what is unsettled?" query returned false positives week after week (#409). `LoadKind::Administrative` plus a kind-aware `LoadRecord::can_transition_to` grants exactly one extra edge, `Planned → Invoiced`. — Why: the trip-shaped path looks total because every load anyone had modelled so far *did* move. The gap only appears when a real record arrives that is legitimately childless, and by then the only workaround is fabricating the children — a fake trip, fake driver assignment, fake stop actuals — which corrupts the operational record far worse than the wrong status did. — How to apply: when a parent's status is denormalized from its children, ask what the machine does for a parent with zero children *by design*, not by accident. If the answer is "it is stuck", the model needs a kind, not another status. Give the kind its own edge rather than relaxing the shared one, so the freight machine keeps its guarantees; then isolate the two — an administrative load is refused a trip outright, is excluded from the routing requeue, and can only be reclassified while `planned` and trip-free. **The routing requeue is the part that hides.** `list_loads_needing_routing` selects on `miles IS NULL AND status NOT IN (terminal)`, so a stopless load matched it on every startup, forever — a silent permanent zombie in the recovery pass that no one would look for.
```

- [ ] **Step 4: Full verification**

```bash
cargo test && cargo clippy --all-targets && cargo build && npm test
```

Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -s -m "test(loads): acceptance coverage for no-trip guarantee loads

Replays the reported sequence: a weekly revenue guarantee reclassified in
place, then invoiced and settled with no trip, dropping out of the unsettled
queue and drawing no load_doctor finding.

Co-Authored-By: Claude with claude-opus-5 <noreply@anthropic.com>"
```

---

## Out of scope

Do not implement these here.

- **`set_load_status`** — a guarded, forward-only, actor-logged status setter. Its own issue, stacked on #395. PR #408 already ships the `dispatched`/`in_transit` repair path via `load_doctor apply=true`, so what remains is generic correction for other causes.
- **Issue #396** — the carrier settlement statement entity.
- **Fleet UI for creating administrative loads** — the load form is stop-centric; reshaping it for stopless loads is a separate piece of work. Creating administrative loads stays an MCP/API operation. Confirmed deferred by the user.
- **A `kind` filter on `list_loads`** — the unsettled query keys off status, which the fix corrects. Speculative until asked for.
- **Freight-requires-stops validation** — see Task 5 Step 4.
- **Version or PWA cache stamp bumps** — owned by `cut-release`.

## Self-review notes

Spec coverage checked section by section: data model → Task 1; persistence and migration → Task 1; transition policy → Task 2; tangle guards → Task 6; routing → Task 6; lifecycle events → Tasks 3 and 4; `list_loads` passthrough → Task 7; fleet SPA → Task 8; `stops` relaxation → Task 5; testing and acceptance → spread across each task plus Task 9.

Type consistency checked: `LoadKind` / `as_str` / `FromStr` / `Default` defined in Task 1 and used unchanged in 2, 5, 6, 7; `update_load_kind` defined in Task 5 and consumed by `apply_load_kind_change` in Task 6; `apply_load_kind_change` defined in Task 6 and consumed in Task 9; `invoiceableFromStatus` defined and consumed within Task 8; `build_load_filter`'s new `facility_id` parameter is inserted at the same position in all three signatures in Task 7.

One deliberate temporary state: Task 1 Step 8 hardcodes `kind: LoadKind::Freight` in the two create handlers, which Task 5 Step 6 replaces with `body.kind.unwrap_or_default()`. Called out in both places.
