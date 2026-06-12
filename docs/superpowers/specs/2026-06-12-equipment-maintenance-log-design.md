# Equipment Maintenance Log — Design

**Date:** 2026-06-12
**Status:** Approved (design phase)

## Summary

Add a CRUD entity for an **equipment maintenance log**. Each entry records a single
piece of completed maintenance work tied to exactly one piece of equipment — a truck
*or* a trailer. The system provides:

- A filterable maintenance list view (filter by equipment and category).
- A "Maintenance History" section on both truck and trailer detail views.
- Full CRUD across all three API surfaces (REST, MCP, SPA), following the existing
  per-entity trio convention.

Scope is **historical only**: each entry is a record of work already performed. There
is no scheduled / preventive / due-date tracking and no status workflow.

## Decisions

- **Historical only** — no `status` field, no scheduled/preventive/due tracking.
- **Polymorphic parent** via `equipment_type` (enum truck/trailer) + `equipment_id` (UUID),
  rather than two nullable typed FK columns.
- **Fixed-enum `category`** with an `other` escape hatch, for a clean filter dropdown.
- **Embeddings included** — entries are semantically searchable like trucks/trailers/loads.
- **Hard delete** — a maintenance entry is a correctable log row, not an operational
  asset with status history, so `delete_maintenance` removes the row (unlike the
  soft-delete/deactivate behavior of equipment tools).
- Maintenance gets **its own top-level nav page** *and* embeds on equipment detail views.

## Data Model — `src/models/maintenance.rs`

New entity following the standard trio (model / `db/*_ops.rs` / API).

### `MaintenanceRecord`

| field | type | notes |
|---|---|---|
| `id` | `Uuid` | primary key |
| `equipment_type` | `EquipmentType` (enum) | `truck` / `trailer` |
| `equipment_id` | `Uuid` | the truck/trailer this entry belongs to |
| `service_date` | `String` (ISO `YYYY-MM-DD`) | when the work was performed |
| `category` | `MaintenanceCategory` (enum) | see below |
| `description` | `String` | work performed (required) |
| `cost` | `Option<f64>` | total cost of the service |
| `odometer` | `Option<i64>` | equipment mileage at time of service |
| `vendor` | `Option<String>` | shop / vendor name |
| `invoice_ref` | `Option<String>` | invoice or reference number |
| `blob_ids` | `Vec<Uuid>` | attached invoices / receipts / photos |
| `embedding` | `Option<Vec<f32>>` | semantic-search vector (`#[serde(skip)]`) |
| `owner_id` | `i64` | multi-tenancy |
| `created_at` | `DateTime<Utc>` | |
| `updated_at` | `DateTime<Utc>` | |

### Enums

```rust
EquipmentType: Truck | Trailer
```

```rust
MaintenanceCategory:
  PreventiveMaintenance | Repair | Tire | Inspection | OilChange | Brakes | Other
```

Both enums implement `as_str()` and `FromStr` (returning `Err(String)` on unknown
input) and get roundtrip unit tests, mirroring `TrailerOwner` / `TrailerStatus`.

### `embedding_text()`

```
"{category} {description} {vendor}"
```

(unit-number context for the parent equipment is appended by the write handler when it
resolves the equipment for validation, so the embedding is searchable by unit number).

### DTOs

- `CreateMaintenanceRequest` — `equipment_type`, `equipment_id`, `service_date`,
  `category`, `description` required; `cost`, `odometer`, `vendor`, `invoice_ref`,
  `blob_ids` optional. Excludes server-managed fields (`embedding`, `owner_id`, timestamps).
- `UpdateMaintenanceRequest` — all content fields optional. `equipment_type` /
  `equipment_id` are **not** updatable (a row belongs to its equipment for life; correct
  by delete + recreate).
- `MaintenanceListItem` — projection for list responses, with optional `score: Option<f32>`
  for semantic-search results (`#[serde(skip_serializing_if = "Option::is_none")]`).
- `MaintenanceListResponse { returned: usize, items: Vec<MaintenanceListItem> }`.

## Persistence — `src/db/maintenance_ops.rs` + schema in `src/db/mod.rs`

- Add `maintenance_table: Table` to `DbClient`.
- `maintenance_schema(embed_dim)`, `open_or_create_maintenance(conn, embed_dim)`,
  `empty_maintenance_batch(schema, embed_dim)` — following the trailer equivalents.
  `blob_ids` stored as a JSON-encoded `Utf8` column (matching the trailer pattern), enums
  and `service_date` as `Utf8`, `cost` as `Float64`, `odometer` / `owner_id` as `Int64`,
  timestamps as `Utf8`.
- All mutations use the canonical `merge_insert` upsert keyed on `["id"]`. Initial create
  uses `table.add(...).execute()`.

### Query helpers

- `insert_maintenance(&record)`
- `get_maintenance_by_id(id) -> MaintenanceRecord` (`NotFound` if absent)
- `upsert_maintenance(&record)` (private; used by update)
- `update_maintenance_metadata(...)` — optional-field update, returns updated record
- `update_maintenance_embedding(id, embedding)`
- `delete_maintenance(id)` — hard delete
- `list_maintenance(owner_id, equipment_type: Option, equipment_id: Option, category: Option, limit, offset)`
  — `only_if` filters, always scoped by `owner_id`, ordered by `service_date` descending.
  The `(equipment_type, equipment_id)` filter powers both the filtered list view and the
  equipment-detail embeds.

The vector index is created with the small-table guard already used elsewhere (see PR #346
/ facility backfill) so it is skipped until the table has enough rows.

## API Surfaces

### REST read — `src/api/fleet_portal/data.rs`

- `GET /fleet/api/v1/maintenance` with
  `ListMaintenanceQuery { equipment_type?, equipment_id?, category?, limit?, offset? }`.
- `GET /fleet/api/v1/maintenance/{id}`.

### REST write — `src/api/fleet_portal/maintenance_writes.rs` (new)

- `create_maintenance` — validates that `equipment_id` resolves to a real truck/trailer of
  the declared `equipment_type` (`batch_get_trucks` / `batch_get_trailers` or the
  single-get helpers); generates the embedding best-effort inline via `embed_text`; persists.
- `update_maintenance` — updates allowed fields; refreshes embedding best-effort.
- `delete_maintenance` — hard delete.

Validation and side effects live in shared functions called by both the REST handlers and
the MCP tools, mirroring `trailer_writes.rs`.

### MCP — `src/api/fleet_portal/mcp.rs`

- Tools: `list_maintenance`, `get_maintenance`, `create_maintenance`, `update_maintenance`,
  `delete_maintenance`.
- Scope map: `list_maintenance | get_maintenance -> maintenance:read`;
  `create_maintenance | update_maintenance -> maintenance:write`;
  `delete_maintenance -> maintenance:delete`.
- `delete_maintenance` is a hard delete (not added to the soft-delete/deactivate set);
  its confirmation description reads "permanently delete the maintenance entry".
- Registered in the tool list and JSON schema blocks alongside the other entities.

### `llms.txt`

Add a short description of the maintenance surface and its filters.

## SPA — `static/fleet/`

- `pages/maintenance.js` (list), `pages/maintenance-detail.js`, `pages/maintenance-form.js`
  built on the existing `_list.js` / `_detail.js` / `_form.js` and `components/table.js` /
  `components/form.js` helpers.
- The list page has a filter bar: equipment type + unit selector and a category dropdown,
  driving `GET /maintenance?equipment_type=…&equipment_id=…&category=…`.
- **Equipment detail embeds:** `pages/truck-detail.js` and `pages/trailer-detail.js` each
  gain a "Maintenance History" section that fetches
  `GET /maintenance?equipment_type=…&equipment_id=…`, renders a compact table, and shows an
  "Add maintenance" button that deep-links to `maintenance-form` pre-filled with that
  equipment.
- `router.js` route + nav entry, scope-gated (`scope-gate.js`) like other pages.

## Testing

- **Rust unit tests** (in `models/maintenance.rs`): enum roundtrips for `EquipmentType`
  and `MaintenanceCategory`, `embedding` skipped in JSON, DTO serde.
- **Rust integration tests** (`tests/`): CRUD, equipment-filtered list, category filter,
  equipment-existence validation on create, hard-delete behavior — following the existing
  per-entity integration test files.
- **Vitest (happy-dom)**: list / detail / form pages and the equipment-detail embed,
  conforming to the fleet Phase-0 test toolkit.

## Out of Scope

- Scheduled / preventive maintenance, due dates, due-mileage, reminders, overdue surfacing.
- Any status workflow on entries.
- Cost rollups / reporting (the `cost` field is captured but not aggregated here).
- Migration/backfill of historical data from external sources.
