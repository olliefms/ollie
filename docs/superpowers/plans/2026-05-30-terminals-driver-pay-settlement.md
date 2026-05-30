# Terminals, Tiered Driver Pay & Settlement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `terminals` table, attach drivers to terminals, compute tiered driver pay (loaded/deadhead/extra-stop/detention) resolved per-field across trip → driver → terminal, and add settlement linkage that freezes pay once a trip is settled — landing #185, #132, and #134 atomically.

**Architecture:** A new `terminals` table holds the mandatory rate floor (every rate field non-null). Drivers gain a required `terminal_id` plus optional rate overrides; trips gain the same optional overrides plus settlement fields and a frozen pay snapshot. A pure `resolve_rates(trip, driver, terminal)` merges the three tiers per-field; a pure `compute_driver_pay(trip, rates)` produces a `DriverPay` from miles + stop dwell times. Pay is computed on read in the trip GET/list assembly, unless a `driver_pay_snapshot` exists (set when `settlement_ref` is first assigned), in which case the snapshot is returned verbatim and pay-affecting edits are rejected.

**Tech Stack:** Rust, Axum, LanceDB (Arrow schema + DataFusion SQL migrations), utoipa (OpenAPI), vanilla JS dispatcher UI, `axum-test` + `tempfile` integration tests.

---

## ⚠️ Critical Constraints (read before any DB task)

1. **LanceDB migration CAST type names are DataFusion SQL keywords, NOT Arrow names.** Use `string`, `double`, `bigint`, `boolean` — NEVER `Utf8`/`Float64`/`Int64`/`utf8`/`float64`. This bug has crash-looped production three times (v1.10.0/v1.13.0/v1.16.0). Copy CAST spellings from existing working migrations in `src/db/mod.rs` (e.g. `"CAST(NULL AS string)"`, `"CAST(NULL AS double)"`, `"CAST(0 AS BIGINT)"`). Every new migration column needs an **existing-DB** migration test, not just a fresh-DB unit test (Task 16).
2. **No version bumps.** Do not edit `Cargo.toml` version, `package.json`, PWA `CACHE_NAME`, or `?v=` asset stamps. Versioning is `cut-release`'s job.
3. **Upserts use `merge_insert(&["id"])`** with `when_matched_update_all(None).when_not_matched_insert_all()` — never delete+insert.
4. **After every task:** `cargo test`, `cargo clippy`, `cargo build` must pass before committing. Keep tests green; never proceed on red.
5. **New endpoints go on the dispatcher portal** (`/dispatch/api/v1/...`), NOT admin `/api/v1/...` (admin API is being deprecated per #236).

---

## File Structure

**New files:**
- `src/models/terminal.rs` — `TerminalRecord`, `CreateTerminalRequest`, `UpdateTerminalRequest`, `TerminalListItem`.
- `src/models/pay.rs` — `RateSchedule`, `DriverPay`, pure `resolve_rates()` and `compute_driver_pay()`.
- `src/db/terminal_ops.rs` — terminal CRUD + `batch_get_terminals`.
- `src/api/dispatcher_portal/terminal_writes.rs` — terminal HTTP handlers + shared `apply_*` logic.
- `tests/terminals_pay_settlement_test.rs` — integration + migration tests for this sprint.

**Modified files:**
- `src/models/trip.rs` — trip rate overrides, settlement fields, `driver_pay_snapshot`, `driver_pay` on `TripListItem`/response.
- `src/models/driver.rs` — `terminal_id` + rate overrides on record/requests/list-item.
- `src/db/mod.rs` — `terminal_schema()`, `open_or_create_terminal()` (with Default seed), driver + trip migrations, register `terminal_table`.
- `src/db/trip_ops.rs` — read/write new trip columns; `update_trip_rate_overrides`; `update_trip_settlement`; pay-period list filter.
- `src/db/driver_ops.rs` — read/write `terminal_id` + driver rate columns (add `opt_f64`/`opt_i64` to `row_to_driver`).
- `src/api/dispatcher_portal/data.rs` — `driver_pay_for_record` helper; attach `driver_pay` in `build_trip_detail`; pay-period query param; edit-lock in `stop_arrive`/`stop_depart`.
- `src/api/dispatcher_portal/trip_writes.rs` — extend `PatchTripBody` with rate overrides + settlement fields; settlement freeze + edit-lock in `apply_trip_patch`.
- `src/api/trips.rs` (admin) — only updated to match the new `db::list_trips` signature (pass `None` for new bounds). NOT where pay/settlement live.
- `src/api/dispatcher_portal/mod.rs` — register terminal routes.
- `src/config.rs` — remove `terminal_timezone` field; keep `free_dwell_minutes` only for the Default-terminal seed.
- `src/api/driver_portal/data.rs` — read free-dwell from driver's terminal, not `config.terminal_timezone`/`config.free_dwell_minutes`.
- `static/dispatch/index.html` + `static/dispatch/app.js` — Terminals list/create/edit view.

---

## Phase A — Terminals foundation (#185 core)

### Task 1: `TerminalRecord` model

**Files:**
- Create: `src/models/terminal.rs`
- Modify: `src/models/mod.rs` (add `pub mod terminal;` and re-exports)

- [ ] **Step 1: Write the model file**

```rust
// src/models/terminal.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// A terminal (yard/HQ). Anchors pay-period timezone and the mandatory rate floor.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TerminalRecord {
    pub id: Uuid,
    pub name: String,
    /// Freeform address string (no dedicated Address struct exists in this codebase;
    /// facilities use a plain String — we match that and allow null).
    pub address: Option<String>,
    /// IANA timezone name, e.g. "America/New_York".
    pub timezone: String,
    /// Exactly one terminal is the default (used as the resolution floor / seed).
    pub is_default: bool,
    // --- mandatory rate floor: always concrete ---
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub extra_stop_fee: f64,
    pub detention_rate_per_hour: f64,
    pub free_dwell_minutes: u32,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTerminalRequest {
    pub name: String,
    #[serde(default)]
    pub address: Option<String>,
    pub timezone: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub loaded_rate_per_mile: f64,
    #[serde(default)]
    pub deadhead_rate_per_mile: f64,
    #[serde(default)]
    pub extra_stop_fee: f64,
    #[serde(default)]
    pub detention_rate_per_hour: f64,
    /// Defaults to 120 if omitted.
    #[serde(default = "default_free_dwell")]
    pub free_dwell_minutes: u32,
}

fn default_free_dwell() -> u32 { 120 }

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTerminalRequest {
    pub name: Option<String>,
    pub address: Option<String>,
    pub timezone: Option<String>,
    pub is_default: Option<bool>,
    pub loaded_rate_per_mile: Option<f64>,
    pub deadhead_rate_per_mile: Option<f64>,
    pub extra_stop_fee: Option<f64>,
    pub detention_rate_per_hour: Option<f64>,
    pub free_dwell_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TerminalListItem {
    pub id: Uuid,
    pub name: String,
    pub address: Option<String>,
    pub timezone: String,
    pub is_default: bool,
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub extra_stop_fee: f64,
    pub detention_rate_per_hour: f64,
    pub free_dwell_minutes: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<TerminalRecord> for TerminalListItem {
    fn from(r: TerminalRecord) -> Self {
        Self {
            id: r.id, name: r.name, address: r.address, timezone: r.timezone,
            is_default: r.is_default,
            loaded_rate_per_mile: r.loaded_rate_per_mile,
            deadhead_rate_per_mile: r.deadhead_rate_per_mile,
            extra_stop_fee: r.extra_stop_fee,
            detention_rate_per_hour: r.detention_rate_per_hour,
            free_dwell_minutes: r.free_dwell_minutes,
            created_at: r.created_at, updated_at: r.updated_at,
        }
    }
}
```

- [ ] **Step 2: Register the module**

In `src/models/mod.rs`, add `pub mod terminal;` alongside the other `pub mod` lines, and add `pub use terminal::{TerminalRecord, TerminalListItem};` next to the existing re-exports (match the existing re-export style in that file).

- [ ] **Step 3: Build to verify it compiles**

Run: `cargo build`
Expected: compiles (unused-code warnings for the new types are fine at this stage).

- [ ] **Step 4: Commit**

```bash
git add src/models/terminal.rs src/models/mod.rs
git commit -m "feat(terminals): add TerminalRecord model and request/list types (#185)"
```

---

### Task 2: `terminal_schema()` + `open_or_create_terminal()` with Default seed

**Files:**
- Modify: `src/db/mod.rs` (add schema fn, open_or_create fn, register table in `DbClient`)

**Reference:** mirror `trip_schema()` (mod.rs:604-630) and `open_or_create_trip()` (mod.rs:176-215). The `DbClient` struct holds `trip_table`, `driver_table`, etc. — add `terminal_table` the same way.

- [ ] **Step 1: Add `terminal_schema()`**

Add near the other `*_schema` functions in `src/db/mod.rs`:

```rust
fn terminal_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("address", DataType::Utf8, true),
        Field::new("timezone", DataType::Utf8, false),
        Field::new("is_default", DataType::Boolean, false),
        Field::new("loaded_rate_per_mile", DataType::Float64, false),
        Field::new("deadhead_rate_per_mile", DataType::Float64, false),
        Field::new("extra_stop_fee", DataType::Float64, false),
        Field::new("detention_rate_per_hour", DataType::Float64, false),
        Field::new("free_dwell_minutes", DataType::Int64, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}
```

Note: the Arrow `Schema` uses `DataType::*` (Arrow names) — that is correct here. The SQL-keyword rule applies ONLY to `CAST(NULL AS ...)` strings in migrations, not to Arrow `Field` definitions.

- [ ] **Step 2: Add `open_or_create_terminal()` with the Default seed**

The Default terminal is seeded as the table's initial row when the table doesn't exist. Timezone comes from the `TERMINAL_TIMEZONE` env (read here directly, since we're removing it from `Config` in Task 15); rates are 0.0; free-dwell is 120.

```rust
async fn open_or_create_terminal(conn: &lancedb::Connection) -> Result<Table, AppError> {
    let schema = terminal_schema();
    match conn.open_table("terminals").execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            let tz = std::env::var("TERMINAL_TIMEZONE")
                .unwrap_or_else(|_| "America/New_York".to_string());
            let free_dwell: i64 = std::env::var("OLLIE_FREE_DWELL_MINUTES")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(120);
            let now = chrono::Utc::now().to_rfc3339();
            let id = uuid::Uuid::new_v4().to_string();
            let batch = RecordBatch::try_new(schema.clone(), vec![
                Arc::new(StringArray::from(vec![id.as_str()])),
                Arc::new(StringArray::from(vec!["Default"])),
                Arc::new(StringArray::from(vec![None::<&str>])),
                Arc::new(StringArray::from(vec![tz.as_str()])),
                Arc::new(BooleanArray::from(vec![true])),
                Arc::new(Float64Array::from(vec![0.0_f64])),
                Arc::new(Float64Array::from(vec![0.0_f64])),
                Arc::new(Float64Array::from(vec![0.0_f64])),
                Arc::new(Float64Array::from(vec![0.0_f64])),
                Arc::new(Int64Array::from(vec![free_dwell])),
                Arc::new(Int64Array::from(vec![0_i64])),
                Arc::new(StringArray::from(vec![now.as_str()])),
                Arc::new(StringArray::from(vec![now.as_str()])),
            ]).map_err(|e| AppError::Internal(e.to_string()))?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("terminals", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
    }
}
```

Verify the needed Arrow array imports (`BooleanArray`, `Float64Array`, `Int64Array`, `StringArray`) are already imported at the top of `src/db/mod.rs`; add any missing ones to the existing `use arrow_array::{...}` import.

- [ ] **Step 3: Register `terminal_table` on `DbClient`**

Find the `DbClient` struct definition and its constructor (`DbClient::new`) in `src/db/mod.rs`. Add a `pub terminal_table: Table,` field, and in `new()` call `let terminal_table = open_or_create_terminal(&conn).await?;` and include `terminal_table` in the struct construction (mirror how `trip_table` is wired).

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src/db/mod.rs
git commit -m "feat(terminals): terminal_schema + open_or_create_terminal with seeded Default (#185)"
```

---

### Task 3: Terminal CRUD ops

**Files:**
- Create: `src/db/terminal_ops.rs`
- Modify: `src/db/mod.rs` (add `mod terminal_ops;` / `impl DbClient` block include — match how `trip_ops` is wired; if ops are `impl DbClient` blocks in their own files, add `mod terminal_ops;`)

**Reference:** mirror `src/db/driver_ops.rs` (insert/get/list/update/soft-delete + `batch_get_drivers`) and its `row_to_driver` helper.

- [ ] **Step 1: Write the ops file**

```rust
// src/db/terminal_ops.rs
use std::collections::HashMap;
use std::sync::Arc;
use arrow_array::{RecordBatch, RecordBatchIterator, RecordBatchReader,
    StringArray, BooleanArray, Float64Array, Int64Array, Array};
use chrono::Utc;
use lancedb::query::{ExecutableQuery, QueryBase};
use futures::TryStreamExt;
use uuid::Uuid;

use crate::db::{DbClient, terminal_schema};
use crate::error::AppError;
use crate::models::{TerminalRecord, TerminalListItem};

fn terminal_to_batch(r: &TerminalRecord) -> Result<RecordBatch, AppError> {
    let schema = terminal_schema();
    let id = r.id.to_string();
    let created = r.created_at.to_rfc3339();
    let updated = r.updated_at.to_rfc3339();
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![id.as_str()])),
        Arc::new(StringArray::from(vec![r.name.as_str()])),
        Arc::new(StringArray::from(vec![r.address.as_deref()])),
        Arc::new(StringArray::from(vec![r.timezone.as_str()])),
        Arc::new(BooleanArray::from(vec![r.is_default])),
        Arc::new(Float64Array::from(vec![r.loaded_rate_per_mile])),
        Arc::new(Float64Array::from(vec![r.deadhead_rate_per_mile])),
        Arc::new(Float64Array::from(vec![r.extra_stop_fee])),
        Arc::new(Float64Array::from(vec![r.detention_rate_per_hour])),
        Arc::new(Int64Array::from(vec![r.free_dwell_minutes as i64])),
        Arc::new(Int64Array::from(vec![r.owner_id])),
        Arc::new(StringArray::from(vec![created.as_str()])),
        Arc::new(StringArray::from(vec![updated.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn row_to_terminal(batch: &RecordBatch, i: usize) -> Result<TerminalRecord, AppError> {
    let s = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string()).unwrap_or_default()
    };
    let opt_s = |name: &str| -> Option<String> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) })
    };
    let f = |name: &str| -> f64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .map(|a| a.value(i)).unwrap_or(0.0)
    };
    let i64c = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i)).unwrap_or(0)
    };
    let b = |name: &str| -> bool {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
            .map(|a| a.value(i)).unwrap_or(false)
    };
    let parse_dt = |raw: String| chrono::DateTime::parse_from_rfc3339(&raw)
        .map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now());
    Ok(TerminalRecord {
        id: s("id").parse().map_err(|e| AppError::Internal(format!("{e}")))?,
        name: s("name"),
        address: opt_s("address"),
        timezone: s("timezone"),
        is_default: b("is_default"),
        loaded_rate_per_mile: f("loaded_rate_per_mile"),
        deadhead_rate_per_mile: f("deadhead_rate_per_mile"),
        extra_stop_fee: f("extra_stop_fee"),
        detention_rate_per_hour: f("detention_rate_per_hour"),
        free_dwell_minutes: i64c("free_dwell_minutes") as u32,
        owner_id: i64c("owner_id"),
        created_at: parse_dt(s("created_at")),
        updated_at: parse_dt(s("updated_at")),
    })
}

impl DbClient {
    async fn all_terminal_batches(&self) -> Result<Vec<RecordBatch>, AppError> {
        self.terminal_table.query().execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .try_collect::<Vec<_>>().await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    async fn upsert_terminal(&self, r: &TerminalRecord) -> Result<(), AppError> {
        let batch = terminal_to_batch(r)?;
        let schema = terminal_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.terminal_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await.map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn insert_terminal(&self, r: &TerminalRecord) -> Result<(), AppError> {
        self.upsert_terminal(r).await
    }

    pub async fn get_terminal_by_id(&self, id: Uuid) -> Result<TerminalRecord, AppError> {
        let id_s = id.to_string();
        let batches = self.terminal_table.query()
            .only_if(format!("id = '{id_s}'")).execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .try_collect::<Vec<_>>().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        for batch in &batches {
            if batch.num_rows() > 0 { return row_to_terminal(batch, 0); }
        }
        // NOTE: AppError::NotFound is a UNIT variant (src/error.rs) — no String payload.
        Err(AppError::NotFound)
    }

    pub async fn batch_get_terminals(&self, ids: &[Uuid])
        -> Result<HashMap<Uuid, TerminalRecord>, AppError>
    {
        let mut out = HashMap::new();
        if ids.is_empty() { return Ok(out); }
        for batch in self.all_terminal_batches().await? {
            for i in 0..batch.num_rows() {
                let t = row_to_terminal(batch, i)?;
                if ids.contains(&t.id) { out.insert(t.id, t); }
            }
        }
        Ok(out)
    }

    pub async fn default_terminal(&self) -> Result<TerminalRecord, AppError> {
        for batch in self.all_terminal_batches().await? {
            for i in 0..batch.num_rows() {
                let t = row_to_terminal(batch, i)?;
                if t.is_default { return Ok(t); }
            }
        }
        Err(AppError::Internal("no default terminal found".into()))
    }

    pub async fn list_terminals(&self) -> Result<Vec<TerminalListItem>, AppError> {
        let mut out = Vec::new();
        for batch in self.all_terminal_batches().await? {
            for i in 0..batch.num_rows() {
                out.push(TerminalListItem::from(row_to_terminal(batch, i)?));
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// If `make_default` is true, clears `is_default` on all other terminals first.
    pub async fn set_terminal(&self, r: &TerminalRecord) -> Result<(), AppError> {
        if r.is_default {
            let others: Vec<TerminalRecord> = {
                let mut v = Vec::new();
                for batch in self.all_terminal_batches().await? {
                    for i in 0..batch.num_rows() {
                        let t = row_to_terminal(batch, i)?;
                        if t.id != r.id && t.is_default { v.push(t); }
                    }
                }
                v
            };
            for mut o in others {
                o.is_default = false;
                o.updated_at = Utc::now();
                self.upsert_terminal(&o).await?;
            }
        }
        self.upsert_terminal(r).await
    }

    pub async fn count_drivers_for_terminal(&self, terminal_id: Uuid) -> Result<usize, AppError> {
        let n = self.driver_table.query()
            .only_if(format!("terminal_id = '{}'", terminal_id))
            .execute().await.map_err(|e| AppError::Internal(e.to_string()))?
            .try_collect::<Vec<_>>().await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .iter().map(|b| b.num_rows()).sum();
        Ok(n)
    }

    pub async fn delete_terminal(&self, id: Uuid) -> Result<(), AppError> {
        self.terminal_table.delete(&format!("id = '{}'", id)).await
            .map_err(|e| AppError::Internal(e.to_string()))
    }
}
```

Note: verify `only_if`, `query()`, `delete()`, and the `futures::TryStreamExt` import match how `driver_ops.rs`/`trip_ops.rs` query the tables; adjust import paths to match the existing files exactly (copy their `use` headers).

- [ ] **Step 2: Wire module into `src/db/mod.rs`**

Add `mod terminal_ops;` next to the existing `mod trip_ops;` / `mod driver_ops;` declarations.

- [ ] **Step 3: Add a round-trip unit test**

Append to `src/db/terminal_ops.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn client() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let c = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (c, dir)
    }

    #[tokio::test]
    async fn seeded_default_terminal_exists() {
        let (c, _d) = client().await;
        let def = c.default_terminal().await.unwrap();
        assert_eq!(def.name, "Default");
        assert_eq!(def.free_dwell_minutes, 120);
        assert_eq!(def.loaded_rate_per_mile, 0.0);
    }

    #[tokio::test]
    async fn insert_get_roundtrip_and_single_default() {
        let (c, _d) = client().await;
        let mut t = TerminalRecord {
            id: Uuid::new_v4(), name: "West".into(), address: Some("1 A St".into()),
            timezone: "America/Los_Angeles".into(), is_default: true,
            loaded_rate_per_mile: 0.55, deadhead_rate_per_mile: 0.40,
            extra_stop_fee: 50.0, detention_rate_per_hour: 25.0, free_dwell_minutes: 90,
            owner_id: 0, created_at: Utc::now(), updated_at: Utc::now(),
        };
        c.set_terminal(&t).await.unwrap();
        let got = c.get_terminal_by_id(t.id).await.unwrap();
        assert_eq!(got.name, "West");
        assert_eq!(got.free_dwell_minutes, 90);
        // The originally-seeded Default must have been un-defaulted.
        let defaults: Vec<_> = c.list_terminals().await.unwrap()
            .into_iter().filter(|x| x.is_default).collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].id, t.id);
        // update
        t.name = "West Yard".into();
        t.updated_at = Utc::now();
        c.set_terminal(&t).await.unwrap();
        assert_eq!(c.get_terminal_by_id(t.id).await.unwrap().name, "West Yard");
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib terminal_ops`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add src/db/terminal_ops.rs src/db/mod.rs
git commit -m "feat(terminals): terminal CRUD ops + single-default invariant (#185)"
```

---

### Task 4: Terminal HTTP handlers + routes (dispatcher portal)

**Files:**
- Create: `src/api/dispatcher_portal/terminal_writes.rs`
- Modify: `src/api/dispatcher_portal/mod.rs` (register routes + `pub mod terminal_writes;`)

**Reference:** mirror `src/api/dispatcher_portal/facility_writes.rs` (shared `apply_*_create`/`apply_*_patch` + thin handlers) and the `data_router` registration block in `mod.rs:45-182`.

- [ ] **Step 1: Write handlers**

```rust
// src/api/dispatcher_portal/terminal_writes.rs
use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::terminal::{CreateTerminalRequest, UpdateTerminalRequest};
use crate::models::{TerminalRecord, TerminalListItem};
use crate::AppState;

fn validate_tz(tz: &str) -> Result<(), AppError> {
    tz.parse::<chrono_tz::Tz>()
        .map(|_| ())
        .map_err(|_| AppError::UnprocessableEntity(format!("invalid IANA timezone: {tz}")))
}

pub async fn apply_terminal_create(state: &AppState, req: CreateTerminalRequest)
    -> Result<TerminalRecord, AppError>
{
    validate_tz(&req.timezone)?;
    let now = Utc::now();
    let record = TerminalRecord {
        id: Uuid::new_v4(),
        name: req.name,
        address: req.address,
        timezone: req.timezone,
        is_default: req.is_default,
        loaded_rate_per_mile: req.loaded_rate_per_mile,
        deadhead_rate_per_mile: req.deadhead_rate_per_mile,
        extra_stop_fee: req.extra_stop_fee,
        detention_rate_per_hour: req.detention_rate_per_hour,
        free_dwell_minutes: req.free_dwell_minutes,
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };
    state.db.set_terminal(&record).await?;
    Ok(record)
}

pub async fn apply_terminal_patch(state: &AppState, id: Uuid, req: UpdateTerminalRequest)
    -> Result<TerminalRecord, AppError>
{
    let mut t = state.db.get_terminal_by_id(id).await?;
    if let Some(tz) = req.timezone { validate_tz(&tz)?; t.timezone = tz; }
    if let Some(v) = req.name { t.name = v; }
    if req.address.is_some() { t.address = req.address; }
    if let Some(v) = req.is_default { t.is_default = v; }
    if let Some(v) = req.loaded_rate_per_mile { t.loaded_rate_per_mile = v; }
    if let Some(v) = req.deadhead_rate_per_mile { t.deadhead_rate_per_mile = v; }
    if let Some(v) = req.extra_stop_fee { t.extra_stop_fee = v; }
    if let Some(v) = req.detention_rate_per_hour { t.detention_rate_per_hour = v; }
    if let Some(v) = req.free_dwell_minutes { t.free_dwell_minutes = v; }
    t.updated_at = Utc::now();
    state.db.set_terminal(&t).await?;
    Ok(t)
}

#[utoipa::path(post, path = "/dispatch/api/v1/terminals",
    request_body = CreateTerminalRequest,
    responses((status = 201, body = TerminalRecord), (status = 401), (status = 422)),
    security(("BearerAuth" = [])), tag = "dispatch")]
pub async fn create_terminal(State(state): State<AppState>,
    Json(req): Json<CreateTerminalRequest>) -> Result<impl IntoResponse, AppError>
{
    let r = apply_terminal_create(&state, req).await?;
    Ok((StatusCode::CREATED, Json(r)))
}

#[utoipa::path(get, path = "/dispatch/api/v1/terminals",
    responses((status = 200, body = [TerminalListItem]), (status = 401)),
    security(("BearerAuth" = [])), tag = "dispatch")]
pub async fn list_terminals(State(state): State<AppState>)
    -> Result<impl IntoResponse, AppError>
{
    Ok(Json(state.db.list_terminals().await?))
}

#[utoipa::path(get, path = "/dispatch/api/v1/terminals/{id}",
    responses((status = 200, body = TerminalRecord), (status = 401), (status = 404)),
    security(("BearerAuth" = [])), tag = "dispatch")]
pub async fn get_terminal(State(state): State<AppState>, Path(id): Path<Uuid>)
    -> Result<impl IntoResponse, AppError>
{
    Ok(Json(state.db.get_terminal_by_id(id).await?))
}

#[utoipa::path(put, path = "/dispatch/api/v1/terminals/{id}",
    request_body = UpdateTerminalRequest,
    responses((status = 200, body = TerminalRecord), (status = 401), (status = 404), (status = 422)),
    security(("BearerAuth" = [])), tag = "dispatch")]
pub async fn update_terminal(State(state): State<AppState>, Path(id): Path<Uuid>,
    Json(req): Json<UpdateTerminalRequest>) -> Result<impl IntoResponse, AppError>
{
    Ok(Json(apply_terminal_patch(&state, id, req).await?))
}

#[utoipa::path(delete, path = "/dispatch/api/v1/terminals/{id}",
    responses((status = 204), (status = 401), (status = 404), (status = 409)),
    security(("BearerAuth" = [])), tag = "dispatch")]
pub async fn delete_terminal(State(state): State<AppState>, Path(id): Path<Uuid>)
    -> Result<impl IntoResponse, AppError>
{
    let t = state.db.get_terminal_by_id(id).await?;
    if t.is_default {
        return Err(AppError::Conflict("cannot delete the default terminal".into()));
    }
    if state.db.count_drivers_for_terminal(id).await? > 0 {
        return Err(AppError::Conflict("terminal has assigned drivers".into()));
    }
    state.db.delete_terminal(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

Note: confirm `AppError::Conflict` and `AppError::UnprocessableEntity` variants exist in `src/error.rs`; if `Conflict` does not exist, use the closest existing 4xx variant (check the enum) and map it to `409`/`422` consistently. If `chrono_tz` is not already a dependency used elsewhere, reuse whatever timezone-validation helper `Config::from_env` uses (it validates `TERMINAL_TIMEZONE` via `chrono_tz::Tz` — so the crate is available).

- [ ] **Step 2: Register routes**

In `src/api/dispatcher_portal/mod.rs`, add `pub mod terminal_writes;` and inside `data_router` add (before the `.route_layer(...)` auth layer):

```rust
        .route("/dispatch/api/v1/terminals",
            get(terminal_writes::list_terminals).post(terminal_writes::create_terminal))
        .route("/dispatch/api/v1/terminals/{id}",  // axum 0.8 path syntax — NOT :id (would panic at build)
            get(terminal_writes::get_terminal)
                .put(terminal_writes::update_terminal)
                .delete(terminal_writes::delete_terminal))
```

- [ ] **Step 3: Register OpenAPI paths** (if the project lists paths explicitly)

Find the `#[derive(OpenApi)]` `paths(...)` list (grep `utoipa::OpenApi` / `paths(`). Add the five `terminal_writes::*` handlers and the terminal schemas to `components(schemas(...))`. If paths are auto-collected, skip.

- [ ] **Step 4: Add an integration test**

Add to `tests/terminals_pay_settlement_test.rs` (create it; copy the `test_server()` + dispatcher-login helper from `tests/integration_test.rs` — reuse the existing dispatcher auth helper if one exists, else replicate the login flow). Test: create terminal → list shows it → get returns it → update changes a rate → delete a non-default empty terminal returns 204; deleting the Default returns 409.

```rust
// pseudocode shape — fill in with the real dispatcher-auth header helper:
// let server = dispatch_server().await;
// let token = dispatch_login(&server).await;
// POST /dispatch/api/v1/terminals {name:"East",timezone:"America/New_York",loaded_rate_per_mile:0.6}
//   -> 201, body.id present
// GET  /dispatch/api/v1/terminals -> 200, contains "East" and seeded "Default"
// PUT  /dispatch/api/v1/terminals/{id} {loaded_rate_per_mile:0.7} -> 200, rate==0.7
// DELETE that terminal -> 204
// DELETE default terminal -> 409
```

- [ ] **Step 5: Run + Commit**

Run: `cargo test --test terminals_pay_settlement_test`
Expected: PASS.

```bash
git add src/api/dispatcher_portal/terminal_writes.rs src/api/dispatcher_portal/mod.rs tests/terminals_pay_settlement_test.rs
git commit -m "feat(terminals): dispatcher CRUD endpoints (#185)"
```

---

### Task 5: Dispatcher UI — Terminals view

**Files:**
- Modify: `static/dispatch/index.html` (add sidebar nav button `data-view="terminals"`)
- Modify: `static/dispatch/app.js` (add `terminals` to `VIEW_TITLES`, a `case 'terminals'` in `_renderView`, and `renderTerminalsView()` with list + create/edit modal)

**Reference:** mirror `renderLoadsView()` (list + create button) and the load create/edit modal in `app.js`. Use `escHtml()` on every interpolated field.

- [ ] **Step 1: Add nav entry**

In `index.html`, add a sidebar link button matching the existing ones:
```html
<button class="sidebar__link" data-view="terminals">Terminals</button>
```

- [ ] **Step 2: Add title + route**

In `app.js`, add `terminals: 'Terminals',` to `VIEW_TITLES`, and in `_renderView`'s switch:
```javascript
    case 'terminals':
      renderTerminalsView();
      break;
```

- [ ] **Step 3: Implement `renderTerminalsView()`**

```javascript
async function renderTerminalsView() {
  const res = await apiFetch(`${API_BASE}/terminals`);
  const terminals = res.ok ? await res.json() : [];
  const rows = terminals.map(t => `
    <tr data-terminal-id="${t.id}">
      <td>${escHtml(t.name)}${t.is_default ? ' <span class="badge">default</span>' : ''}</td>
      <td>${escHtml(t.timezone)}</td>
      <td>$${Number(t.loaded_rate_per_mile).toFixed(2)}</td>
      <td>$${Number(t.deadhead_rate_per_mile).toFixed(2)}</td>
      <td>$${Number(t.extra_stop_fee).toFixed(2)}</td>
      <td>$${Number(t.detention_rate_per_hour).toFixed(2)}/hr</td>
      <td>${t.free_dwell_minutes} min</td>
    </tr>`).join('');
  setContent(`
    <div class="list-view">
      <div class="list-view__header">
        <h2>Terminals</h2>
        <button class="btn btn--primary" id="create-terminal-btn">+ Create Terminal</button>
      </div>
      <table class="table">
        <thead><tr><th>Name</th><th>Timezone</th><th>Loaded $/mi</th>
          <th>Deadhead $/mi</th><th>Extra stop</th><th>Detention</th><th>Free dwell</th></tr></thead>
        <tbody id="terminals-tbody">${rows}</tbody>
      </table>
    </div>`);
  document.getElementById('create-terminal-btn')
    .addEventListener('click', () => showTerminalModal(null));
  document.querySelectorAll('#terminals-tbody tr[data-terminal-id]').forEach(row => {
    row.addEventListener('click', async () => {
      const r = await apiFetch(`${API_BASE}/terminals/${row.dataset.terminalId}`);
      if (r.ok) showTerminalModal(await r.json());
    });
  });
}
```

- [ ] **Step 4: Implement `showTerminalModal(terminal)`**

Create/edit in one modal. On save: `POST /terminals` when `terminal` is null, else `PUT /terminals/{id}`. Mirror the existing load modal's open/close/submit + error-alert pattern exactly (use the same modal helper functions the load modal uses — grep `showCreateLoadModal` / `showEditLoadModal`). Form fields: name (text, required), timezone (text, required, default `America/New_York`), is_default (checkbox), loaded_rate_per_mile / deadhead_rate_per_mile / extra_stop_fee / detention_rate_per_hour (number, step 0.01), free_dwell_minutes (number, default 120). On success, close modal and call `renderTerminalsView()`.

```javascript
function showTerminalModal(terminal) {
  const t = terminal || { name: '', timezone: 'America/New_York', is_default: false,
    loaded_rate_per_mile: 0, deadhead_rate_per_mile: 0, extra_stop_fee: 0,
    detention_rate_per_hour: 0, free_dwell_minutes: 120 };
  const isEdit = !!terminal;
  // Build modal markup mirroring showEditLoadModal(...) — reuse openModal()/closeModal() helpers.
  // On submit:
  //   const body = { name, timezone, is_default, loaded_rate_per_mile, ... , free_dwell_minutes };
  //   const url = isEdit ? `${API_BASE}/terminals/${t.id}` : `${API_BASE}/terminals`;
  //   const method = isEdit ? 'PUT' : 'POST';
  //   const res = await apiFetch(url, { method, body: JSON.stringify(body) });
  //   if (!res.ok) { showError(...); return; }
  //   closeModal(); renderTerminalsView();
}
```

- [ ] **Step 5: Manual smoke (no automated JS tests in this project)**

Confirm `cargo build` still serves static assets; visually verify via the running app is deferred to the sprint's verification step. Mark step done after code review of the modal wiring against the load modal.

- [ ] **Step 6: Commit**

```bash
git add static/dispatch/index.html static/dispatch/app.js
git commit -m "feat(terminals): dispatcher Terminals list + create/edit view (#185)"
```

---

## Phase B — Driver terminal_id + rate overrides (#185 + #132)

### Task 6: Driver schema migration — `terminal_id` + rate overrides + backfill

**Files:**
- Modify: `src/db/mod.rs` (`driver_schema` + `open_or_create_driver` migration + backfill)

**Reference:** `open_or_create_trip` migration block (mod.rs:176-215) and the existing driver migration for `current_truck_id` (CAST string).

- [ ] **Step 1: Extend `driver_schema()`**

Add fields to the driver Arrow schema:
```rust
        Field::new("terminal_id", DataType::Utf8, true), // nullable in storage; required at API
        Field::new("loaded_rate_per_mile", DataType::Float64, true),
        Field::new("deadhead_rate_per_mile", DataType::Float64, true),
        Field::new("extra_stop_fee", DataType::Float64, true),
        Field::new("detention_rate_per_hour", DataType::Float64, true),
        Field::new("free_dwell_minutes", DataType::Int64, true),
```

- [ ] **Step 2: Add migration CASTs in `open_or_create_driver`**

In the `Ok(table)` branch where existing columns are checked, append (⚠️ SQL keyword types only):
```rust
            if existing.field_with_name("terminal_id").is_err() {
                transforms.push(("terminal_id".into(), "CAST(NULL AS string)".into()));
            }
            for col in ["loaded_rate_per_mile", "deadhead_rate_per_mile",
                        "extra_stop_fee", "detention_rate_per_hour"] {
                if existing.field_with_name(col).is_err() {
                    transforms.push((col.into(), "CAST(NULL AS double)".into()));
                }
            }
            if existing.field_with_name("free_dwell_minutes").is_err() {
                transforms.push(("free_dwell_minutes".into(), "CAST(NULL AS bigint)".into()));
            }
```

- [ ] **Step 3: Backfill `terminal_id` to the Default terminal after migration**

After the `add_columns` call (and after the terminal table is guaranteed to exist — ensure `open_or_create_terminal` runs before `open_or_create_driver` in `DbClient::new`), backfill any driver rows with NULL `terminal_id`:
```rust
            // Backfill NULL terminal_id -> default terminal id.
            // Done in DbClient::new after both tables open (see Step 4), not here,
            // because we need the default terminal's id.
```
Move the backfill into `DbClient::new` (Step 4) since it needs the default terminal id.

- [ ] **Step 4: Implement backfill in `DbClient::new`**

After both `terminal_table` and `driver_table` are opened, add a helper call:
```rust
        let default_terminal_id = {
            // read default terminal id (mirror default_terminal() but pre-impl available)
            // simplest: call the already-defined method once self-fields are set is not possible
            // here, so query the terminal_table directly:
            // -- implement inline or call a small free fn that takes &Table --
        };
        // For each driver row with NULL terminal_id, set it to default_terminal_id via merge_insert.
```
Implementation: add a free function `backfill_driver_terminals(driver_table: &Table, default_terminal_id: Uuid) -> Result<(), AppError>` in `mod.rs` that scans all driver rows, and for any with null `terminal_id`, sets it and re-writes the row via `merge_insert(&["id"])`. ⚠️ Do NOT use `driver_table.update()` — the LanceDB `UpdateBuilder` is used NOWHERE in `src/db/` (no precedent, unverified API). Use the read-modify-upsert path: collect rows with null `terminal_id` into `DriverRecord`s (reuse `row_to_driver`), set `terminal_id`, and upsert each. This runs once on startup over a small table, so per-row upsert is fine.

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: compiles. (Full migration round-trip is tested in Task 16.)

- [ ] **Step 6: Commit**

```bash
git add src/db/mod.rs
git commit -m "feat(drivers): add terminal_id + rate-override columns with backfill (#185, #132)"
```

---

### Task 7: Driver model + ops + dispatcher driver write for rate overrides

**Files:**
- Modify: `src/models/driver.rs` (record/requests/list-item)
- Modify: `src/db/driver_ops.rs` (read/write new columns; `update_driver_terminal`)
- Modify: dispatcher driver create/patch path (grep for the driver create handler in `src/api/dispatcher_portal/`)

- [ ] **Step 1: Extend `DriverRecord`**

Add after `current_trailer_ids`:
```rust
    /// FK to terminals. Optional in the struct for backward-compat deserialization,
    /// but always populated after migration (backfilled to the default terminal).
    #[serde(default)]
    pub terminal_id: Option<Uuid>,
    #[serde(default)]
    pub loaded_rate_per_mile: Option<f64>,
    #[serde(default)]
    pub deadhead_rate_per_mile: Option<f64>,
    #[serde(default)]
    pub extra_stop_fee: Option<f64>,
    #[serde(default)]
    pub detention_rate_per_hour: Option<f64>,
    #[serde(default)]
    pub free_dwell_minutes: Option<u32>,
```
Add the same six fields (all `#[serde(default)] Option<...>`) to `CreateDriverRequest`, `UpdateDriverRequest`, and `DriverListItem`, and copy them in the `From<DriverRecord> for DriverListItem` impl.

- [ ] **Step 2: Read/write columns in `driver_ops.rs`**

In `driver_to_batch`, append the new columns to the `RecordBatch::try_new` value vector in schema order: `terminal_id` (StringArray of `r.terminal_id.map(|u| u.to_string())`), the four rate fields (Float64Array from `Option<f64>` — use `Float64Array::from(vec![r.loaded_rate_per_mile])` since `From<Vec<Option<f64>>>` is supported), and `free_dwell_minutes` (Int64Array from `r.free_dwell_minutes.map(|v| v as i64)`).
In `row_to_driver`, read them. ⚠️ `row_to_driver` currently defines ONLY `opt_str` (unlike `row_to_trip`, which has `opt_f64`). You must ADD both `opt_f64` and `opt_i64` closures to `row_to_driver` (copy `opt_f64` from `trip_ops.rs::row_to_trip` and write the `Int64Array` analog for `opt_i64`):

```rust
    let opt_f64 = |name: &str| -> Option<f64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };
    let opt_i64 = |name: &str| -> Option<i64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };
```

then:
```rust
    terminal_id: opt_str("terminal_id").map(|s| s.parse()).transpose()
        .map_err(|e| AppError::Internal(format!("{e}")))?,
    loaded_rate_per_mile: opt_f64("loaded_rate_per_mile"),
    deadhead_rate_per_mile: opt_f64("deadhead_rate_per_mile"),
    extra_stop_fee: opt_f64("extra_stop_fee"),
    detention_rate_per_hour: opt_f64("detention_rate_per_hour"),
    free_dwell_minutes: opt_i64("free_dwell_minutes").map(|v| v as u32),
```

- [ ] **Step 3: `update_driver_terminal` + rate overrides through create/patch**

Add `pub async fn update_driver_terminal(&self, id: Uuid, terminal_id: Uuid) -> Result<DriverRecord, AppError>` (fetch → set → upsert). In the dispatcher driver create handler, default `terminal_id` to `state.db.default_terminal().await?.id` when the request omits it, and pass through any rate-override fields. In the driver patch handler, apply the optional rate-override fields and `terminal_id` if present (validate the terminal exists via `get_terminal_by_id`).

- [ ] **Step 4: Unit test round-trip of driver rate overrides**

Add to `src/db/driver_ops.rs` tests (or the integration test): insert a driver with `loaded_rate_per_mile: Some(0.62)`, `terminal_id: Some(<default>)`, others None; get it back; assert the values round-trip and the None fields are None.

- [ ] **Step 5: Run + Commit**

Run: `cargo test --lib driver_ops`
Expected: PASS.

```bash
git add src/models/driver.rs src/db/driver_ops.rs src/api/dispatcher_portal/
git commit -m "feat(drivers): terminal_id + optional rate overrides on record/API (#185, #132)"
```

---

## Phase C — Trip rate overrides + DriverPay (#132)

### Task 8: Trip schema migration — rate overrides

**Files:**
- Modify: `src/db/mod.rs` (`trip_schema` + `open_or_create_trip` migration)
- Modify: `src/models/trip.rs` (`TripRecord` + `TripListItem` override fields)
- Modify: `src/db/trip_ops.rs` (read/write the columns)

- [ ] **Step 1: Extend `trip_schema()`** with five nullable columns:
```rust
        Field::new("loaded_rate_per_mile", DataType::Float64, true),
        Field::new("deadhead_rate_per_mile", DataType::Float64, true),
        Field::new("extra_stop_fee", DataType::Float64, true),
        Field::new("detention_rate_per_hour", DataType::Float64, true),
        Field::new("free_dwell_minutes", DataType::Int64, true),
```

- [ ] **Step 2: Migration CASTs in `open_or_create_trip`** (SQL keywords only):
```rust
            for col in ["loaded_rate_per_mile", "deadhead_rate_per_mile",
                        "extra_stop_fee", "detention_rate_per_hour"] {
                if existing.field_with_name(col).is_err() {
                    transforms.push((col.into(), "CAST(NULL AS double)".into()));
                }
            }
            if existing.field_with_name("free_dwell_minutes").is_err() {
                transforms.push(("free_dwell_minutes".into(), "CAST(NULL AS bigint)".into()));
            }
```

- [ ] **Step 3: Add the five `Option<...>` fields** to `TripRecord` and `TripListItem` (all `#[serde(default)]`), and copy them in `From<TripRecord> for TripListItem`. Add to `trip_to_batch` / `row_to_trip` in `trip_ops.rs` exactly as in Task 7 Step 2.

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src/db/mod.rs src/models/trip.rs src/db/trip_ops.rs
git commit -m "feat(trips): add optional rate-override columns (#132)"
```

---

### Task 9: `RateSchedule` + pure `resolve_rates()`

**Files:**
- Create: `src/models/pay.rs` (this task adds `RateSchedule` + `resolve_rates`; Task 10 adds `DriverPay` + `compute_driver_pay` to the same file)
- Modify: `src/models/mod.rs` (`pub mod pay;` + re-exports)

- [ ] **Step 1: Write the failing test**

```rust
// in src/models/pay.rs
#[cfg(test)]
mod tests {
    use super::*;

    fn terminal_floor() -> TerminalRates {
        TerminalRates { loaded_rate_per_mile: 0.50, deadhead_rate_per_mile: 0.40,
            extra_stop_fee: 30.0, detention_rate_per_hour: 20.0, free_dwell_minutes: 120 }
    }

    #[test]
    fn resolves_terminal_floor_when_no_overrides() {
        let r = resolve_rates(&RateOverrides::default(), &RateOverrides::default(), &terminal_floor());
        assert_eq!(r.loaded_rate_per_mile, 0.50);
        assert_eq!(r.free_dwell_minutes, 120);
    }

    #[test]
    fn driver_overrides_terminal_per_field() {
        let driver = RateOverrides { loaded_rate_per_mile: Some(0.60), ..Default::default() };
        let r = resolve_rates(&RateOverrides::default(), &driver, &terminal_floor());
        assert_eq!(r.loaded_rate_per_mile, 0.60);   // from driver
        assert_eq!(r.deadhead_rate_per_mile, 0.40); // still terminal
    }

    #[test]
    fn trip_overrides_driver_and_terminal() {
        let driver = RateOverrides { loaded_rate_per_mile: Some(0.60), detention_rate_per_hour: Some(25.0), ..Default::default() };
        let trip = RateOverrides { loaded_rate_per_mile: Some(0.75), ..Default::default() };
        let r = resolve_rates(&trip, &driver, &terminal_floor());
        assert_eq!(r.loaded_rate_per_mile, 0.75);    // trip wins
        assert_eq!(r.detention_rate_per_hour, 25.0); // driver (no trip override)
        assert_eq!(r.extra_stop_fee, 30.0);          // terminal
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib pay::tests`
Expected: FAIL (types/functions undefined).

- [ ] **Step 3: Implement `RateOverrides`, `TerminalRates`, `RateSchedule`, `resolve_rates`**

```rust
// src/models/pay.rs  (top of file)
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The optional per-field overrides carried by a trip or a driver.
#[derive(Debug, Clone, Default)]
pub struct RateOverrides {
    pub loaded_rate_per_mile: Option<f64>,
    pub deadhead_rate_per_mile: Option<f64>,
    pub extra_stop_fee: Option<f64>,
    pub detention_rate_per_hour: Option<f64>,
    pub free_dwell_minutes: Option<u32>,
}

/// The mandatory terminal floor (all concrete).
#[derive(Debug, Clone)]
pub struct TerminalRates {
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub extra_stop_fee: f64,
    pub detention_rate_per_hour: f64,
    pub free_dwell_minutes: u32,
}

/// Fully resolved, concrete rates used for computation.
#[derive(Debug, Clone, PartialEq)]
pub struct RateSchedule {
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub extra_stop_fee: f64,
    pub detention_rate_per_hour: f64,
    pub free_dwell_minutes: u32,
}

/// Per-field resolution: trip override ?? driver override ?? terminal floor.
pub fn resolve_rates(trip: &RateOverrides, driver: &RateOverrides, terminal: &TerminalRates)
    -> RateSchedule
{
    RateSchedule {
        loaded_rate_per_mile: trip.loaded_rate_per_mile
            .or(driver.loaded_rate_per_mile).unwrap_or(terminal.loaded_rate_per_mile),
        deadhead_rate_per_mile: trip.deadhead_rate_per_mile
            .or(driver.deadhead_rate_per_mile).unwrap_or(terminal.deadhead_rate_per_mile),
        extra_stop_fee: trip.extra_stop_fee
            .or(driver.extra_stop_fee).unwrap_or(terminal.extra_stop_fee),
        detention_rate_per_hour: trip.detention_rate_per_hour
            .or(driver.detention_rate_per_hour).unwrap_or(terminal.detention_rate_per_hour),
        free_dwell_minutes: trip.free_dwell_minutes
            .or(driver.free_dwell_minutes).unwrap_or(terminal.free_dwell_minutes),
    }
}
```

Add `pub mod pay;` to `src/models/mod.rs` and re-export `pay::{DriverPay, RateSchedule}` (DriverPay added next task).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib pay::tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/models/pay.rs src/models/mod.rs
git commit -m "feat(pay): RateSchedule + per-field resolve_rates (#132)"
```

---

### Task 10: `DriverPay` + pure `compute_driver_pay()` (incl. detention)

**Files:**
- Modify: `src/models/pay.rs`

Detention per stop uses the **most-specific** free-dwell: `stop.detention_free_minutes ?? resolved.free_dwell_minutes`. Dwell is computed from UTC-normalized arrive/depart (use the already-derived `actual_arrive_utc`/`actual_depart_utc`, or normalize via `crate::models::load::naive_local_to_utc`). Stops missing either time contribute 0.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod pay_tests {
    use super::*;

    fn sched() -> RateSchedule {
        RateSchedule { loaded_rate_per_mile: 0.50, deadhead_rate_per_mile: 0.40,
            extra_stop_fee: 30.0, detention_rate_per_hour: 20.0, free_dwell_minutes: 120 }
    }

    // A minimal stop input the computation accepts (see PayStopInput below).
    fn stop(free: Option<u32>, arrive: Option<&str>, depart: Option<&str>) -> PayStopInput {
        PayStopInput {
            detention_free_minutes: free,
            actual_arrive_utc: arrive.map(|s| s.to_string()),
            actual_depart_utc: depart.map(|s| s.to_string()),
        }
    }

    #[test]
    fn loaded_deadhead_and_extra_stops() {
        // 100 loaded mi, 20 deadhead mi, 4 stops (2 extra), no dwell -> no detention
        let pay = compute_driver_pay(Some(100.0), Some(20.0),
            &[stop(None,None,None), stop(None,None,None), stop(None,None,None), stop(None,None,None)],
            &sched());
        assert_eq!(pay.loaded_pay, 50.0);
        assert_eq!(pay.deadhead_pay, 8.0);
        assert_eq!(pay.extra_stop_pay, 60.0); // 30 * (4-2)
        assert_eq!(pay.detention_pay, 0.0);
        assert_eq!(pay.total_pay, 118.0);
    }

    #[test]
    fn two_or_fewer_stops_no_extra_stop_pay() {
        let pay = compute_driver_pay(Some(10.0), None, &[stop(None,None,None), stop(None,None,None)], &sched());
        assert_eq!(pay.deadhead_pay, 0.0); // deadhead miles None
        assert_eq!(pay.extra_stop_pay, 0.0);
    }

    #[test]
    fn detention_beyond_free_dwell() {
        // arrive 12:00Z depart 15:00Z = 180 min; free 120 -> 60 min over = 1.0h * 20 = 20.0
        let pay = compute_driver_pay(Some(0.0), None,
            &[stop(None, Some("2026-05-30T12:00:00+00:00"), Some("2026-05-30T15:00:00+00:00"))],
            &sched());
        assert_eq!(pay.detention_pay, 20.0);
    }

    #[test]
    fn per_stop_free_dwell_override_wins() {
        // 180 min dwell, stop overrides free to 180 -> 0 detention
        let pay = compute_driver_pay(Some(0.0), None,
            &[stop(Some(180), Some("2026-05-30T12:00:00+00:00"), Some("2026-05-30T15:00:00+00:00"))],
            &sched());
        assert_eq!(pay.detention_pay, 0.0);
    }

    #[test]
    fn missing_times_contribute_zero_detention() {
        let pay = compute_driver_pay(Some(0.0), None,
            &[stop(None, Some("2026-05-30T12:00:00+00:00"), None)], &sched());
        assert_eq!(pay.detention_pay, 0.0);
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `cargo test --lib pay`
Expected: FAIL (DriverPay / compute_driver_pay / PayStopInput undefined).

- [ ] **Step 3: Implement**

```rust
/// Minimal per-stop input for pay computation (decouples pay from TripStop).
#[derive(Debug, Clone)]
pub struct PayStopInput {
    pub detention_free_minutes: Option<u32>,
    /// RFC3339 UTC timestamps (use TripStop::actual_arrive_utc/actual_depart_utc).
    pub actual_arrive_utc: Option<String>,
    pub actual_depart_utc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct DriverPay {
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub loaded_pay: f64,
    pub deadhead_pay: f64,
    pub extra_stop_pay: f64,
    pub detention_pay: f64,
    pub total_pay: f64,
}

fn round2(x: f64) -> f64 { (x * 100.0).round() / 100.0 }

pub fn compute_driver_pay(
    loaded_miles: Option<f64>,
    deadhead_miles: Option<f64>,
    stops: &[PayStopInput],
    rates: &RateSchedule,
) -> DriverPay {
    let loaded_pay = loaded_miles.unwrap_or(0.0) * rates.loaded_rate_per_mile;
    let deadhead_pay = deadhead_miles.unwrap_or(0.0) * rates.deadhead_rate_per_mile;
    let extra_stops = (stops.len() as i64 - 2).max(0) as f64;
    let extra_stop_pay = extra_stops * rates.extra_stop_fee;

    let mut detention_pay = 0.0;
    for s in stops {
        let (Some(a), Some(d)) = (s.actual_arrive_utc.as_deref(), s.actual_depart_utc.as_deref())
            else { continue };
        let (Ok(at), Ok(dt)) = (
            chrono::DateTime::parse_from_rfc3339(a),
            chrono::DateTime::parse_from_rfc3339(d),
        ) else { continue };
        let dwell_min = (dt - at).num_minutes();
        if dwell_min <= 0 { continue; }
        let free = s.detention_free_minutes.unwrap_or(rates.free_dwell_minutes) as i64;
        let over_min = (dwell_min - free).max(0) as f64;
        detention_pay += (over_min / 60.0) * rates.detention_rate_per_hour;
    }

    let loaded_pay = round2(loaded_pay);
    let deadhead_pay = round2(deadhead_pay);
    let extra_stop_pay = round2(extra_stop_pay);
    let detention_pay = round2(detention_pay);
    DriverPay {
        loaded_rate_per_mile: rates.loaded_rate_per_mile,
        deadhead_rate_per_mile: rates.deadhead_rate_per_mile,
        loaded_pay, deadhead_pay, extra_stop_pay, detention_pay,
        total_pay: round2(loaded_pay + deadhead_pay + extra_stop_pay + detention_pay),
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib pay`
Expected: PASS (all pay + resolve tests).

- [ ] **Step 5: Commit**

```bash
git add src/models/pay.rs src/models/mod.rs
git commit -m "feat(pay): DriverPay struct + compute_driver_pay with detention (#132)"
```

---

### Task 11: Compute `driver_pay` on read — DISPATCHER portal

**⚠️ Retarget note (from plan review):** The dispatcher trip GET/list does NOT use `src/api/trips.rs` (that's the deprecated admin `/api/v1` path, per Constraint #5). The dispatcher detail is built by `build_trip_detail()` in `src/api/dispatcher_portal/data.rs`, which returns a **`DispatcherTripListItem`** (a wrapper carrying `mileage_summary`, enriched driver/truck names, and flattened miles — NOT a bare `TripRecord`/`TripListItem`). Attach `driver_pay` there.

**Files:**
- Modify: `src/api/dispatcher_portal/data.rs` (`DispatcherTripListItem` struct, `build_trip_detail`, the list builder) + add a shared `driver_pay_for_record` helper.

- [ ] **Step 1: Add `driver_pay` to `DispatcherTripListItem`**

In `src/api/dispatcher_portal/data.rs`, add to the `DispatcherTripListItem` struct (near the flattened mileage fields):
```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_pay: Option<crate::models::pay::DriverPay>,
```
Set `driver_pay: None` wherever `DispatcherTripListItem` is constructed (e.g. in `enrich_trip`).

- [ ] **Step 2: Add the shared `driver_pay_for_record` helper in `data.rs`**

This is the single source of truth for pay-on-read. Snapshot wins (freeze); else live-compute. Resolves driver overrides + terminal floor.

```rust
/// Computes driver pay for a trip on read. Returns the frozen snapshot if the
/// trip is settled; otherwise resolves rates (trip ?? driver ?? terminal floor)
/// and computes live. None when there are no loaded miles to pay on.
pub async fn driver_pay_for_record(
    state: &AppState,
    record: &crate::models::TripRecord,
) -> Option<crate::models::pay::DriverPay> {
    use crate::models::pay::*;
    if let Some(snap) = &record.driver_pay_snapshot {
        return Some(snap.clone());
    }
    record.loaded_miles?; // no loaded miles -> no pay
    // Driver overrides + terminal floor.
    let driver = match record.driver_id {
        Some(did) => state.db.get_driver_by_id(did).await.ok(),
        None => None,
    };
    let terminal = {
        let tid = driver.as_ref().and_then(|d| d.terminal_id);
        let by_id = match tid { Some(t) => state.db.get_terminal_by_id(t).await.ok(), None => None };
        match by_id { Some(t) => t, None => state.db.default_terminal().await.ok()? }
    };
    let driver_ov = driver.as_ref().map(|d| RateOverrides {
        loaded_rate_per_mile: d.loaded_rate_per_mile,
        deadhead_rate_per_mile: d.deadhead_rate_per_mile,
        extra_stop_fee: d.extra_stop_fee,
        detention_rate_per_hour: d.detention_rate_per_hour,
        free_dwell_minutes: d.free_dwell_minutes,
    }).unwrap_or_default();
    let trip_ov = RateOverrides {
        loaded_rate_per_mile: record.loaded_rate_per_mile,
        deadhead_rate_per_mile: record.deadhead_rate_per_mile,
        extra_stop_fee: record.extra_stop_fee,
        detention_rate_per_hour: record.detention_rate_per_hour,
        free_dwell_minutes: record.free_dwell_minutes,
    };
    let floor = TerminalRates {
        loaded_rate_per_mile: terminal.loaded_rate_per_mile,
        deadhead_rate_per_mile: terminal.deadhead_rate_per_mile,
        extra_stop_fee: terminal.extra_stop_fee,
        detention_rate_per_hour: terminal.detention_rate_per_hour,
        free_dwell_minutes: terminal.free_dwell_minutes,
    };
    let rates = resolve_rates(&trip_ov, &driver_ov, &floor);
    // Build PayStopInput from stops — derive UTC times via fill_utc_fields semantics.
    let stops: Vec<PayStopInput> = record.stops.iter().map(|s| {
        let mut s2 = s.clone();
        s2.fill_utc_fields();
        PayStopInput {
            detention_free_minutes: s2.detention_free_minutes,
            actual_arrive_utc: s2.actual_arrive_utc,
            actual_depart_utc: s2.actual_depart_utc,
        }
    }).collect();
    Some(compute_driver_pay(record.loaded_miles, record.deadhead_miles, &stops, &rates))
}
```

- [ ] **Step 3: Wire into `build_trip_detail` (detail GET)**

In `build_trip_detail`, after `enriched` is assembled and before the `Ok(enriched)`, add:
```rust
    enriched.driver_pay = driver_pay_for_record(state, &record).await;
```
(`record` is already in scope in `build_trip_detail`.)

- [ ] **Step 4: Wire into the dispatcher list builder**

In `data::list_trips`, after the per-trip `DispatcherTripListItem`s are built, populate `driver_pay` for each. The list already `batch_get_drivers`; to keep it simple and correct, call `driver_pay_for_record(state, &record).await` per trip (N+1, acceptable at this app's fleet scale and only for filtered lists). If `list_trips` works from `TripListItem`s rather than full `TripRecord`s, fetch the record via `state.db.get_trip(id)` only when needed, OR skip live compute in the list and set `driver_pay` from the snapshot only (frozen trips show pay in lists; unsettled show it on detail). **Decision: list shows snapshot-only** to avoid N+1 in list views; detail always computes live. Add a one-line `// list shows frozen pay only; live pay is on the detail endpoint` comment so the cap is explicit.

- [ ] **Step 5: Integration test**

In `tests/terminals_pay_settlement_test.rs`: PUT Default terminal rates (e.g. loaded 0.50, deadhead 0.40, extra-stop 30, detention 20), create a driver on the default terminal, create a trip (with stops; obtain its `loaded_miles`/`deadhead_miles` from the GET response since miles are ORS-computed), GET the trip detail, assert `driver_pay.loaded_pay == loaded_miles * 0.50` (compute expected from the returned miles) and `driver_pay.total_pay` equals the sum. Then PUT a driver `loaded_rate_per_mile` override and assert the detail GET reflects it.

- [ ] **Step 6: Run + Commit**

Run: `cargo test --test terminals_pay_settlement_test`
Expected: PASS.

```bash
git add src/api/dispatcher_portal/data.rs tests/terminals_pay_settlement_test.rs
git commit -m "feat(trips): compute driver_pay on dispatcher trip read with tiered resolution (#132)"
```

---

## Phase D — Settlement + freeze (#134)

### Task 12: Trip settlement fields + snapshot column

**Files:**
- Modify: `src/db/mod.rs` (`trip_schema` + migration: `settlement_ref`, `pay_period_start`, `pay_period_end`, `driver_pay_snapshot`)
- Modify: `src/models/trip.rs` (`TripRecord` + `TripListItem`)
- Modify: `src/db/trip_ops.rs` (read/write)

`driver_pay_snapshot` is stored as a JSON string column (like `stops`/`trailer_ids`), deserialized to `Option<DriverPay>`.

- [ ] **Step 1: Schema + migration**

`trip_schema()` add:
```rust
        Field::new("settlement_ref", DataType::Utf8, true),
        Field::new("pay_period_start", DataType::Utf8, true),
        Field::new("pay_period_end", DataType::Utf8, true),
        Field::new("driver_pay_snapshot", DataType::Utf8, true),
```
Migration CASTs (all `string`):
```rust
            for col in ["settlement_ref", "pay_period_start", "pay_period_end", "driver_pay_snapshot"] {
                if existing.field_with_name(col).is_err() {
                    transforms.push((col.into(), "CAST(NULL AS string)".into()));
                }
            }
```

- [ ] **Step 2: Model fields**

On `TripRecord` and `TripListItem`:
```rust
    #[serde(default)]
    pub settlement_ref: Option<String>,
    #[serde(default)]
    pub pay_period_start: Option<String>,
    #[serde(default)]
    pub pay_period_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_pay_snapshot: Option<crate::models::pay::DriverPay>,
```
Copy in `From<TripRecord>`.

- [ ] **Step 3: Read/write in `trip_ops.rs`**

In `trip_to_batch`: `settlement_ref`/`pay_period_start`/`pay_period_end` as StringArray of the Options; `driver_pay_snapshot` as `record.driver_pay_snapshot.as_ref().map(|p| serde_json::to_string(p).unwrap_or_default())` (StringArray Option). In `row_to_trip`: `opt_str` for the three strings, and `opt_str("driver_pay_snapshot").and_then(|s| serde_json::from_str(&s).ok())` for the snapshot.

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src/db/mod.rs src/models/trip.rs src/db/trip_ops.rs
git commit -m "feat(trips): settlement fields + driver_pay_snapshot column (#134)"
```

---

### Task 13: Settlement PATCH + freeze snapshot + edit-lock (DISPATCHER portal)

**⚠️ Retarget note (from plan review):** The dispatcher trip PATCH is `apply_trip_patch()` in `src/api/dispatcher_portal/trip_writes.rs` (HTTP `patch_trip_handler` + the MCP `update_trip` tool both call it). It deserializes a `PatchTripBody` (with `#[serde(deny_unknown_fields)]`) and already rejects raw mileage fields. The dispatcher arrive/depart handlers are `data::stop_arrive`/`data::stop_depart` in `src/api/dispatcher_portal/data.rs` (NOT `trip_actions::record_arrive` — no such fn; the admin ones are `trip_actions::stop_arrive`/`stop_depart`).

**Files:**
- Modify: `src/api/dispatcher_portal/trip_writes.rs` (`PatchTripBody` + `apply_trip_patch`)
- Modify: `src/api/dispatcher_portal/data.rs` (`stop_arrive`/`stop_depart` edit-lock)
- Modify: `src/db/trip_ops.rs` (`update_trip_settlement`, `update_trip_rate_overrides`)

**Freeze rules:**
- When `settlement_ref` transitions `None → Some`, compute the current `DriverPay` via `driver_pay_for_record` (Task 11) on the live record BEFORE persisting the snapshot, and store it in `driver_pay_snapshot` in the same update.
- While `settlement_ref.is_some()`: reject PATCHes that change trip rate overrides; reject `stop_arrive`/`stop_depart`. Return `AppError::Conflict(...)` (→ 409).

- [ ] **Step 1: Extend `PatchTripBody`**

In `trip_writes.rs`, add `#[serde(default)]` optional fields to `PatchTripBody`: `settlement_ref: Option<String>`, `pay_period_start: Option<String>`, `pay_period_end: Option<String>`, and the five rate overrides (`loaded_rate_per_mile: Option<f64>`, `deadhead_rate_per_mile`, `extra_stop_fee`, `detention_rate_per_hour`, `free_dwell_minutes: Option<u32>`). `deny_unknown_fields` stays — these are now known fields.

- [ ] **Step 2: Add ops methods to `trip_ops.rs`**

```rust
pub async fn update_trip_rate_overrides(
    &self, id: Uuid,
    loaded: Option<f64>, deadhead: Option<f64>, extra_stop: Option<f64>,
    detention: Option<f64>, free_dwell: Option<u32>,
) -> Result<TripRecord, AppError> {
    let mut t = self.get_trip(id).await?;
    if loaded.is_some() { t.loaded_rate_per_mile = loaded; }
    if deadhead.is_some() { t.deadhead_rate_per_mile = deadhead; }
    if extra_stop.is_some() { t.extra_stop_fee = extra_stop; }
    if detention.is_some() { t.detention_rate_per_hour = detention; }
    if free_dwell.is_some() { t.free_dwell_minutes = free_dwell; }
    t.updated_at = Utc::now();
    self.upsert_trip(&t).await?;
    Ok(t)
}

pub async fn update_trip_settlement(
    &self, id: Uuid,
    settlement_ref: Option<String>,
    pay_period_start: Option<String>,
    pay_period_end: Option<String>,
    snapshot: Option<crate::models::pay::DriverPay>,
) -> Result<TripRecord, AppError> {
    let mut t = self.get_trip(id).await?;
    if settlement_ref.is_some() { t.settlement_ref = settlement_ref; }
    if pay_period_start.is_some() { t.pay_period_start = pay_period_start; }
    if pay_period_end.is_some() { t.pay_period_end = pay_period_end; }
    if snapshot.is_some() { t.driver_pay_snapshot = snapshot; }
    t.updated_at = Utc::now();
    self.upsert_trip(&t).await?;
    Ok(t)
}
```

- [ ] **Step 3: Freeze + lock logic in `apply_trip_patch`**

After `parsed` is built (and the existing notes/previous_trip_id handling), add this block. Fetch the existing record once at the top of `apply_trip_patch` (it currently doesn't — add `let existing = state.db.get_trip(id).await?;` near the start, reused below):

```rust
    let was_settled = existing.settlement_ref.is_some();

    let touches_rate = parsed.loaded_rate_per_mile.is_some()
        || parsed.deadhead_rate_per_mile.is_some()
        || parsed.extra_stop_fee.is_some()
        || parsed.detention_rate_per_hour.is_some()
        || parsed.free_dwell_minutes.is_some();

    if was_settled && touches_rate {
        return Err(AppError::Conflict(
            "trip is settled; pay-affecting fields are frozen".into()));
    }

    // Apply trip rate overrides (allowed only when not settled).
    if touches_rate {
        state.db.update_trip_rate_overrides(id,
            parsed.loaded_rate_per_mile, parsed.deadhead_rate_per_mile,
            parsed.extra_stop_fee, parsed.detention_rate_per_hour,
            parsed.free_dwell_minutes).await?;
    }

    // Settlement transition None -> Some: compute snapshot from the LIVE record, then persist.
    if !was_settled && parsed.settlement_ref.is_some() {
        // Re-fetch to include any rate overrides just written above.
        let live = state.db.get_trip(id).await?;
        let snapshot = crate::api::dispatcher_portal::data::driver_pay_for_record(state, &live).await;
        state.db.update_trip_settlement(id,
            parsed.settlement_ref.clone(), parsed.pay_period_start.clone(),
            parsed.pay_period_end.clone(), snapshot).await?;
    } else if parsed.pay_period_start.is_some() || parsed.pay_period_end.is_some() {
        // Updating pay-period metadata is allowed; does not re-freeze or un-settle.
        state.db.update_trip_settlement(id, None,
            parsed.pay_period_start.clone(), parsed.pay_period_end.clone(), None).await?;
    }
```
Keep the existing best-effort mileage recompute and `build_trip_detail(state, id)` return at the end — the returned detail will now carry the (possibly frozen) `driver_pay`.

- [ ] **Step 4: Edit-lock in `data::stop_arrive` / `data::stop_depart`**

In each of `data::stop_arrive` and `data::stop_depart`, after the trip is loaded (or add a fetch), before persisting the time:
```rust
    let trip = state.db.get_trip(id).await?;
    if trip.settlement_ref.is_some() {
        return Err(AppError::Conflict("trip is settled; stop times are frozen".into()));
    }
```
Reuse an existing fetch if the handler already loads the trip. If these handlers delegate to `trip_actions::stop_arrive`/`stop_depart`, add the guard in the dispatcher wrapper (before delegating) so the admin path is unaffected.

- [ ] **Step 5: Tests**

In `tests/terminals_pay_settlement_test.rs`:
1. Create trip with miles+stops, set terminal rates, GET → record `driver_pay.total_pay` = `T`.
2. PATCH `settlement_ref = "S-2026-009"`, `pay_period_start/end`. GET → `driver_pay_snapshot` present and `driver_pay == snapshot` and `total_pay == T`.
3. PUT a new (higher) Default terminal `loaded_rate_per_mile`. GET the settled trip → `driver_pay.total_pay` STILL `T` (frozen).
4. PATCH the settled trip's `loaded_rate_per_mile` override → expect 409.
5. POST the dispatcher `stop_arrive` on the settled trip → expect 409.
6. GET `/dispatch/api/v1/trips?pay_period_start=...&pay_period_end=...` returns the trip (Task 14).

- [ ] **Step 6: Run + Commit**

Run: `cargo test --test terminals_pay_settlement_test`
Expected: PASS.

```bash
git add src/api/dispatcher_portal/trip_writes.rs src/api/dispatcher_portal/data.rs src/db/trip_ops.rs
git commit -m "feat(trips): settlement PATCH freezes driver_pay + locks pay edits (#134)"
```

---

### Task 14: `GET /dispatch/api/v1/trips` pay-period range filter

**Files:**
- Modify: `src/api/dispatcher_portal/data.rs` (the dispatcher `list_trips` query struct + handler)
- Modify: `src/db/trip_ops.rs` (`list_trips` accepts pay-period bounds)

Filter semantics (from #134): trips where `pay_period_start >= X AND pay_period_end <= Y`.

- [ ] **Step 1: Add query params**

Add `pay_period_start: Option<String>` and `pay_period_end: Option<String>` to the dispatcher `list_trips` query struct (find the `Query<...>` extractor type used by `data::list_trips`).

- [ ] **Step 2: Push the filter into `db::list_trips`**

Extend the `db::list_trips` signature with the two optional bounds and add `.only_if(...)` string conditions when present (mirror the existing `status`/`load_id`/`driver_id` filtering — note its current signature is `(load_id, driver_id, status)`, so add the two bounds and update ALL callers, including the admin `src/api/trips.rs::list_trips`, to pass `None, None`). Build conditions like `pay_period_start >= '{x}'` and `pay_period_end <= '{y}'` joined with ` AND ` (ISO date strings compare lexicographically).

- [ ] **Step 3: Test**

Covered by Task 13 Step 5 item 6 — assert the settled trip is returned for a range that includes it and excluded for a range that doesn't.

- [ ] **Step 4: Run + Commit**

Run: `cargo test --test terminals_pay_settlement_test`
Expected: PASS.

```bash
git add src/api/dispatcher_portal/data.rs src/api/trips.rs src/db/trip_ops.rs
git commit -m "feat(trips): filter dispatcher GET /trips by pay_period range (#134)"
```

---

## Phase E — Config retirement + migration test

### Task 15: Retire `Config.terminal_timezone`; free-dwell from terminal

**Files:**
- Modify: `src/config.rs` (remove `terminal_timezone` field + its parse + tests; keep `free_dwell_minutes` parse ONLY if still referenced by the terminal seed — the seed reads the env directly in Task 2, so `free_dwell_minutes` can also be removed from `Config` unless other code uses it)
- Modify: `src/api/driver_portal/data.rs:657` (read free-dwell from the driver's terminal instead of `config.free_dwell_minutes`)
- Modify: `AGENTS.md` (mark `TERMINAL_TIMEZONE` deprecated, document terminals)

**Reference:** the only non-test reader of `config.free_dwell_minutes` is `data.rs:657`; `config.terminal_timezone` readers — grep `terminal_timezone` across `src/` to find all (the explorer found it only in config + tests, but re-grep to be sure).

- [ ] **Step 1: Re-grep all readers**

Run: `rg -n "terminal_timezone|free_dwell_minutes" src/`
Confirm the full set of call sites before editing. Update this task's edits to cover every hit.

- [ ] **Step 2: Replace `data.rs` free-dwell source**

At `data.rs:657`, instead of `state.config.free_dwell_minutes`, resolve from the trip's driver's terminal: fetch the driver (the handler already has driver/trip context — use it), then the terminal, and use `terminal.free_dwell_minutes` (fall back to `default_terminal()` if unset). If that handler doesn't already have the driver id conveniently, use `state.db.default_terminal().await?.free_dwell_minutes` as the value (acceptable: this is a display default in the stop response; the authoritative per-stop free-dwell still lives in `detention_free_minutes` and pay resolution). Keep the change minimal and documented with a comment.

- [ ] **Step 3: Remove `terminal_timezone` from `Config`**

Delete the field, its `from_env` parse + validation, and the two unit tests (`test_terminal_timezone_default`, `test_terminal_timezone_invalid_rejects`). The `TERMINAL_TIMEZONE` env is still read directly by `open_or_create_terminal` (Task 2) for seeding; document that in a comment. Remove `free_dwell_minutes` from `Config` only if Step 1 shows no remaining readers after Step 2; otherwise keep it.

- [ ] **Step 4: Update `AGENTS.md`**

Add a short "Terminals & driver pay" note: terminals own timezone + rate floor; `TERMINAL_TIMEZONE`/`OLLIE_FREE_DWELL_MINUTES` env vars are now only seed values for the Default terminal on first boot and are otherwise deprecated.

- [ ] **Step 5: Build + full test**

Run: `cargo build && cargo test`
Expected: compiles; all tests pass (the removed config tests are gone; nothing else should reference the removed field).

- [ ] **Step 6: Commit**

```bash
git add src/config.rs src/api/driver_portal/data.rs AGENTS.md
git commit -m "refactor(config): retire terminal_timezone; free-dwell sourced from terminal (#185)"
```

---

### Task 16: Existing-DB migration integration test

**Files:**
- Modify: `tests/terminals_pay_settlement_test.rs` (or add to `tests/migration_test.rs` following its `seed_pre_*` pattern)

This is the **mandatory** guard for the CAST-null trap: seed a pre-sprint drivers + trips DB (old schemas without the new columns), then open via `DbClient::new` and assert the migration adds columns, backfills `terminal_id`, and pay computes.

**Reference:** `tests/migration_test.rs` `seed_pre_equipment_drivers` + `migration_opens_pre_equipment_drivers_table_and_adds_new_columns`.

- [ ] **Step 1: Write old-schema seeders**

Define `driver_schema_pre_sprint(embed_dim)` and `trip_schema_pre_sprint(embed_dim)` = current shipped schemas WITHOUT the columns this sprint adds. Seed one driver row and one trip row (with `loaded_miles` set, a couple of stops) into a TempDir DB using `create_table` directly (mirror `seed_pre_v16_db`).

- [ ] **Step 2: Write the migration test**

```rust
#[tokio::test]
async fn migrates_pre_sprint_db_and_computes_pay() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_str().unwrap();
    seed_pre_sprint_db(path).await;

    // Opening must migrate without crash-looping (the CAST-null trap).
    let client = DbClient::new(path, EMBED_DIM).await
        .expect("must migrate pre-sprint drivers+trips without erroring");

    // terminals table created + Default seeded
    let def = client.default_terminal().await.unwrap();
    assert_eq!(def.name, "Default");

    // driver backfilled to default terminal
    let (_n, drivers) = client.list_drivers(None, 10, 0).await.unwrap();
    let d = client.get_driver_by_id(drivers[0].id).await.unwrap();
    assert_eq!(d.terminal_id, Some(def.id));
    assert_eq!(d.loaded_rate_per_mile, None); // override unset -> resolves to floor

    // new trip columns round-trip as None/null
    let trips = client.list_trips(None, None, None).await.unwrap();
    assert!(trips[0].settlement_ref.is_none());
    assert!(trips[0].driver_pay_snapshot.is_none());
}
```

- [ ] **Step 3: Run**

Run: `cargo test --test terminals_pay_settlement_test migrates_pre_sprint_db_and_computes_pay`
Expected: PASS. (If it crash-loops/errors, a CAST used an Arrow type name — fix per Critical Constraint #1.)

- [ ] **Step 4: Full suite + clippy**

Run: `cargo test && cargo clippy && cargo build`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add tests/terminals_pay_settlement_test.rs
git commit -m "test(migration): existing-DB migration + backfill guard for sprint columns"
```

---

## Final verification (before PR)

- [ ] `cargo test && cargo clippy && cargo build` — all green.
- [ ] Manual smoke: launch the app, open dispatcher Terminals view, create/edit a terminal, set real rates; open a trip with miles and confirm `driver_pay` renders; set a settlement_ref and confirm pay freezes and edits are rejected.
- [ ] Confirm no `Cargo.toml`/PWA version stamps changed: `git diff main -- Cargo.toml static/` shows no version edits.
- [ ] Confirm no `CAST(NULL AS Utf8|Float64|Int64|utf8|float64)` anywhere: `rg "CAST\(.*AS (Utf8|Float64|Int64|utf8|float64|Double)" src/`.

## Spec coverage map

| Spec requirement | Task(s) |
|---|---|
| terminals table + Default seed | 2 |
| TerminalRecord + rate floor | 1, 2 |
| Terminal CRUD ops + single-default | 3 |
| Terminal CRUD endpoints (dispatcher) | 4 |
| Terminals UI (list/create/edit) | 5 |
| driver.terminal_id + backfill | 6 |
| driver rate overrides (API) | 7 |
| trip rate overrides (API) | 8 |
| per-field resolve_rates | 9 |
| DriverPay + detention compute | 10 |
| total_miles (already done) | verified pre-sprint; surfaced via TripListItem |
| driver_pay compute-on-read | 11 |
| settlement fields + snapshot | 12 |
| freeze + edit-lock (PATCH + stop times) | 13 |
| pay-period range filter | 14 |
| retire terminal_timezone / config | 15 |
| existing-DB migration guard | 16 |
