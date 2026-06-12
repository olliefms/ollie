# Equipment Maintenance Log Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a CRUD "equipment maintenance log" entity — each entry records completed maintenance work tied to exactly one truck or trailer — with a filterable list view, embeds on truck/trailer detail pages, and full coverage across REST, MCP, and the fleet SPA.

**Architecture:** Follows the existing per-entity trio convention exactly, modeled on `trailer`. New `MaintenanceRecord` model, `maintenance` LanceDB table with `merge_insert` upserts, REST read/write handlers under the fleet-user JWT, MCP tools, and SPA list/detail/form pages plus equipment-detail embeds. Parent is polymorphic via `equipment_type` (truck/trailer) + `equipment_id`. Historical-only: no status workflow. Embeddings generated best-effort inline on write. **Hard delete** (a log correction), unlike the soft-delete equipment entities.

**Tech Stack:** Rust (Axum 0.8, LanceDB 0.29, Arrow 58, utoipa 4, chrono, uuid, serde), vanilla-JS fleet SPA, Vitest + happy-dom for SPA tests, axum-test for Rust integration tests.

**Spec:** `docs/superpowers/specs/2026-06-12-equipment-maintenance-log-design.md`

**Conventions locked in for this plan (use these exact names everywhere):**
- Module / table: `maintenance` (Lance table name `"maintenance"`).
- Types: `MaintenanceRecord`, `EquipmentType` (`Truck` | `Trailer`), `MaintenanceCategory` (`PreventiveMaintenance` | `Repair` | `Tire` | `Inspection` | `OilChange` | `Brakes` | `Other`), `MaintenanceListItem`, `MaintenanceListResponse`.
- Scopes: `maintenance:read`, `maintenance:write`, `maintenance:delete`.
- REST routes: `GET/POST /fleet/api/v1/maintenance`, `GET/PATCH/DELETE /fleet/api/v1/maintenance/{id}`.
- MCP tools: `list_maintenance`, `get_maintenance`, `create_maintenance`, `update_maintenance`, `delete_maintenance`.
- SPA view names: `maintenance` (list), `maintenance-new`, `maintenance-detail`, `maintenance-edit`; SPA URLs under `/fleet/maintenance`.
- **Canonical column order** (every Arrow function must match this order exactly):
  `id, equipment_type, equipment_id, service_date, category, description, cost, odometer, vendor, invoice_ref, embedding, owner_id, created_at, updated_at, blob_ids`.

**Commit style:** sign-off required (`git commit -s`). Co-author line:
`Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>`

---

## Task 1: Model — `src/models/maintenance.rs`

**Files:**
- Create: `src/models/maintenance.rs`
- Modify: `src/models/mod.rs` (add `pub mod maintenance;` after line 10 `pub mod load;` region — keep alphabetical-ish with neighbors; and `pub use maintenance::*;`)

- [ ] **Step 1: Write the model file with its unit tests**

Create `src/models/maintenance.rs`:

```rust
// src/models/maintenance.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentType {
    Truck,
    Trailer,
}

impl EquipmentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Truck => "truck",
            Self::Trailer => "trailer",
        }
    }
}

impl std::str::FromStr for EquipmentType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "truck" => Ok(Self::Truck),
            "trailer" => Ok(Self::Trailer),
            other => Err(format!("unknown equipment type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceCategory {
    PreventiveMaintenance,
    Repair,
    Tire,
    Inspection,
    OilChange,
    Brakes,
    Other,
}

impl MaintenanceCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreventiveMaintenance => "preventive_maintenance",
            Self::Repair => "repair",
            Self::Tire => "tire",
            Self::Inspection => "inspection",
            Self::OilChange => "oil_change",
            Self::Brakes => "brakes",
            Self::Other => "other",
        }
    }
}

impl std::str::FromStr for MaintenanceCategory {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "preventive_maintenance" => Ok(Self::PreventiveMaintenance),
            "repair" => Ok(Self::Repair),
            "tire" => Ok(Self::Tire),
            "inspection" => Ok(Self::Inspection),
            "oil_change" => Ok(Self::OilChange),
            "brakes" => Ok(Self::Brakes),
            "other" => Ok(Self::Other),
            other => Err(format!("unknown maintenance category: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MaintenanceRecord {
    pub id: Uuid,
    pub equipment_type: EquipmentType,
    pub equipment_id: Uuid,
    /// ISO date string `YYYY-MM-DD` for when the work was performed.
    pub service_date: String,
    pub category: MaintenanceCategory,
    pub description: String,
    pub cost: Option<f64>,
    pub odometer: Option<i64>,
    pub vendor: Option<String>,
    pub invoice_ref: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MaintenanceRecord {
    /// Base embedding text. The write handler prepends the parent equipment's
    /// unit number so entries are searchable by unit (e.g. "brake job TR-100").
    pub fn embedding_text(&self) -> String {
        format!(
            "{} {} {}",
            self.category.as_str(),
            self.description,
            self.vendor.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MaintenanceListItem {
    pub id: Uuid,
    pub equipment_type: EquipmentType,
    pub equipment_id: Uuid,
    pub service_date: String,
    pub category: MaintenanceCategory,
    pub description: String,
    pub cost: Option<f64>,
    pub odometer: Option<i64>,
    pub vendor: Option<String>,
    pub invoice_ref: Option<String>,
    pub blob_ids: Vec<Uuid>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<MaintenanceRecord> for MaintenanceListItem {
    fn from(r: MaintenanceRecord) -> Self {
        Self {
            id: r.id,
            equipment_type: r.equipment_type,
            equipment_id: r.equipment_id,
            service_date: r.service_date,
            category: r.category,
            description: r.description,
            cost: r.cost,
            odometer: r.odometer,
            vendor: r.vendor,
            invoice_ref: r.invoice_ref,
            blob_ids: r.blob_ids,
            owner_id: r.owner_id,
            created_at: r.created_at,
            score: None,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MaintenanceListResponse {
    pub returned: usize,
    pub items: Vec<MaintenanceListItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_equipment_type_roundtrip() {
        for s in ["truck", "trailer"] {
            let t: EquipmentType = s.parse().unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn test_equipment_type_unknown() {
        assert!("bus".parse::<EquipmentType>().is_err());
    }

    #[test]
    fn test_maintenance_category_roundtrip() {
        for s in [
            "preventive_maintenance", "repair", "tire", "inspection",
            "oil_change", "brakes", "other",
        ] {
            let c: MaintenanceCategory = s.parse().unwrap();
            assert_eq!(c.as_str(), s);
        }
    }

    #[test]
    fn test_maintenance_category_unknown() {
        assert!("transmission".parse::<MaintenanceCategory>().is_err());
    }

    #[test]
    fn test_record_embedding_skipped_in_json() {
        let now = Utc::now();
        let r = MaintenanceRecord {
            id: Uuid::new_v4(),
            equipment_type: EquipmentType::Truck,
            equipment_id: Uuid::new_v4(),
            service_date: "2026-06-01".into(),
            category: MaintenanceCategory::Repair,
            description: "replaced alternator".into(),
            cost: Some(412.50),
            odometer: Some(184000),
            vendor: Some("Acme Diesel".into()),
            invoice_ref: Some("INV-9931".into()),
            blob_ids: vec![],
            embedding: Some(vec![0.1]),
            owner_id: 0,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
        assert_eq!(json["category"], "repair");
        assert_eq!(json["equipment_type"], "truck");
    }
}
```

- [ ] **Step 2: Register the module in `src/models/mod.rs`**

Add `pub mod maintenance;` in the `pub mod` block (place it right after `pub mod load;`):

```rust
pub mod load;
pub mod maintenance;
```

Add `pub use maintenance::*;` in the `pub use` block (right after `pub use load::*;`):

```rust
pub use load::*;
pub use maintenance::*;
```

- [ ] **Step 3: Run the model tests**

Run: `cargo test --lib models::maintenance`
Expected: PASS (5 tests).

- [ ] **Step 4: Commit**

```bash
git add src/models/maintenance.rs src/models/mod.rs
git commit -s -m "feat(maintenance): add MaintenanceRecord model + enums

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 2: Schema + table registration — `src/db/mod.rs` and `src/main.rs`

**Files:**
- Modify: `src/db/mod.rs` (struct field, constructor, `open_or_create_maintenance`, `maintenance_schema`, `empty_maintenance_batch`)
- Modify: `src/main.rs` (vector index at startup)

> The `create_maintenance_vector_index` method itself lives in `db/maintenance_ops.rs` (Task 3); this task adds everything else and wires the startup index call. Until Task 3 lands, `src/main.rs` will not compile — that's expected; Task 2 and Task 3 form one compile unit. Do Task 2 then Task 3 before running `cargo build`.

- [ ] **Step 1: Add the struct field**

In `src/db/mod.rs`, in `pub struct DbClient` (after `pub trailer_table: Table,`):

```rust
    pub trailer_table: Table,
    pub maintenance_table: Table,
```

- [ ] **Step 2: Open the table in the constructor**

In `DbClient::new`, right after the `let trailer_table = open_or_create_trailer(&conn, embed_dim).await?;` line:

```rust
    let trailer_table = open_or_create_trailer(&conn, embed_dim).await?;

    let maintenance_table = open_or_create_maintenance(&conn, embed_dim).await?;
```

And in the `Self { ... }` initializer (after `trailer_table,`):

```rust
        trailer_table,
        maintenance_table,
```

- [ ] **Step 3: Add `open_or_create_maintenance`**

Add this function next to `open_or_create_trailer` in `src/db/mod.rs`:

```rust
async fn open_or_create_maintenance(conn: &lancedb::Connection, embed_dim: usize) -> Result<Table, AppError> {
    let schema = maintenance_schema(embed_dim);
    match conn.open_table("maintenance").execute().await {
        Err(_) => {
            let batch = empty_maintenance_batch(schema.clone(), embed_dim)?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table("maintenance", reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
        Ok(table) => Ok(table),
    }
}
```

- [ ] **Step 4: Add `maintenance_schema` and `empty_maintenance_batch`**

Add next to `trailer_schema` / `empty_trailer_batch` in `src/db/mod.rs`:

```rust
pub fn maintenance_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("equipment_type", DataType::Utf8, false),
        Field::new("equipment_id", DataType::Utf8, false),
        Field::new("service_date", DataType::Utf8, false),
        Field::new("category", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, false),
        Field::new("cost", DataType::Float64, true),
        Field::new("odometer", DataType::Int64, true),
        Field::new("vendor", DataType::Utf8, true),
        Field::new("invoice_ref", DataType::Utf8, true),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
        Field::new("blob_ids", DataType::Utf8, false),
    ]))
}

fn empty_maintenance_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // id
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // equipment_type
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // equipment_id
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // service_date
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // category
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // description
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),   // cost
        Arc::new(Int64Array::from(Vec::<Option<i64>>::new())),     // odometer
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // vendor
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // invoice_ref
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),                               // embedding
        Arc::new(Int64Array::from(Vec::<i64>::new())),             // owner_id
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // created_at
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // updated_at
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),   // blob_ids
    ]).map_err(|e| AppError::Internal(e.to_string()))
}
```

> Note: `Float64Array`, `Int64Array`, `FixedSizeListArray`, `StringArray`, `Field`, `DataType`, `Schema`, `RecordBatch`, `RecordBatchIterator`, `RecordBatchReader`, `Arc` are already imported in `db/mod.rs` (used by the trailer equivalents). No new imports needed.

- [ ] **Step 5: Wire the startup vector index in `src/main.rs`**

In the index-creation array (the `for (result, label) in [ ... ]` block), add after the trailers line:

```rust
    (db.create_trailer_vector_index().await, "trailers"),
    (db.create_maintenance_vector_index().await, "maintenance"),
```

- [ ] **Step 6: (Defer build to Task 3)**

`create_maintenance_vector_index` is implemented in Task 3. Proceed to Task 3 before building. Do not commit a non-compiling tree alone — commit Task 2 + Task 3 together at the end of Task 3.

---

## Task 3: DB operations — `src/db/maintenance_ops.rs`

**Files:**
- Create: `src/db/maintenance_ops.rs`
- Modify: `src/db/mod.rs` (add `mod maintenance_ops;` with the other ops modules — find the block of `mod *_ops;` / `pub mod *_ops;` declarations near the top and mirror how `trailer_ops` is declared)

- [ ] **Step 1: Register the ops module in `src/db/mod.rs`**

Find where `trailer_ops` is declared (e.g. `mod trailer_ops;`) and add alongside:

```rust
mod trailer_ops;
mod maintenance_ops;
```

(If the existing declarations are `pub mod`, match that; mirror `trailer_ops` exactly.)

- [ ] **Step 2: Create `src/db/maintenance_ops.rs` with ops + unit tests**

```rust
// src/db/maintenance_ops.rs
use crate::{
    db::{maintenance_schema, DbClient},
    error::AppError,
    models::{EquipmentType, MaintenanceCategory, MaintenanceListItem, MaintenanceRecord},
};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Float64Array, Int64Array,
    RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray,
};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_maintenance(&self, record: &MaintenanceRecord) -> Result<(), AppError> {
        let batch = maintenance_to_batch(record, self.embed_dim)?;
        let schema = maintenance_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.maintenance_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_maintenance_by_id(&self, id: Uuid) -> Result<MaintenanceRecord, AppError> {
        let id_str = id.to_string();
        let stream = self.maintenance_table.query()
            .only_if(format!("id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_maintenance(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    async fn upsert_maintenance(&self, record: &MaintenanceRecord) -> Result<(), AppError> {
        let batch = maintenance_to_batch(record, self.embed_dim)?;
        let schema = maintenance_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.maintenance_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_maintenance_metadata(
        &self, id: Uuid,
        service_date: Option<String>,
        category: Option<MaintenanceCategory>,
        description: Option<String>,
        cost: Option<f64>,
        odometer: Option<i64>,
        vendor: Option<String>,
        invoice_ref: Option<String>,
        blob_ids: Option<Vec<Uuid>>,
    ) -> Result<MaintenanceRecord, AppError> {
        let mut record = self.get_maintenance_by_id(id).await?;
        if let Some(v) = service_date { record.service_date = v; }
        if let Some(v) = category { record.category = v; }
        if let Some(v) = description { record.description = v; }
        if let Some(v) = cost { record.cost = Some(v); }
        if let Some(v) = odometer { record.odometer = Some(v); }
        if let Some(v) = vendor { record.vendor = Some(v); }
        if let Some(v) = invoice_ref { record.invoice_ref = Some(v); }
        if let Some(v) = blob_ids { record.blob_ids = v; }
        record.updated_at = Utc::now();
        self.upsert_maintenance(&record).await?;
        Ok(record)
    }

    pub async fn update_maintenance_embedding(&self, id: Uuid, embedding: Vec<f32>) -> Result<(), AppError> {
        let mut record = self.get_maintenance_by_id(id).await?;
        record.embedding = Some(embedding);
        record.updated_at = Utc::now();
        self.upsert_maintenance(&record).await
    }

    /// Hard delete — a maintenance entry is a correctable log row.
    pub async fn delete_maintenance(&self, id: Uuid) -> Result<(), AppError> {
        let id_str = id.to_string();
        self.maintenance_table
            .delete(&format!("id = '{id_str}'"))
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn list_maintenance(
        &self,
        equipment_type: Option<&str>,
        equipment_id: Option<&str>,
        category: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<MaintenanceListItem>), AppError> {
        let filter = build_maintenance_filter(equipment_type, equipment_id, category);
        let total = self.maintenance_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.maintenance_table.query();
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_maintenance(collect_stream(stream).await?)?;
        // Most-recent service first; tie-break on created_at desc for stability.
        records.sort_by(|a, b| {
            b.service_date.cmp(&a.service_date)
                .then(b.created_at.cmp(&a.created_at))
        });
        let items: Vec<MaintenanceListItem> = records.into_iter()
            .skip(offset).take(limit).map(MaintenanceListItem::from).collect();
        Ok((total, items))
    }

    pub async fn any_maintenance_references_blob(&self, blob_id: Uuid) -> Result<bool, AppError> {
        let id_str = blob_id.to_string();
        let count = self.maintenance_table
            .count_rows(Some(format!("blob_ids LIKE '%\"{id_str}\"%'")))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(count > 0)
    }

    pub async fn maintenance_referencing_blob(&self, blob_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let id_str = blob_id.to_string();
        let stream = self.maintenance_table.query()
            .only_if(format!("blob_ids LIKE '%\"{id_str}\"%'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_maintenance(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn create_maintenance_vector_index(&self) -> Result<(), AppError> {
        self.create_ivfpq_index(&self.maintenance_table, "embedding", "maintenance").await
    }
}

// --- Helpers ---

fn maintenance_to_batch(record: &MaintenanceRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = maintenance_schema(embed_dim);
    let id_str = record.id.to_string();
    let equipment_type_str = record.equipment_type.as_str();
    let equipment_id_str = record.equipment_id.to_string();
    let category_str = record.category.as_str();
    let created_str = record.created_at.to_rfc3339();
    let updated_str = record.updated_at.to_rfc3339();
    let blob_ids_json = serde_json::to_string(&record.blob_ids)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let embedding_col: Arc<dyn arrow_array::Array> = match &record.embedding {
        Some(v) => {
            let floats: Vec<Option<f32>> = v.iter().map(|&f| Some(f)).collect();
            Arc::new(FixedSizeListArray::from_iter_primitive::<
                arrow_array::types::Float32Type, _, _
            >(vec![Some(floats)], embed_dim as i32))
        }
        None => Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(vec![None::<Vec<Option<f32>>>], embed_dim as i32)),
    };

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![id_str.as_str()])),
        Arc::new(StringArray::from(vec![equipment_type_str])),
        Arc::new(StringArray::from(vec![equipment_id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.service_date.as_str()])),
        Arc::new(StringArray::from(vec![category_str])),
        Arc::new(StringArray::from(vec![record.description.as_str()])),
        Arc::new(Float64Array::from(vec![record.cost])),
        Arc::new(Int64Array::from(vec![record.odometer])),
        Arc::new(StringArray::from(vec![record.vendor.as_deref()])),
        Arc::new(StringArray::from(vec![record.invoice_ref.as_deref()])),
        embedding_col,
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
        Arc::new(StringArray::from(vec![updated_str.as_str()])),
        Arc::new(StringArray::from(vec![blob_ids_json.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_maintenance(batches: Vec<RecordBatch>) -> Result<Vec<MaintenanceRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_maintenance(batch, i)?); }
    }
    Ok(out)
}

fn row_to_maintenance(batch: &RecordBatch, i: usize) -> Result<MaintenanceRecord, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string()).unwrap_or_default()
    };
    let opt_str = |name: &str| -> Option<String> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) })
    };
    let i64_col = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i)).unwrap_or(0)
    };
    let opt_i64 = |name: &str| -> Option<i64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };
    let opt_f64 = |name: &str| -> Option<f64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(MaintenanceRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        equipment_type: str_col("equipment_type").parse().map_err(AppError::Internal)?,
        equipment_id: str_col("equipment_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        service_date: str_col("service_date"),
        category: str_col("category").parse().map_err(AppError::Internal)?,
        description: str_col("description"),
        cost: opt_f64("cost"),
        odometer: opt_i64("odometer"),
        vendor: opt_str("vendor"),
        invoice_ref: opt_str("invoice_ref"),
        blob_ids: serde_json::from_str(&str_col("blob_ids")).unwrap_or_default(),
        embedding,
        owner_id: i64_col("owner_id"),
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_maintenance_filter(
    equipment_type: Option<&str>,
    equipment_id: Option<&str>,
    category: Option<&str>,
) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    if let Some(t) = equipment_type {
        clauses.push(format!("equipment_type = '{}'", t.replace('\'', "''")));
    }
    if let Some(id) = equipment_id {
        clauses.push(format!("equipment_id = '{}'", id.replace('\'', "''")));
    }
    if let Some(c) = category {
        clauses.push(format!("category = '{}'", c.replace('\'', "''")));
    }
    if clauses.is_empty() { None } else { Some(clauses.join(" AND ")) }
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample(equipment_id: Uuid) -> MaintenanceRecord {
        let now = Utc::now();
        MaintenanceRecord {
            id: Uuid::new_v4(),
            equipment_type: EquipmentType::Truck,
            equipment_id,
            service_date: "2026-06-01".into(),
            category: MaintenanceCategory::Repair,
            description: "replaced alternator".into(),
            cost: Some(412.50),
            odometer: Some(184000),
            vendor: Some("Acme Diesel".into()),
            invoice_ref: Some("INV-9931".into()),
            blob_ids: vec![],
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let (db, _dir) = test_db().await;
        let m = sample(Uuid::new_v4());
        db.insert_maintenance(&m).await.unwrap();
        let got = db.get_maintenance_by_id(m.id).await.unwrap();
        assert_eq!(got.id, m.id);
        assert_eq!(got.description, "replaced alternator");
        assert_eq!(got.cost, Some(412.50));
        assert_eq!(got.odometer, Some(184000));
        assert_eq!(got.category, MaintenanceCategory::Repair);
        assert_eq!(got.equipment_type, EquipmentType::Truck);
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_maintenance_by_id(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_update_metadata() {
        let (db, _dir) = test_db().await;
        let m = sample(Uuid::new_v4());
        db.insert_maintenance(&m).await.unwrap();
        let updated = db.update_maintenance_metadata(
            m.id,
            None,
            Some(MaintenanceCategory::Brakes),
            Some("front brake pads".into()),
            Some(220.0),
            None, None, None, None,
        ).await.unwrap();
        assert_eq!(updated.category, MaintenanceCategory::Brakes);
        assert_eq!(updated.description, "front brake pads");
        assert_eq!(updated.cost, Some(220.0));
        // unchanged field preserved
        assert_eq!(updated.odometer, Some(184000));
    }

    #[tokio::test]
    async fn test_hard_delete() {
        let (db, _dir) = test_db().await;
        let m = sample(Uuid::new_v4());
        db.insert_maintenance(&m).await.unwrap();
        db.delete_maintenance(m.id).await.unwrap();
        assert!(matches!(db.get_maintenance_by_id(m.id).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_list_filtered_by_equipment() {
        let (db, _dir) = test_db().await;
        let eq_a = Uuid::new_v4();
        let eq_b = Uuid::new_v4();
        let mut m1 = sample(eq_a);
        m1.service_date = "2026-05-01".into();
        let mut m2 = sample(eq_a);
        m2.service_date = "2026-06-15".into();
        let m3 = sample(eq_b);
        db.insert_maintenance(&m1).await.unwrap();
        db.insert_maintenance(&m2).await.unwrap();
        db.insert_maintenance(&m3).await.unwrap();

        let (total, items) = db.list_maintenance(
            Some("truck"), Some(&eq_a.to_string()), None, 50, 0,
        ).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(items.len(), 2);
        // newest service_date first
        assert_eq!(items[0].service_date, "2026-06-15");
        assert_eq!(items[1].service_date, "2026-05-01");

        let (total_b, _) = db.list_maintenance(
            Some("truck"), Some(&eq_b.to_string()), None, 50, 0,
        ).await.unwrap();
        assert_eq!(total_b, 1);
    }

    #[tokio::test]
    async fn test_list_filtered_by_category() {
        let (db, _dir) = test_db().await;
        let eq = Uuid::new_v4();
        let mut tire = sample(eq);
        tire.category = MaintenanceCategory::Tire;
        let repair = sample(eq);
        db.insert_maintenance(&tire).await.unwrap();
        db.insert_maintenance(&repair).await.unwrap();

        let (total, items) = db.list_maintenance(None, None, Some("tire"), 50, 0).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].category, MaintenanceCategory::Tire);
    }
}
```

- [ ] **Step 3: Build, then run the ops + model tests**

Run: `cargo build`
Expected: compiles clean (Task 2 + Task 3 together).

Run: `cargo test --lib maintenance`
Expected: PASS (model tests + 6 ops tests).

- [ ] **Step 4: Commit (Task 2 + Task 3 together)**

```bash
git add src/db/mod.rs src/db/maintenance_ops.rs src/main.rs
git commit -s -m "feat(maintenance): add LanceDB table, schema, and CRUD ops

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 4: REST write handlers — `src/api/fleet_portal/maintenance_writes.rs`

**Files:**
- Create: `src/api/fleet_portal/maintenance_writes.rs`
- Modify: `src/api/fleet_portal/mod.rs` (add `pub mod maintenance_writes;` and route wiring)

- [ ] **Step 1: Create the write-handlers file**

```rust
// src/api/fleet_portal/maintenance_writes.rs
//
// Fleet-portal maintenance write endpoints:
//   - POST   /fleet/api/v1/maintenance
//   - PATCH  /fleet/api/v1/maintenance/{id}
//   - DELETE /fleet/api/v1/maintenance/{id}   (hard delete — a log correction)
//
// The `apply_*` helpers are shared with the MCP tools so validation and side
// effects (embedding refresh, equipment-existence checks) stay in one place.
// `equipment_type` / `equipment_id` are set on create and are NOT patchable —
// a row belongs to its equipment for life (correct via delete + recreate).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use super::jwt::FleetUserClaims;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{EquipmentType, MaintenanceCategory, MaintenanceRecord},
    AppState,
};

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMaintenanceBody {
    pub equipment_type: EquipmentType,
    pub equipment_id: Uuid,
    pub service_date: String,
    pub category: MaintenanceCategory,
    pub description: String,
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default)]
    pub odometer: Option<i64>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub invoice_ref: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchMaintenanceBody {
    #[serde(default)]
    pub service_date: Option<String>,
    #[serde(default)]
    pub category: Option<MaintenanceCategory>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default)]
    pub odometer: Option<i64>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub invoice_ref: Option<String>,
    #[serde(default)]
    pub blob_ids: Option<Vec<Uuid>>,
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/maintenance",
    request_body(content = CreateMaintenanceBody, description = "Maintenance entry to create"),
    responses(
        (status = 201, description = "Created maintenance record", body = MaintenanceRecord),
        (status = 400, description = "Bad request — unknown field, blank description, or unknown equipment"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn create_maintenance_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("maintenance:write")?;
    let record = apply_maintenance_create(&state, body).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    patch,
    path = "/fleet/api/v1/maintenance/{id}",
    params(("id" = Uuid, Path, description = "Maintenance UUID")),
    request_body(content = PatchMaintenanceBody, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated maintenance record", body = MaintenanceRecord),
        (status = 400, description = "Bad request — unknown field or invalid body"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Maintenance entry not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn update_maintenance_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("maintenance:write")?;
    let record = apply_maintenance_patch(&state, id, body).await?;
    Ok(Json(record))
}

#[utoipa::path(
    delete,
    path = "/fleet/api/v1/maintenance/{id}",
    params(("id" = Uuid, Path, description = "Maintenance UUID")),
    responses(
        (status = 204, description = "Hard-deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Maintenance entry not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn delete_maintenance_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("maintenance:delete")?;
    // 404 if absent, so delete is observable.
    state.db.get_maintenance_by_id(id).await?;
    state.db.delete_maintenance(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Resolve the parent equipment's unit number, erroring if it does not exist.
/// Used both to validate equipment existence and to enrich the embedding text.
async fn resolve_equipment_unit(
    state: &AppState,
    equipment_type: EquipmentType,
    equipment_id: Uuid,
) -> Result<String, AppError> {
    match equipment_type {
        EquipmentType::Truck => {
            let t = state.db.get_truck_by_id(equipment_id).await
                .map_err(|_| AppError::BadRequest(format!("unknown truck: {equipment_id}")))?;
            Ok(t.unit_number)
        }
        EquipmentType::Trailer => {
            let t = state.db.get_trailer_by_id(equipment_id).await
                .map_err(|_| AppError::BadRequest(format!("unknown trailer: {equipment_id}")))?;
            Ok(t.unit_number)
        }
    }
}

pub async fn apply_maintenance_create(
    state: &AppState,
    body: Value,
) -> Result<MaintenanceRecord, AppError> {
    let parsed: CreateMaintenanceBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if parsed.description.trim().is_empty() {
        return Err(AppError::BadRequest("description is required".into()));
    }
    if parsed.service_date.trim().is_empty() {
        return Err(AppError::BadRequest("service_date is required".into()));
    }

    let unit = resolve_equipment_unit(state, parsed.equipment_type, parsed.equipment_id).await?;

    let now = Utc::now();
    let record = MaintenanceRecord {
        id: Uuid::new_v4(),
        equipment_type: parsed.equipment_type,
        equipment_id: parsed.equipment_id,
        service_date: parsed.service_date,
        category: parsed.category,
        description: parsed.description,
        cost: parsed.cost,
        odometer: parsed.odometer,
        vendor: parsed.vendor,
        invoice_ref: parsed.invoice_ref,
        blob_ids: parsed.blob_ids,
        embedding: None,
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };

    let embed_input = format!("{} {}", unit, record.embedding_text());
    let embedding = embed_text(&state.ai, &embed_input).await.ok();
    let record = MaintenanceRecord { embedding, ..record };

    state.db.insert_maintenance(&record).await?;
    Ok(record)
}

pub async fn apply_maintenance_patch(
    state: &AppState,
    id: Uuid,
    body: Value,
) -> Result<MaintenanceRecord, AppError> {
    let parsed: PatchMaintenanceBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if let Some(ref d) = parsed.description {
        if d.trim().is_empty() {
            return Err(AppError::BadRequest("description cannot be empty".into()));
        }
    }

    let updated = state.db.update_maintenance_metadata(
        id,
        parsed.service_date,
        parsed.category,
        parsed.description,
        parsed.cost,
        parsed.odometer,
        parsed.vendor,
        parsed.invoice_ref,
        parsed.blob_ids,
    ).await?;

    // Refresh embedding best-effort, prepending unit number for searchability.
    if let Ok(unit) = resolve_equipment_unit(state, updated.equipment_type, updated.equipment_id).await {
        let embed_input = format!("{} {}", unit, updated.embedding_text());
        if let Ok(embedding) = embed_text(&state.ai, &embed_input).await {
            let _ = state.db.update_maintenance_embedding(id, embedding).await;
        }
    }

    Ok(updated)
}
```

> Verify `state.db.get_truck_by_id` exists (truck analog of `get_trailer_by_id`). If the truck method has a different name, grep `src/db/truck_ops.rs` for the single-get fn and use that name. As of this plan, trucks follow the same trio so `get_truck_by_id` is expected.

- [ ] **Step 2: Register the module + routes in `src/api/fleet_portal/mod.rs`**

Add to the module declarations (with the other `pub mod *_writes;`):

```rust
pub mod maintenance_writes;
```

Add to the router builder, right after the trailer routes block:

```rust
        .route(
            "/fleet/api/v1/maintenance",
            get(data::list_maintenance).post(maintenance_writes::create_maintenance_handler),
        )
        .route(
            "/fleet/api/v1/maintenance/{id}",
            get(data::get_maintenance)
                .patch(maintenance_writes::update_maintenance_handler)
                .delete(maintenance_writes::delete_maintenance_handler),
        )
```

> `data::list_maintenance` / `data::get_maintenance` are added in Task 5; the tree won't compile until Task 5 lands. Tasks 4 and 5 form one compile unit — do both, then build.

- [ ] **Step 3: Proceed to Task 5 before building.**

---

## Task 5: REST read handlers — `src/api/fleet_portal/data.rs`

**Files:**
- Modify: `src/api/fleet_portal/data.rs`

- [ ] **Step 1: Ensure the response type is imported**

In the `use crate::{ ... models::{ ... } }` block at the top of `data.rs`, add `MaintenanceListResponse`:

```rust
        TrailerListResponse,
        MaintenanceListResponse,
```

- [ ] **Step 2: Add the read handlers**

Append a new section (mirroring the Trailers section near the end of `data.rs`):

```rust
// ---------------------------------------------------------------------------
// Maintenance
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListMaintenanceQuery {
    pub equipment_type: Option<String>,
    pub equipment_id: Option<uuid::Uuid>,
    pub category: Option<String>,
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/maintenance",
    params(
        ("equipment_type" = Option<String>, Query, description = "Filter by equipment type (truck/trailer)"),
        ("equipment_id" = Option<Uuid>, Query, description = "Filter by equipment UUID"),
        ("category" = Option<String>, Query, description = "Filter by category"),
    ),
    responses(
        (status = 200, description = "List of maintenance entries", body = MaintenanceListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn list_maintenance(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ListMaintenanceQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("maintenance:read")?;
    let equipment_id = q.equipment_id.map(|id| id.to_string());
    let (total, items) = state.db.list_maintenance(
        q.equipment_type.as_deref(),
        equipment_id.as_deref(),
        q.category.as_deref(),
        100,
        0,
    ).await?;
    Ok(Json(MaintenanceListResponse { returned: total, items }))
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/maintenance/{id}",
    params(("id" = Uuid, Path, description = "Maintenance UUID")),
    responses(
        (status = 200, description = "Maintenance record", body = MaintenanceRecord),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_maintenance(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("maintenance:read")?;
    let record = state.db.get_maintenance_by_id(id).await?;
    Ok(Json(record))
}
```

> `State`, `Extension`, `Query`, `Path`, `Json`, `FleetUserClaims`, `AppState`, `AppError`, `IntoResponse`, `Deserialize` are already imported in `data.rs`. `MaintenanceRecord` is reachable via `crate::models::*` for the utoipa `body =` reference; if utoipa's `ApiDoc` registry needs the schema explicitly, see Task 9.

- [ ] **Step 3: Build and run the full lib + check**

Run: `cargo build`
Expected: compiles clean (Tasks 4 + 5).

- [ ] **Step 4: Commit (Tasks 4 + 5 together)**

```bash
git add src/api/fleet_portal/maintenance_writes.rs src/api/fleet_portal/mod.rs src/api/fleet_portal/data.rs
git commit -s -m "feat(maintenance): add REST read + write handlers

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 6: REST integration tests

**Files:**
- Modify: `tests/integration_test.rs` (append maintenance tests + a `create_truck` helper if one does not already exist)

- [ ] **Step 1: Check for an existing truck-create helper**

Run: `grep -n "async fn create_truck\|async fn create_trailer" tests/integration_test.rs`
If `create_truck` is missing, add this helper near `create_trailer` (defined around line 6006):

```rust
async fn create_truck(server: &TestServer, unit: &str) -> uuid::Uuid {
    let owner_token = setup_owner(server).await;
    server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": unit }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().parse().unwrap()
}
```

> If `create_truck` exists, reuse it. If the truck create body needs more required fields than `unit_number`, copy the shape from an existing truck test in the file.

- [ ] **Step 2: Append the maintenance integration tests**

```rust
#[tokio::test]
async fn test_fleet_user_maintenance_crud_http() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mnt-crud@example.com", "password-mnt1").await;
    let auth = format!("Bearer {token}");
    let truck_id = create_truck(&server, "MNT-TRK-1").await;

    // POST create
    let created = server.post("/fleet/api/v1/maintenance")
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({
            "equipment_type": "truck",
            "equipment_id": truck_id,
            "service_date": "2026-06-01",
            "category": "repair",
            "description": "replaced alternator",
            "cost": 412.5,
            "odometer": 184000,
            "vendor": "Acme Diesel",
            "invoice_ref": "INV-9931"
        }))
        .await;
    assert_eq!(created.status_code(), 201);
    let body: serde_json::Value = created.json();
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["category"], "repair");
    assert_eq!(body["equipment_type"], "truck");
    assert_eq!(body["cost"], 412.5);

    // GET one
    let one = server.get(&format!("/fleet/api/v1/maintenance/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(one.status_code(), 200);
    assert_eq!(one.json::<serde_json::Value>()["description"], "replaced alternator");

    // GET list filtered by equipment
    let list = server.get(&format!(
        "/fleet/api/v1/maintenance?equipment_type=truck&equipment_id={truck_id}"
    ))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(list.status_code(), 200);
    let items = list.json::<serde_json::Value>()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], id);

    // PATCH update
    let patched = server.patch(&format!("/fleet/api/v1/maintenance/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .json(&serde_json::json!({ "category": "brakes", "description": "front pads" }))
        .await;
    assert_eq!(patched.status_code(), 200);
    let pbody: serde_json::Value = patched.json();
    assert_eq!(pbody["category"], "brakes");
    assert_eq!(pbody["description"], "front pads");

    // DELETE (hard)
    let deleted = server.delete(&format!("/fleet/api/v1/maintenance/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(deleted.status_code(), 204);
    let gone = server.get(&format!("/fleet/api/v1/maintenance/{id}"))
        .add_header(header::AUTHORIZATION, &auth)
        .await;
    assert_eq!(gone.status_code(), 404);
}

#[tokio::test]
async fn test_maintenance_create_rejects_unknown_equipment() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mnt-eq@example.com", "password-mnt2").await;
    let bogus = uuid::Uuid::new_v4();

    let resp = server.post("/fleet/api/v1/maintenance")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "equipment_type": "truck",
            "equipment_id": bogus,
            "service_date": "2026-06-01",
            "category": "repair",
            "description": "ghost truck"
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_maintenance_create_rejects_unknown_field() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mnt-unk@example.com", "password-mnt3").await;
    let trailer_id = create_trailer(&server, "MNT-TRL-1").await;

    let resp = server.post("/fleet/api/v1/maintenance")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "equipment_type": "trailer",
            "equipment_id": trailer_id,
            "service_date": "2026-06-01",
            "category": "tire",
            "description": "new tires",
            "owner_id": 5
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}
```

- [ ] **Step 3: Run the maintenance integration tests**

Run: `cargo test --test integration_test maintenance -- --nocapture`
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add tests/integration_test.rs
git commit -s -m "test(maintenance): REST CRUD, equipment validation, unknown-field

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 7: MCP tools — `src/api/fleet_portal/mcp.rs`

**Files:**
- Modify: `src/api/fleet_portal/mcp.rs` (8 locations)

- [ ] **Step 1: Scope map** — in `tool_required_scope`, after the Trailers arms:

```rust
        // Maintenance
        "list_maintenance" | "get_maintenance" => "maintenance:read",
        "create_maintenance" | "update_maintenance" => "maintenance:write",
        "delete_maintenance" => "maintenance:delete",
```

- [ ] **Step 2: Destructive op set** — in `is_destructive_op`, add `| "delete_maintenance"` to the first match arm list:

```rust
        | "delete_truck" | "delete_trailer" | "delete_facility" | "delete_user"
        | "delete_maintenance" => true,
```

- [ ] **Step 3: Destructive description** — in `destructive_op_description`, after the `"delete_trailer"` arm:

```rust
        "delete_maintenance" => "permanently delete the maintenance entry",
```

- [ ] **Step 4: Annotations** — in `annotations_for`, add `| "delete_maintenance"` to the `destructive` matches list:

```rust
            | "delete_facility"
            | "delete_maintenance"
```

- [ ] **Step 5: Paginated list tools** — add to `PAGINATED_LIST_TOOLS`:

```rust
    "list_trailers",
    "list_maintenance",
```

- [ ] **Step 6: Dispatch** — in `handle_tool_call`, add list/get/create/update arms after the trailer arms, and the delete arm in the delete group:

```rust
        "list_maintenance" => tool_list_maintenance(state, args).await,
        "get_maintenance" => tool_get_maintenance(state, args).await,
        "create_maintenance" => tool_create_maintenance(state, args).await,
        "update_maintenance" => tool_update_maintenance(state, args).await,
```

```rust
        "delete_maintenance" => tool_delete_maintenance(state, args).await,
```

- [ ] **Step 7: Tool definitions** — in `tools_list()` JSON, after the trailer tool defs (and the delete_trailer def), add:

```json
            {
                "name": "list_maintenance",
                "description": "List equipment maintenance entries. Optional filters: equipment_type (truck/trailer), equipment_id, category. Newest service_date first.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "equipment_type": { "type": "string", "enum": ["truck","trailer"] },
                        "equipment_id":   { "type": "string", "format": "uuid" },
                        "category":       { "type": "string", "enum": ["preventive_maintenance","repair","tire","inspection","oil_change","brakes","other"] }
                    }
                }
            },
            {
                "name": "get_maintenance",
                "description": "Get a single maintenance entry by UUID.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "maintenance_id": { "type": "string", "format": "uuid" } },
                    "required": ["maintenance_id"]
                }
            },
            {
                "name": "create_maintenance",
                "description": "Record completed maintenance work on a truck or trailer. `equipment_id` must reference an existing unit of the given `equipment_type`. `service_date` is an ISO date (YYYY-MM-DD). Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "equipment_type": { "type": "string", "enum": ["truck","trailer"] },
                        "equipment_id":   { "type": "string", "format": "uuid" },
                        "service_date":   { "type": "string" },
                        "category":       { "type": "string", "enum": ["preventive_maintenance","repair","tire","inspection","oil_change","brakes","other"] },
                        "description":    { "type": "string" },
                        "cost":           { "type": "number" },
                        "odometer":       { "type": "integer" },
                        "vendor":         { "type": "string" },
                        "invoice_ref":    { "type": "string" },
                        "blob_ids":       { "type": "array", "items": { "type": "string", "format": "uuid" } }
                    },
                    "required": ["equipment_type", "equipment_id", "service_date", "category", "description"]
                }
            },
            {
                "name": "update_maintenance",
                "description": "Update a maintenance entry's fields. equipment_type/equipment_id are not changeable (delete + recreate to re-link). Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "maintenance_id": { "type": "string", "format": "uuid" },
                        "service_date":   { "type": "string" },
                        "category":       { "type": "string", "enum": ["preventive_maintenance","repair","tire","inspection","oil_change","brakes","other"] },
                        "description":    { "type": "string" },
                        "cost":           { "type": "number" },
                        "odometer":       { "type": "integer" },
                        "vendor":         { "type": "string" },
                        "invoice_ref":    { "type": "string" },
                        "blob_ids":       { "type": "array", "items": { "type": "string", "format": "uuid" } }
                    },
                    "required": ["maintenance_id"]
                }
            },
```

And in the delete-tools area (next to `delete_trailer`):

```json
            {
                "name": "delete_maintenance",
                "description": "Permanently delete a maintenance entry (hard delete). Returns { deleted: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "maintenance_id": { "type": "string", "format": "uuid" } },
                    "required": ["maintenance_id"]
                }
            },
```

- [ ] **Step 8: Tool implementation functions** — add next to the trailer tool fns:

```rust
async fn tool_list_maintenance(state: &AppState, args: &Value) -> Result<Value, String> {
    let offset = cursor_offset(args)?;
    let equipment_type = args["equipment_type"].as_str().map(|s| s.to_string());
    let equipment_id = args["equipment_id"].as_str().map(|s| s.to_string());
    let category = args["category"].as_str().map(|s| s.to_string());
    let (total, items) = state.db.list_maintenance(
        equipment_type.as_deref(),
        equipment_id.as_deref(),
        category.as_deref(),
        PAGE_SIZE,
        offset,
    ).await.map_err(|e| e.to_string())?;
    let returned = items.len();
    Ok(mcp_content(paged(items, returned, total, offset)))
}

async fn tool_get_maintenance(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "maintenance_id")?;
    let record = state.db.get_maintenance_by_id(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_create_maintenance(state: &AppState, args: &Value) -> Result<Value, String> {
    let record = super::maintenance_writes::apply_maintenance_create(state, args.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_update_maintenance(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "maintenance_id")?;
    let mut body = args.clone();
    if let Value::Object(map) = &mut body {
        map.remove("maintenance_id");
    }
    let record = super::maintenance_writes::apply_maintenance_patch(state, id, body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_delete_maintenance(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "maintenance_id")?;
    state.db.get_maintenance_by_id(id).await.map_err(|e| e.to_string())?;
    state.db.delete_maintenance(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "deleted": true })))
}
```

> `cursor_offset`, `parse_uuid`, `mcp_content`, `paged`, `PAGE_SIZE` are existing helpers used by the trailer tool fns — reuse as-is.

- [ ] **Step 9: Optionally wire blob reverse-lookup** — if `tool_get_blob_metadata` and `tool_delete_blob` enumerate every entity that references a blob, add maintenance to keep the reverse lookup complete:
  - In `tool_get_blob_metadata`: add `let maintenance = state.db.maintenance_referencing_blob(id).await.map_err(|e| e.to_string())?;` and include `"maintenance": maintenance` in the `attached_to` object.
  - In `tool_delete_blob`: add `let attached_to_maintenance = state.db.any_maintenance_references_blob(id).await.map_err(|e| e.to_string())?;` and OR it into `was_attached`.

  (These DB methods were added in Task 3. This step is required only because maintenance supports `blob_ids`.)

- [ ] **Step 10: Build**

Run: `cargo build`
Expected: compiles clean.

- [ ] **Step 11: Commit**

```bash
git add src/api/fleet_portal/mcp.rs
git commit -s -m "feat(maintenance): add MCP tools + scopes + blob reverse-lookup

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 8: MCP integration test

**Files:**
- Modify: `tests/integration_test.rs` (append one MCP round-trip test using the existing `mcp_call` helper)

- [ ] **Step 1: Append the MCP test**

```rust
#[tokio::test]
async fn test_mcp_maintenance_crud() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = fleet_user_login(&server, "mnt-mcp@example.com", "password-mnt-mcp").await;
    let truck_id = create_truck(&server, "MCP-TRK-1").await;

    let created = mcp_call(&server, &token, "create_maintenance", serde_json::json!({
        "equipment_type": "truck",
        "equipment_id": truck_id,
        "service_date": "2026-06-02",
        "category": "oil_change",
        "description": "full synthetic + filter"
    })).await;
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["category"], "oil_change");

    let got = mcp_call(&server, &token, "get_maintenance", serde_json::json!({
        "maintenance_id": id
    })).await;
    assert_eq!(got["description"], "full synthetic + filter");

    let listed = mcp_call(&server, &token, "list_maintenance", serde_json::json!({
        "equipment_type": "truck",
        "equipment_id": truck_id
    })).await;
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);

    let deleted = mcp_call(&server, &token, "delete_maintenance", serde_json::json!({
        "maintenance_id": id
    })).await;
    assert_eq!(deleted["deleted"], true);
}
```

> `mcp_call` is defined around line 5348. If `delete_maintenance` triggers a destructive-confirmation gate in `mcp_call` (the helper asserts no error), check how existing destructive deletes are tested (search for `delete_trailer` / `cancel_trip` MCP tests) and mirror their confirmation handling — e.g. pass a `confirm: true` arg if that is the established pattern. Match whatever the trailer/trip MCP delete tests do.

- [ ] **Step 2: Run**

Run: `cargo test --test integration_test test_mcp_maintenance_crud -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/integration_test.rs
git commit -s -m "test(maintenance): MCP CRUD round-trip

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 9: Docs surface — `llms.txt` + utoipa registration

**Files:**
- Modify: `src/api/mod.rs` (the `LLMS_TXT` constant and the `ApiDoc` utoipa `paths`/`components` if entities are registered there)

- [ ] **Step 1: Find how trailers are documented**

Run: `grep -n "trailer\|Trailer" src/api/mod.rs`
This shows the `LLMS_TXT` lines and any utoipa `paths(...)` / `components(schemas(...))` registrations for trailers.

- [ ] **Step 2: Mirror for maintenance in `LLMS_TXT`**

Add a maintenance entry alongside the trailers entry in the `LLMS_TXT` string. Match the surrounding format exactly. Example line to adapt to the existing style:

```
- GET/POST /fleet/api/v1/maintenance, GET/PATCH/DELETE /fleet/api/v1/maintenance/{id} — equipment maintenance log (filter by equipment_type, equipment_id, category)
```

- [ ] **Step 3: Register utoipa paths/schemas (only if trailers are registered there)**

If `ApiDoc` lists handler paths and schemas explicitly, add the maintenance handlers and DTOs in the same lists:
- paths: `fleet_portal::data::list_maintenance`, `fleet_portal::data::get_maintenance`, `fleet_portal::maintenance_writes::create_maintenance_handler`, `fleet_portal::maintenance_writes::update_maintenance_handler`, `fleet_portal::maintenance_writes::delete_maintenance_handler`
- schemas: `MaintenanceRecord`, `MaintenanceListItem`, `MaintenanceListResponse`, `MaintenanceCategory`, `EquipmentType`, `CreateMaintenanceBody`, `PatchMaintenanceBody`

If trailers are NOT explicitly registered (utoipa auto-collects), skip this step.

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: compiles clean.

- [ ] **Step 5: Commit**

```bash
git add src/api/mod.rs
git commit -s -m "docs(maintenance): document REST surface in llms.txt

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 10: SPA list page + routing/nav wiring

**Files:**
- Create: `static/fleet/pages/maintenance.js`
- Create: `static/fleet/utils/maintenance-meta.js` (shared category labels/options + a column/date formatter, reused by list, form, detail, and the equipment embed)
- Modify: `static/fleet/router.js`, `static/fleet/app.js`, `static/fleet/utils/dom.js`, `static/fleet/index.html`

- [ ] **Step 1: Create the shared metadata helper**

`static/fleet/utils/maintenance-meta.js`:

```javascript
// Shared maintenance category metadata + formatters, reused by the list,
// form, detail, and equipment-embed views so labels stay consistent.
export const CATEGORY_OPTIONS = [
  { value: 'preventive_maintenance', label: 'Preventive Maintenance' },
  { value: 'repair', label: 'Repair' },
  { value: 'tire', label: 'Tire' },
  { value: 'inspection', label: 'Inspection' },
  { value: 'oil_change', label: 'Oil Change' },
  { value: 'brakes', label: 'Brakes' },
  { value: 'other', label: 'Other' },
];

const LABELS = Object.fromEntries(CATEGORY_OPTIONS.map(o => [o.value, o.label]));

export function categoryLabel(value) {
  return LABELS[value] || value || '—';
}

export function money(value) {
  if (value == null) return '—';
  return `$${Number(value).toFixed(2)}`;
}
```

- [ ] **Step 2: Create the list page**

`static/fleet/pages/maintenance.js`:

```javascript
import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent } from '../utils/dom.js';
import { renderEntityList } from './_list.js';
import { categoryLabel, money, CATEGORY_OPTIONS } from '../utils/maintenance-meta.js';

export async function renderMaintenanceView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  const qs = new URLSearchParams();
  if (params.equipment_type) qs.set('equipment_type', params.equipment_type);
  if (params.equipment_id) qs.set('equipment_id', params.equipment_id);
  if (params.category) qs.set('category', params.category);
  const suffix = qs.toString() ? `?${qs.toString()}` : '';

  try {
    const res = await apiFetch(`${API_BASE}/maintenance${suffix}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.items || (Array.isArray(data) ? data : []);

    const categoryFilter = `
      <label class="filter-control">Category
        <select id="mnt-category-filter">
          <option value="">All</option>
          ${CATEGORY_OPTIONS.map(o =>
            `<option value="${o.value}"${params.category === o.value ? ' selected' : ''}>${escHtml(o.label)}</option>`
          ).join('')}
        </select>
      </label>`;

    renderEntityList({
      title: 'Maintenance',
      createView: 'maintenance-new',
      createScope: 'maintenance:write',
      createLabel: '+ Add Maintenance',
      detailView: 'maintenance-detail',
      emptyText: 'No maintenance entries found.',
      extraControls: categoryFilter,
      columns: [
        { header: 'Date',     cell: m => escHtml(m.service_date || '—'), html: true },
        { header: 'Equipment', cell: m => escHtml(`${m.equipment_type || ''}`), html: true },
        { header: 'Category', cell: m => escHtml(categoryLabel(m.category)), html: true },
        { header: 'Description', cell: m => escHtml(m.description || '—'), html: true },
        { header: 'Cost',     cell: m => escHtml(money(m.cost)), html: true },
        { header: 'Vendor',   cell: m => escHtml(m.vendor || '—'), html: true },
      ],
      rows: items,
    });

    const sel = document.getElementById('mnt-category-filter');
    if (sel) {
      sel.addEventListener('change', () => {
        renderMaintenanceView({ ...params, category: sel.value || undefined });
      });
    }
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load maintenance: ${escHtml(err.message)}</div>`);
    }
  }
}
```

> Confirm `renderEntityList` accepts an `extraControls` option (the explore notes list it in the `_list.js` signature). If the option name differs, grep `static/fleet/pages/_list.js` for how `extraControls` is consumed and adjust. If `_list.js` does not support extra controls, render the filter `<select>` by wrapping: call `renderEntityList` first, then `document.querySelector('.page-header')?.insertAdjacentHTML('beforeend', categoryFilter)` and attach the listener. Use whichever the file actually supports.

- [ ] **Step 3: Add routes in `static/fleet/router.js`**

After the trailer route entries:

```javascript
  { name: 'maintenance',         re: /^\/fleet\/maintenance$/ },
  { name: 'maintenance-new',     re: /^\/fleet\/maintenance\/new$/ },
  { name: 'maintenance-edit',    re: /^\/fleet\/maintenance\/([^/]+)\/edit$/, id: true },
  { name: 'maintenance-detail',  re: /^\/fleet\/maintenance\/([^/]+)$/, id: true },
```

- [ ] **Step 4: Add view-path mappings in `static/fleet/utils/dom.js`**

In `VIEW_PATHS` (after the trailer entries):

```javascript
  maintenance: () => '/fleet/maintenance',
  'maintenance-new': () => '/fleet/maintenance/new',
  'maintenance-detail': (p) => `/fleet/maintenance/${p.id}`,
  'maintenance-edit': (p) => `/fleet/maintenance/${p.id}/edit`,
```

- [ ] **Step 5: Wire `static/fleet/app.js`** — imports, route dispatch, titles

Imports (with the trailer imports):

```javascript
import { renderMaintenanceView } from './pages/maintenance.js';
import { renderMaintenanceDetail } from './pages/maintenance-detail.js';
import { renderMaintenanceForm } from './pages/maintenance-form.js';
```

Route dispatch (in the `renderRoute` switch, after trailer cases):

```javascript
    case 'maintenance': renderMaintenanceView(params.query || {}); break;
    case 'maintenance-new': renderMaintenanceForm(null, params.query || {}); break;
    case 'maintenance-detail': renderMaintenanceDetail(params.id); break;
    case 'maintenance-edit': renderMaintenanceForm(params.id); break;
```

> Check how the router passes query-string params to the dispatcher. If `params.query` is not how other pages receive query params, mirror the existing convention (or parse `new URLSearchParams(location.search)` inside the page). The list page already tolerates an empty `params` object. For `maintenance-new`, the second arg pre-fills equipment from the deep link (Task 11/13); if the dispatcher cannot pass it, the form reads `location.search` itself.

`VIEW_TITLES` (with the trailer entries):

```javascript
  maintenance: 'Maintenance',
  'maintenance-new': 'New Maintenance Record',
  'maintenance-detail': 'Maintenance Record',
  'maintenance-edit': 'Edit Maintenance Record',
```

- [ ] **Step 6: Add the nav link in `static/fleet/index.html`**

In the `Equipment` sidebar group (after the Trailers link, around line 112):

```html
<a class="sidebar__link" data-link href="/fleet/maintenance"><span>Maintenance</span></a>
```

- [ ] **Step 7: Manual smoke check (optional now, full test in Task 14)**

Run: `npm test` (existing suite still green; new tests come in Task 14).
Expected: PASS (no regressions).

- [ ] **Step 8: Commit**

```bash
git add static/fleet/pages/maintenance.js static/fleet/utils/maintenance-meta.js static/fleet/router.js static/fleet/app.js static/fleet/utils/dom.js static/fleet/index.html
git commit -s -m "feat(maintenance): SPA list page + routing + nav

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 11: SPA form page (create/edit)

**Files:**
- Create: `static/fleet/pages/maintenance-form.js`

- [ ] **Step 1: Create the form page**

```javascript
import { apiFetch, API_BASE } from '../utils/api.js';
import { navigate } from '../utils/dom.js';
import { renderFormPage } from './_form.js';
import { CATEGORY_OPTIONS } from '../utils/maintenance-meta.js';

// equipment_type/equipment_id are set on create and not editable afterward
// (the backend ignores them on PATCH). On create they are pre-fillable via the
// deep link from an equipment detail page.
function fields({ editing }) {
  return [
    { key: 'equipment_type', label: 'Equipment Type', type: 'select', required: true,
      options: [{ value: 'truck', label: 'Truck' }, { value: 'trailer', label: 'Trailer' }],
      disabled: editing },
    { key: 'equipment_id', label: 'Equipment ID (UUID)', type: 'text', required: true,
      disabled: editing },
    { key: 'service_date', label: 'Service Date', type: 'date', required: true },
    { key: 'category', label: 'Category', type: 'select', required: true,
      options: CATEGORY_OPTIONS },
    { key: 'description', label: 'Description', type: 'text', required: true },
    { key: 'cost', label: 'Cost', type: 'number' },
    { key: 'odometer', label: 'Odometer', type: 'int' },
    { key: 'vendor', label: 'Vendor', type: 'text' },
    { key: 'invoice_ref', label: 'Invoice Ref', type: 'text' },
  ];
}

export async function renderMaintenanceForm(id, prefill = {}) {
  let values = {};
  if (id) {
    const res = await apiFetch(`${API_BASE}/maintenance/${encodeURIComponent(id)}`);
    if (res.ok) values = await res.json();
  } else {
    // Deep-link prefill from equipment detail (equipment_type + equipment_id).
    const fromQuery = new URLSearchParams(location.search);
    values = {
      equipment_type: prefill.equipment_type || fromQuery.get('equipment_type') || undefined,
      equipment_id: prefill.equipment_id || fromQuery.get('equipment_id') || undefined,
    };
  }

  renderFormPage({
    title: id ? `Edit Maintenance — ${values.service_date || ''}` : 'New Maintenance',
    fields: fields({ editing: !!id }),
    values,
    submitLabel: id ? 'Save changes' : 'Add maintenance',
    onSubmit: async (payload) => {
      // equipment_* are immutable on edit — the backend rejects unknown/extra
      // fields, so strip them from the PATCH payload.
      if (id) {
        delete payload.equipment_type;
        delete payload.equipment_id;
      }
      const url = id
        ? `${API_BASE}/maintenance/${encodeURIComponent(id)}`
        : `${API_BASE}/maintenance`;
      const res = await apiFetch(url, {
        method: id ? 'PATCH' : 'POST',
        body: JSON.stringify(payload),
      });
      if (res.ok) {
        const saved = await res.json().catch(() => ({}));
        navigate('maintenance-detail', { id: id || saved.id });
      }
      return res;
    },
  });
}
```

> Confirm the form component supports a `disabled` field flag and `date` / `int` / `number` field types (the explore notes list these types in `components/form.js`). If `disabled` is unsupported, drop it — the backend already ignores `equipment_*` on PATCH, so an editable-but-ignored field is acceptable; alternatively render those two as plain read-only text on edit. If `buildPayload` omits empty optional fields, that's the desired behavior (don't send blank `cost`).

- [ ] **Step 2: Smoke test**

Run: `npm test`
Expected: PASS (no regressions).

- [ ] **Step 3: Commit**

```bash
git add static/fleet/pages/maintenance-form.js
git commit -s -m "feat(maintenance): SPA create/edit form

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 12: SPA detail page

**Files:**
- Create: `static/fleet/pages/maintenance-detail.js`

- [ ] **Step 1: Create the detail page**

```javascript
import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';
import { renderDetailPage } from './_detail.js';
import { confirmDelete } from '../components/confirm.js';
import { categoryLabel, money } from '../utils/maintenance-meta.js';

export async function renderMaintenanceDetail(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/maintenance/${encodeURIComponent(id)}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const m = await res.json();

    renderDetailPage({
      title: `Maintenance — ${m.service_date || ''}`.trim(),
      fields: [
        { label: 'Service Date', value: m.service_date },
        { label: 'Equipment Type', value: m.equipment_type },
        { label: 'Equipment ID', value: m.equipment_id },
        { label: 'Category', value: categoryLabel(m.category) },
        { label: 'Description', value: m.description },
        { label: 'Cost', value: money(m.cost) },
        { label: 'Odometer', value: m.odometer },
        { label: 'Vendor', value: m.vendor },
        { label: 'Invoice Ref', value: m.invoice_ref },
      ],
      actions: [
        { label: 'Edit', scope: 'maintenance:write', onClick: () => navigate('maintenance-edit', { id }) },
        { label: 'Delete', scope: 'maintenance:delete', onClick: (statusEl) => deleteMaintenance(statusEl, id) },
      ],
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load maintenance: ${escHtml(err.message)}</div>`);
    }
  }
}

// Hard delete: the entry is removed entirely.
async function deleteMaintenance(statusEl, id) {
  if (!confirmDelete('this maintenance entry')) return;
  try {
    const res = await apiFetch(`${API_BASE}/maintenance/${encodeURIComponent(id)}`, { method: 'DELETE' });
    if (res.ok || res.status === 204) { navigate('maintenance'); return; }
    const data = await res.json().catch(() => ({}));
    statusEl.hidden = false;
    statusEl.className = 'alert alert--error';
    statusEl.textContent = data.error || `Delete failed (HTTP ${res.status}).`;
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      statusEl.hidden = false;
      statusEl.className = 'alert alert--error';
      statusEl.textContent = `Delete failed: ${err.message}`;
    }
  }
}
```

- [ ] **Step 2: Smoke test**

Run: `npm test`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add static/fleet/pages/maintenance-detail.js
git commit -s -m "feat(maintenance): SPA detail page

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 13: Equipment-detail embeds (truck + trailer)

**Files:**
- Create: `static/fleet/pages/_maintenance-history.js` (shared embed renderer)
- Modify: `static/fleet/pages/truck-detail.js`, `static/fleet/pages/trailer-detail.js`

- [ ] **Step 1: Create the shared embed renderer**

`renderDetailPage` writes the whole `#main-content`, so the embed appends a section to `#main-content` after the detail card is rendered.

`static/fleet/pages/_maintenance-history.js`:

```javascript
import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { navigate } from '../utils/dom.js';
import { hasScope } from '../utils/api.js';
import { categoryLabel, money } from '../utils/maintenance-meta.js';

// Appends a "Maintenance History" section to #main-content for the given
// equipment. Call AFTER renderDetailPage() has populated the page.
export async function appendMaintenanceHistory(equipmentType, equipmentId) {
  const host = document.getElementById('main-content');
  if (!host) return;

  const section = document.createElement('div');
  section.className = 'detail-card';
  section.style.marginTop = 'var(--space-4)';
  section.innerHTML = `
    <div class="detail-card__title">Maintenance History</div>
    <div id="mnt-history-body"><div class="spinner"></div></div>`;
  host.appendChild(section);

  const addBtn = hasScope('maintenance:write')
    ? `<button class="btn btn--secondary" id="mnt-add-btn">+ Add maintenance</button>`
    : '';

  try {
    const res = await apiFetch(
      `${API_BASE}/maintenance?equipment_type=${encodeURIComponent(equipmentType)}&equipment_id=${encodeURIComponent(equipmentId)}`
    );
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.items || [];

    const rows = items.map(m => `
      <tr data-id="${escHtml(m.id)}" class="clickable-row">
        <td>${escHtml(m.service_date || '—')}</td>
        <td>${escHtml(categoryLabel(m.category))}</td>
        <td>${escHtml(m.description || '—')}</td>
        <td>${escHtml(money(m.cost))}</td>
        <td>${escHtml(m.vendor || '—')}</td>
      </tr>`).join('');

    const table = items.length
      ? `<table class="data-table">
           <thead><tr><th>Date</th><th>Category</th><th>Description</th><th>Cost</th><th>Vendor</th></tr></thead>
           <tbody>${rows}</tbody>
         </table>`
      : `<div class="state-empty">No maintenance entries.</div>`;

    document.getElementById('mnt-history-body').innerHTML = `${table}<div style="margin-top:var(--space-3);">${addBtn}</div>`;

    section.querySelectorAll('tr[data-id]').forEach(tr => {
      tr.addEventListener('click', () => navigate('maintenance-detail', { id: tr.dataset.id }));
    });
    const btn = document.getElementById('mnt-add-btn');
    if (btn) {
      btn.addEventListener('click', () => {
        navigate('maintenance-new', { query: { equipment_type: equipmentType, equipment_id: equipmentId } });
      });
    }
  } catch (err) {
    const body = document.getElementById('mnt-history-body');
    if (body) body.innerHTML = `<div class="state-error">Failed to load maintenance: ${escHtml(err.message)}</div>`;
  }
}
```

> Two things to confirm against the codebase and adjust if needed:
> 1. `navigate(view, opts)` — confirm how `navigate` forwards query params (the `{ query: {...} }` shape). If `navigate` builds the URL via `VIEW_PATHS` only, then instead build the deep link explicitly: `navigate('maintenance-new')` won't carry params, so use `location.assign('/fleet/maintenance/new?equipment_type=...&equipment_id=...')` or extend `VIEW_PATHS['maintenance-new']` to read query. Simplest robust option: `window.history.pushState({}, '', \`/fleet/maintenance/new?equipment_type=${equipmentType}&equipment_id=${equipmentId}\`)` followed by the app's route-refresh call. Match the app's existing navigation mechanism.
> 2. CSS classes (`data-table`, `clickable-row`, `state-empty`) — reuse whatever the existing tables use; grep `components/table.js` output classes and match them so styling is consistent.

- [ ] **Step 2: Call it from `truck-detail.js`**

Add the import at the top:

```javascript
import { appendMaintenanceHistory } from './_maintenance-history.js';
```

Immediately after the `renderDetailPage({ ... })` call in `renderTruckDetail`, add:

```javascript
    await appendMaintenanceHistory('truck', id);
```

- [ ] **Step 3: Call it from `trailer-detail.js`**

Add the import:

```javascript
import { appendMaintenanceHistory } from './_maintenance-history.js';
```

After the `renderDetailPage({ ... })` call in `renderTrailerDetail`:

```javascript
    await appendMaintenanceHistory('trailer', id);
```

- [ ] **Step 4: Smoke test**

Run: `npm test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add static/fleet/pages/_maintenance-history.js static/fleet/pages/truck-detail.js static/fleet/pages/trailer-detail.js
git commit -s -m "feat(maintenance): embed maintenance history on equipment detail

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 14: SPA Vitest tests

**Files:**
- Create: `tests/fleet/maintenance.test.js`

- [ ] **Step 1: Write the page tests**

```javascript
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { clearMe, API_BASE } from '../../static/fleet/utils/api.js';

function jsonResponse(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

beforeEach(() => {
  document.body.innerHTML = '<div id="main-content"></div>';
  localStorage.clear();
  clearMe();
  saveToken('tok');
  // Grant all scopes so action buttons render. loadMe caches identity/scopes.
  vi.restoreAllMocks();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('renderMaintenanceView', () => {
  it('lists maintenance entries returned by the API', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({
      returned: 1,
      items: [{
        id: 'm1', equipment_type: 'truck', equipment_id: 't1',
        service_date: '2026-06-01', category: 'repair',
        description: 'alternator', cost: 412.5, vendor: 'Acme',
      }],
    }));
    vi.stubGlobal('fetch', fetchMock);

    const { renderMaintenanceView } = await import('../../static/fleet/pages/maintenance.js');
    await renderMaintenanceView({});
    await Promise.resolve();

    const html = document.getElementById('main-content').innerHTML;
    expect(html).toContain('alternator');
    expect(html).toContain('Repair');
    expect(html).toContain('$412.50');
    // request hit the maintenance endpoint
    expect(fetchMock.mock.calls[0][0]).toContain(`${API_BASE}/maintenance`);
  });

  it('passes equipment filters as query params', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ returned: 0, items: [] }));
    vi.stubGlobal('fetch', fetchMock);

    const { renderMaintenanceView } = await import('../../static/fleet/pages/maintenance.js');
    await renderMaintenanceView({ equipment_type: 'trailer', equipment_id: 'tr1' });
    await Promise.resolve();

    const url = fetchMock.mock.calls[0][0];
    expect(url).toContain('equipment_type=trailer');
    expect(url).toContain('equipment_id=tr1');
  });
});

describe('appendMaintenanceHistory', () => {
  it('renders a table of entries for the equipment', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({
      returned: 1,
      items: [{ id: 'm9', service_date: '2026-05-01', category: 'tire', description: 'new tires', cost: 800, vendor: 'TireCo' }],
    }));
    vi.stubGlobal('fetch', fetchMock);

    const { appendMaintenanceHistory } = await import('../../static/fleet/pages/_maintenance-history.js');
    await appendMaintenanceHistory('truck', 't1');
    await Promise.resolve();

    const html = document.getElementById('main-content').innerHTML;
    expect(html).toContain('Maintenance History');
    expect(html).toContain('new tires');
    expect(html).toContain('$800.00');
    const url = fetchMock.mock.calls[0][0];
    expect(url).toContain('equipment_type=truck');
    expect(url).toContain('equipment_id=t1');
  });

  it('shows an empty state when there are no entries', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ returned: 0, items: [] }));
    vi.stubGlobal('fetch', fetchMock);

    const { appendMaintenanceHistory } = await import('../../static/fleet/pages/_maintenance-history.js');
    await appendMaintenanceHistory('trailer', 'x1');
    await Promise.resolve();

    expect(document.getElementById('main-content').innerHTML).toContain('No maintenance entries');
  });
});
```

> If `renderMaintenanceView` / `appendMaintenanceHistory` need more than one microtask tick to settle (nested awaits), add another `await Promise.resolve();` or use `await vi.waitFor(() => expect(...))`. Check how the existing trailer/load page tests await rendering and mirror it. If `hasScope` gating hides the add button, the history-table assertions still hold regardless of scope.

- [ ] **Step 2: Run the SPA tests**

Run: `npm test`
Expected: PASS (existing suite + new `maintenance.test.js`).

- [ ] **Step 3: Commit**

```bash
git add tests/fleet/maintenance.test.js
git commit -s -m "test(maintenance): SPA list + equipment-embed tests

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 15: Final verification

- [ ] **Step 1: Full Rust test + clippy**

Run: `cargo test`
Expected: all tests PASS.

Run: `cargo clippy --all-targets`
Expected: no new warnings on maintenance files. (Repo is hand-formatted — do NOT run `cargo fmt`; match surrounding style.)

- [ ] **Step 2: Full SPA test**

Run: `npm test`
Expected: all PASS.

- [ ] **Step 3: Manual run-through (optional)**

Boot the app, sign in, and verify: the Maintenance nav item lists entries and filters by category; an entry's detail page shows fields and Edit/Delete; a truck and a trailer detail page each show a "Maintenance History" section with a working "Add maintenance" deep link; create/edit/delete round-trip works.

- [ ] **Step 4: Open the PR**

```bash
git push -u origin worktree-maintenance-log-spec
gh pr create --fill --base main
```

---

## Self-Review Notes (author checklist — already applied)

- **Spec coverage:** model+enums (T1), table/schema (T2), CRUD ops incl. hard delete + equipment/category filter (T3), REST write w/ equipment-existence validation + inline embedding (T4), REST read w/ filters (T5), REST tests (T6), MCP tools + scopes + blob reverse-lookup (T7), MCP test (T8), llms.txt (T9), SPA list w/ filter (T10), form (T11), detail (T12), equipment embeds on truck+trailer (T13), Vitest (T14). All spec sections mapped.
- **Type consistency:** `MaintenanceRecord`, `EquipmentType`, `MaintenanceCategory`, `MaintenanceListItem`, `MaintenanceListResponse`, `list_maintenance(equipment_type, equipment_id, category, limit, offset)`, `apply_maintenance_create/patch`, scopes `maintenance:read|write|delete`, table `"maintenance"` — used identically across all tasks. Canonical Arrow column order stated once and reused in schema/empty-batch/to-batch/row-reader.
- **Compile-unit pairing:** T2+T3 (table+ops+startup index) and T4+T5 (writes+reads) each only compile together; the plan calls this out and commits them paired.
- **Known verify-points flagged inline:** `get_truck_by_id` name, `renderEntityList` `extraControls` support, form `disabled`/field-types, `navigate` query-param forwarding, utoipa explicit registration, MCP delete confirmation pattern in tests. Each has a stated fallback.
