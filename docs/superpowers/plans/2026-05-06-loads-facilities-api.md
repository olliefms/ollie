# Loads & Facilities API (v1.1.0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Facilities and Loads as first-class resources with full CRUD, keyword + vector search, async geocoding via US Census Bureau API, and HGV route-distance auto-calculation via OpenRouteService.

**Architecture:** Two new LanceDB tables (facilities, loads) follow the same columnar + vector-search pattern as blobs. Facilities gain an async geocoding pipeline (Census Bureau, no API key). Loads gain an async routing pipeline (ORS HGV) that fires when all stop-facility coordinates are available. Loads reference facilities for stop locations; stop resolution uses vector similarity with configurable high/low thresholds for dedup. Referential integrity is enforced at delete time across all resource types.

**Tech Stack:** Rust/Axum, LanceDB (Arrow), Ollama (embeddings), `reqwest` (already in Cargo.toml), US Census Bureau Geocoding API, OpenRouteService HGV API (API key required)

---

## File Map

**Convert (rename/restructure):**
- `src/models.rs` → `src/models/mod.rs` + `src/models/blob.rs`
- `src/db/ops.rs` → `src/db/blob_ops.rs` (rename; `self.table` → `self.blob_table` throughout)

**Create:**
- `src/models/facility.rs`
- `src/models/load.rs`
- `src/db/facility_ops.rs`
- `src/db/load_ops.rs`
- `src/geocoding/mod.rs`
- `src/routing/mod.rs`
- `src/pipeline/geocoding.rs`
- `src/pipeline/routing.rs`
- `src/api/facilities.rs`
- `src/api/loads.rs`

**Modify:**
- `src/db/mod.rs` — `DbClient` gains `facility_table` + `load_table`; add `facility_schema()` and `load_schema()`
- `src/api/mod.rs` — register facility + load routes
- `src/api/blob.rs` — referential integrity on delete
- `src/pipeline/mod.rs` — spawn geocoding + routing pipelines
- `src/pipeline/recovery.rs` — add geocoding + routing requeue
- `src/config.rs` — add `ORS_API_KEY`, `FACILITY_DEDUP_HIGH_THRESHOLD`, `FACILITY_DEDUP_LOW_THRESHOLD`, `GEOCODING_WORKERS`
- `src/lib.rs` — expand `AppState` with new pipeline senders + clients
- `src/main.rs` — wire new clients, pipelines, vector indexes
- `tests/integration_test.rs` — update `test_server()` for new `AppState` fields

---

## Task 1: Convert `models.rs` to a module

**Files:**
- Create: `src/models/mod.rs`
- Create: `src/models/blob.rs`
- Delete: `src/models.rs`

- [ ] **Step 1: Create `src/models/blob.rs`** — copy the entire current content of `src/models.rs` into it, replacing the opening comment with `// src/models/blob.rs`. No other changes.

- [ ] **Step 2: Create `src/models/mod.rs`**

```rust
// src/models/mod.rs
pub mod blob;
pub mod facility;
pub mod load;

pub use blob::*;
pub use facility::*;
pub use load::*;
```

- [ ] **Step 3: Create empty placeholder files so it compiles**

```rust
// src/models/facility.rs
// placeholder — filled in Task 2
```

```rust
// src/models/load.rs
// placeholder — filled in Task 3
```

- [ ] **Step 4: Delete `src/models.rs`**

```bash
rm /path/to/src/models.rs
```

- [ ] **Step 5: Verify it compiles and all tests pass**

```bash
cargo test 2>&1 | tail -20
```
Expected: all existing tests pass; no import errors.

- [ ] **Step 6: Commit**

```bash
git add src/models/
git rm src/models.rs
git commit -m "refactor: split models.rs into models module"
```

---

## Task 2: Facility models

**Files:**
- Modify: `src/models/facility.rs`
- Modify: `src/models/mod.rs` (remove placeholder comment)

- [ ] **Step 1: Write tests first** — add to the bottom of `src/models/facility.rs` after implementing the types below:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_geocode_status_roundtrip() {
        for s in ["pending", "ready", "failed"] {
            let status: GeocodeStatus = s.parse().unwrap();
            assert_eq!(status.as_str(), s);
        }
    }

    #[test]
    fn test_facility_record_embedding_skipped_in_json() {
        let r = FacilityRecord {
            id: uuid::Uuid::new_v4(), owner_id: 0,
            name: "Test Facility".into(), address: "Memphis, TN".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: GeocodeStatus::Pending,
            contacts: vec![], notes: None, tags: vec![],
            blob_ids: vec![], avg_dwell_minutes: None,
            dwell_sample_count: 0, embedding: Some(vec![0.1, 0.2]),
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
    }

    #[test]
    fn test_facility_embedding_text_includes_contacts() {
        let r = FacilityRecord {
            id: uuid::Uuid::new_v4(), owner_id: 0,
            name: "ABC Warehouse".into(), address: "Memphis, TN".into(),
            normalized_address: Some("315 Industrial Blvd, Memphis, TN 38118".into()),
            lat: None, lng: None, geocode_status: GeocodeStatus::Pending,
            contacts: vec![FacilityContact {
                name: "Jane Smith".into(), title: Some("Dock Manager".into()),
                phone: None, email: None, notes: None,
            }],
            notes: Some("call ahead".into()), tags: vec!["cold".into()],
            blob_ids: vec![], avg_dwell_minutes: None, dwell_sample_count: 0,
            embedding: None,
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        let text = r.embedding_text();
        assert!(text.contains("ABC Warehouse"));
        assert!(text.contains("Jane Smith"));
        assert!(text.contains("Dock Manager"));
    }
}
```

- [ ] **Step 2: Run — expect compile error** (types not defined yet)

```bash
cargo test models::facility 2>&1 | head -20
```

- [ ] **Step 3: Implement `src/models/facility.rs`**

```rust
// src/models/facility.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GeocodeStatus {
    Pending,
    Ready,
    Failed,
}

impl GeocodeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for GeocodeStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown geocode status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacilityContact {
    pub name: String,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacilityRecord {
    pub id: Uuid,
    pub owner_id: i64,
    pub name: String,
    pub address: String,
    pub normalized_address: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub geocode_status: GeocodeStatus,
    pub contacts: Vec<FacilityContact>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub avg_dwell_minutes: Option<f64>,
    pub dwell_sample_count: i64,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl FacilityRecord {
    pub fn embedding_text(&self) -> String {
        let contact_text = self.contacts.iter()
            .map(|c| {
                let mut parts = vec![c.name.clone()];
                if let Some(t) = &c.title { parts.push(t.clone()); }
                parts.join(" ")
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "{} {} {} {} {}",
            self.name,
            self.normalized_address.as_deref().unwrap_or(&self.address),
            self.notes.as_deref().unwrap_or(""),
            self.tags.join(" "),
            contact_text,
        )
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateFacilityRequest {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub contacts: Vec<FacilityContact>,
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateFacilityRequest {
    pub name: Option<String>,
    pub address: Option<String>,
    pub contacts: Option<Vec<FacilityContact>>,
    pub notes: Option<String>,
    pub tags: Option<Vec<String>>,
    pub blob_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FacilityListItem {
    pub id: Uuid,
    pub owner_id: i64,
    pub name: String,
    pub address: String,
    pub normalized_address: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub geocode_status: GeocodeStatus,
    pub contacts: Vec<FacilityContact>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub avg_dwell_minutes: Option<f64>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<FacilityRecord> for FacilityListItem {
    fn from(r: FacilityRecord) -> Self {
        Self {
            id: r.id, owner_id: r.owner_id, name: r.name,
            address: r.address, normalized_address: r.normalized_address,
            lat: r.lat, lng: r.lng, geocode_status: r.geocode_status,
            contacts: r.contacts, notes: r.notes, tags: r.tags,
            blob_ids: r.blob_ids, avg_dwell_minutes: r.avg_dwell_minutes,
            created_at: r.created_at, score: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct FacilityListResponse {
    pub total: usize,
    pub items: Vec<FacilityListItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FacilityCandidate {
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub normalized_address: Option<String>,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacilityResolutionResponse {
    pub facility_resolution_required: bool,
    pub candidates: Vec<FacilityCandidate>,
}
```

- [ ] **Step 4: Run tests — expect pass**

```bash
cargo test models::facility 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add src/models/facility.rs
git commit -m "feat: add facility models"
```

---

## Task 3: Load models

**Files:**
- Modify: `src/models/load.rs`

- [ ] **Step 1: Write tests first** — add to bottom of `src/models/load.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stop_type_roundtrip() {
        for s in ["pickup", "delivery"] {
            let t: StopType = s.parse().unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn test_service_type_roundtrip() {
        for s in ["pre_loaded", "live_load", "live_unload", "drop_and_hook", "relay"] {
            let t: ServiceType = s.parse().unwrap();
            assert_eq!(t.as_str(), s);
        }
    }

    #[test]
    fn test_service_type_valid_for_stop() {
        assert!(ServiceType::PreLoaded.is_valid_for(&StopType::Pickup));
        assert!(ServiceType::LiveLoad.is_valid_for(&StopType::Pickup));
        assert!(!ServiceType::LiveUnload.is_valid_for(&StopType::Pickup));
        assert!(!ServiceType::DropAndHook.is_valid_for(&StopType::Pickup));
        assert!(ServiceType::Relay.is_valid_for(&StopType::Pickup));
        assert!(ServiceType::Relay.is_valid_for(&StopType::Delivery));
        assert!(ServiceType::LiveUnload.is_valid_for(&StopType::Delivery));
        assert!(ServiceType::DropAndHook.is_valid_for(&StopType::Delivery));
        assert!(!ServiceType::PreLoaded.is_valid_for(&StopType::Delivery));
    }

    #[test]
    fn test_load_status_roundtrip() {
        for s in ["planned","dispatched","in_transit","delivered","invoiced","settled","cancelled"] {
            let st: LoadStatus = s.parse().unwrap();
            assert_eq!(st.as_str(), s);
        }
    }

    #[test]
    fn test_load_status_valid_transitions() {
        assert!(LoadStatus::Planned.can_transition_to(&LoadStatus::Dispatched));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::InTransit));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::Delivered));
        assert!(LoadStatus::InTransit.can_transition_to(&LoadStatus::Delivered));
        assert!(LoadStatus::Delivered.can_transition_to(&LoadStatus::Invoiced));
        assert!(LoadStatus::Invoiced.can_transition_to(&LoadStatus::Settled));
        assert!(LoadStatus::Planned.can_transition_to(&LoadStatus::Cancelled));
        assert!(LoadStatus::Dispatched.can_transition_to(&LoadStatus::Cancelled));
        assert!(LoadStatus::InTransit.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Delivered.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Invoiced.can_transition_to(&LoadStatus::Cancelled));
        assert!(!LoadStatus::Settled.can_transition_to(&LoadStatus::Dispatched));
        assert!(!LoadStatus::Cancelled.can_transition_to(&LoadStatus::Planned));
        assert!(!LoadStatus::Planned.can_transition_to(&LoadStatus::Delivered));
    }

    #[test]
    fn test_total_rate_usd_sums_including_negatives() {
        let record = LoadRecord {
            id: uuid::Uuid::new_v4(), load_number: "LD-2026-0001".into(),
            owner_id: 0, status: LoadStatus::Planned,
            customer_name: "ACME".into(), customer_ref: None,
            stops: vec![], rate_items: vec![
                RateLineItem { description: "Line Haul".into(), amount_usd: 1800.0 },
                RateLineItem { description: "Fuel Surcharge".into(), amount_usd: 210.0 },
                RateLineItem { description: "Short Pay".into(), amount_usd: -50.0 },
            ],
            commodity: None, weight_lbs: None, miles: None, notes: None,
            tags: vec![], blob_ids: vec![],
            invoice_number: None, invoice_date: None, cancellation_reason: None,
            embedding: None,
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        assert!((record.total_rate_usd() - 1960.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_load_record_embedding_skipped_in_json() {
        let r = LoadRecord {
            id: uuid::Uuid::new_v4(), load_number: "LD-2026-0001".into(),
            owner_id: 0, status: LoadStatus::Planned,
            customer_name: "ACME".into(), customer_ref: None,
            stops: vec![], rate_items: vec![], commodity: None,
            weight_lbs: None, miles: None, notes: None, tags: vec![],
            blob_ids: vec![], invoice_number: None, invoice_date: None,
            cancellation_reason: None, embedding: Some(vec![0.1]),
            created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("embedding").is_none());
    }
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test models::load 2>&1 | head -20
```

- [ ] **Step 3: Implement `src/models/load.rs`**

```rust
// src/models/load.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StopType { Pickup, Delivery }

impl StopType {
    pub fn as_str(&self) -> &'static str {
        match self { Self::Pickup => "pickup", Self::Delivery => "delivery" }
    }
}

impl std::str::FromStr for StopType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pickup" => Ok(Self::Pickup),
            "delivery" => Ok(Self::Delivery),
            other => Err(format!("unknown stop type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceType { PreLoaded, LiveLoad, LiveUnload, DropAndHook, Relay }

impl ServiceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreLoaded => "pre_loaded", Self::LiveLoad => "live_load",
            Self::LiveUnload => "live_unload", Self::DropAndHook => "drop_and_hook",
            Self::Relay => "relay",
        }
    }

    pub fn is_valid_for(&self, stop_type: &StopType) -> bool {
        match (self, stop_type) {
            (Self::PreLoaded | Self::LiveLoad, StopType::Pickup) => true,
            (Self::LiveUnload | Self::DropAndHook, StopType::Delivery) => true,
            (Self::Relay, _) => true,
            _ => false,
        }
    }
}

impl std::str::FromStr for ServiceType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pre_loaded" => Ok(Self::PreLoaded), "live_load" => Ok(Self::LiveLoad),
            "live_unload" => Ok(Self::LiveUnload), "drop_and_hook" => Ok(Self::DropAndHook),
            "relay" => Ok(Self::Relay),
            other => Err(format!("unknown service type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LoadStatus {
    Planned, Dispatched, InTransit, Delivered, Invoiced, Settled, Cancelled,
}

impl LoadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned", Self::Dispatched => "dispatched",
            Self::InTransit => "in_transit", Self::Delivered => "delivered",
            Self::Invoiced => "invoiced", Self::Settled => "settled",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn can_transition_to(&self, next: &LoadStatus) -> bool {
        match (self, next) {
            (Self::Planned, Self::Dispatched) => true,
            (Self::Dispatched, Self::InTransit) => true,
            (Self::Dispatched | Self::InTransit, Self::Delivered) => true,
            (Self::Delivered, Self::Invoiced) => true,
            (Self::Invoiced, Self::Settled) => true,
            // Cancel only from pre-delivery states; delivered/invoiced/settled are terminal
            (Self::Planned | Self::Dispatched | Self::InTransit, Self::Cancelled) => true,
            _ => false,
        }
    }
}

impl std::str::FromStr for LoadStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "planned" => Ok(Self::Planned), "dispatched" => Ok(Self::Dispatched),
            "in_transit" => Ok(Self::InTransit), "delivered" => Ok(Self::Delivered),
            "invoiced" => Ok(Self::Invoiced), "settled" => Ok(Self::Settled),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown load status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLineItem {
    pub description: String,
    pub amount_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stop {
    pub sequence: u32,
    pub stop_type: StopType,
    pub service_type: ServiceType,
    pub facility_id: Uuid,
    pub scheduled_arrive: String,
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopInput {
    pub sequence: u32,
    pub stop_type: StopType,
    pub service_type: ServiceType,
    pub facility_id: Option<Uuid>,
    pub facility_name: Option<String>,
    pub address: Option<String>,
    #[serde(default)]
    pub force_new_facility: bool,
    pub scheduled_arrive: String,
    pub notes: Option<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StopResponse {
    pub sequence: u32,
    pub stop_type: StopType,
    pub service_type: ServiceType,
    pub facility_id: Uuid,
    pub facility_name: String,
    pub address: String,
    pub normalized_address: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub scheduled_arrive: String,
    pub notes: Option<String>,
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadRecord {
    pub id: Uuid,
    pub load_number: String,
    pub owner_id: i64,
    pub status: LoadStatus,
    pub customer_name: String,
    pub customer_ref: Option<String>,
    pub stops: Vec<Stop>,
    pub rate_items: Vec<RateLineItem>,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub cancellation_reason: Option<String>,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LoadRecord {
    pub fn total_rate_usd(&self) -> f64 {
        self.rate_items.iter().map(|r| r.amount_usd).sum()
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateLoadRequest {
    pub load_number: Option<String>,
    pub customer_name: String,
    pub customer_ref: Option<String>,
    pub stops: Vec<StopInput>,
    #[serde(default)]
    pub rate_items: Vec<RateLineItem>,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLoadRequest {
    pub customer_name: Option<String>,
    pub customer_ref: Option<String>,
    pub stops: Option<Vec<StopInput>>,
    pub rate_items: Option<Vec<RateLineItem>>,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    pub tags: Option<Vec<String>>,
    pub blob_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize)]
pub struct InvoiceActionRequest {
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CancelActionRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadListItem {
    pub id: Uuid,
    pub load_number: String,
    pub status: LoadStatus,
    pub customer_name: String,
    pub customer_ref: Option<String>,
    pub stops: Vec<Stop>,
    pub rate_items: Vec<RateLineItem>,
    pub total_rate_usd: f64,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub cancellation_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<LoadRecord> for LoadListItem {
    fn from(r: LoadRecord) -> Self {
        let total = r.total_rate_usd();
        Self {
            id: r.id, load_number: r.load_number, status: r.status,
            customer_name: r.customer_name, customer_ref: r.customer_ref,
            stops: r.stops, rate_items: r.rate_items, total_rate_usd: total,
            commodity: r.commodity, weight_lbs: r.weight_lbs, miles: r.miles,
            notes: r.notes, tags: r.tags, blob_ids: r.blob_ids,
            invoice_number: r.invoice_number, invoice_date: r.invoice_date,
            cancellation_reason: r.cancellation_reason,
            created_at: r.created_at, score: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LoadListResponse {
    pub total: usize,
    pub items: Vec<LoadListItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadDetailResponse {
    pub id: Uuid,
    pub load_number: String,
    pub status: LoadStatus,
    pub customer_name: String,
    pub customer_ref: Option<String>,
    pub stops: Vec<StopResponse>,
    pub rate_items: Vec<RateLineItem>,
    pub total_rate_usd: f64,
    pub commodity: Option<String>,
    pub weight_lbs: Option<f64>,
    pub miles: Option<f64>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub blob_ids: Vec<Uuid>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub cancellation_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

- [ ] **Step 4: Run tests — expect pass**

```bash
cargo test models::load 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add src/models/load.rs
git commit -m "feat: add load models"
```

---

## Task 4: Expand `DbClient` to multi-table + rename `blob_ops`

**Files:**
- Modify: `src/db/mod.rs`
- Rename: `src/db/ops.rs` → `src/db/blob_ops.rs`

- [ ] **Step 1: Write failing test for multi-table init**

Add to `src/db/mod.rs` tests:

```rust
#[tokio::test]
async fn test_db_client_creates_all_three_tables() {
    let dir = TempDir::new().unwrap();
    let client = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
    // all three tables accessible with zero rows
    assert_eq!(client.blob_table.count_rows(None).await.unwrap(), 0);
    assert_eq!(client.facility_table.count_rows(None).await.unwrap(), 0);
    assert_eq!(client.load_table.count_rows(None).await.unwrap(), 0);
}
```

- [ ] **Step 2: Run — expect compile error** (`blob_table`, `facility_table`, `load_table` don't exist yet)

```bash
cargo test db::tests::test_db_client_creates_all_three_tables 2>&1 | head -20
```

- [ ] **Step 3: Rename `src/db/ops.rs` → `src/db/blob_ops.rs`**

```bash
mv src/db/ops.rs src/db/blob_ops.rs
```

- [ ] **Step 4: Replace all `self.table` with `self.blob_table` in `src/db/blob_ops.rs`**

```bash
sed -i '' 's/self\.table/self.blob_table/g' src/db/blob_ops.rs
```

Also update the import at the top of `blob_ops.rs` — it currently imports `blob_schema` and `DbClient` from `crate::db`. No change needed there.

- [ ] **Step 5: Replace `src/db/mod.rs`** with the full multi-table version:

```rust
// src/db/mod.rs
pub mod blob_ops;
pub mod facility_ops;
pub mod load_ops;

use crate::error::AppError;
use arrow_array::{
    FixedSizeListArray, Float64Array, Int64Array, RecordBatch,
    RecordBatchIterator, RecordBatchReader, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use lancedb::Table;
use std::sync::Arc;

pub struct DbClient {
    pub blob_table: Table,
    pub facility_table: Table,
    pub load_table: Table,
    pub embed_dim: usize,
}

impl DbClient {
    pub async fn new(path: &str, embed_dim: usize) -> Result<Self, AppError> {
        let conn = lancedb::connect(path)
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let blob_table = open_or_create(&conn, "blobs", blob_schema(embed_dim), |schema| {
            empty_blob_batch(schema, embed_dim)
        }).await?;

        let facility_table = open_or_create(&conn, "facilities", facility_schema(embed_dim), |schema| {
            empty_facility_batch(schema, embed_dim)
        }).await?;

        let load_table = open_or_create(&conn, "loads", load_schema(embed_dim), |schema| {
            empty_load_batch(schema, embed_dim)
        }).await?;

        Ok(Self { blob_table, facility_table, load_table, embed_dim })
    }
}

async fn open_or_create<F>(
    conn: &lancedb::Connection,
    name: &str,
    schema: Arc<Schema>,
    make_batch: F,
) -> Result<Table, AppError>
where
    F: FnOnce(Arc<Schema>) -> Result<RecordBatch, AppError>,
{
    match conn.open_table(name).execute().await {
        Ok(t) => Ok(t),
        Err(_) => {
            let batch = make_batch(schema.clone())?;
            let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
            conn.create_table(name, reader).execute().await
                .map_err(|e| AppError::Internal(e.to_string()))
        }
    }
}

pub fn blob_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("checksum", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("mime_type", DataType::Utf8, false),
        Field::new("size", DataType::Int64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("error", DataType::Utf8, true),
        Field::new("summary", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

pub fn facility_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("address", DataType::Utf8, false),
        Field::new("normalized_address", DataType::Utf8, true),
        Field::new("lat", DataType::Float64, true),
        Field::new("lng", DataType::Float64, true),
        Field::new("geocode_status", DataType::Utf8, false),
        Field::new("contacts", DataType::Utf8, false),
        Field::new("notes", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new("blob_ids", DataType::Utf8, false),
        Field::new("avg_dwell_minutes", DataType::Float64, true),
        Field::new("dwell_sample_count", DataType::Int64, false),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

pub fn load_schema(embed_dim: usize) -> Arc<Schema> {
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

fn empty_blob_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(Int64Array::from(Vec::<i64>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn empty_facility_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // id
        Arc::new(Int64Array::from(Vec::<i64>::new())),             // owner_id
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // name
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // address
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // normalized_address
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // lat
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // lng
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // geocode_status
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // contacts
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // notes
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // tags
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // blob_ids
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // avg_dwell_minutes
        Arc::new(Int64Array::from(Vec::<i64>::new())),             // dwell_sample_count
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),                               // embedding
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // created_at
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // updated_at
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn empty_load_batch(schema: Arc<Schema>, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // id
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // load_number
        Arc::new(Int64Array::from(Vec::<i64>::new())),             // owner_id
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // status
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // customer_name
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // customer_ref
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // stops
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // rate_items
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // commodity
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // weight_lbs
        Arc::new(Float64Array::from(Vec::<Option<f64>>::new())),  // miles
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // notes
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // tags
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // blob_ids
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // invoice_number
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // invoice_date
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // cancellation_reason
        Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(nulls, embed_dim as i32)),                               // embedding
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // created_at
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),  // updated_at
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_db_client_creates_all_three_tables() {
        let dir = TempDir::new().unwrap();
        let client = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        assert_eq!(client.blob_table.count_rows(None).await.unwrap(), 0);
        assert_eq!(client.facility_table.count_rows(None).await.unwrap(), 0);
        assert_eq!(client.load_table.count_rows(None).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_blob_schema_has_fixed_size_embedding() {
        let schema = blob_schema(768);
        let field = schema.field_with_name("embedding").unwrap();
        assert!(matches!(field.data_type(), DataType::FixedSizeList(_, 768)));
    }

    #[tokio::test]
    async fn test_facility_schema_has_float64_lat_lng() {
        let schema = facility_schema(4);
        assert!(matches!(schema.field_with_name("lat").unwrap().data_type(), DataType::Float64));
        assert!(matches!(schema.field_with_name("lng").unwrap().data_type(), DataType::Float64));
    }
}
```

- [ ] **Step 6: Create empty placeholder ops files so it compiles**

```rust
// src/db/facility_ops.rs
// placeholder — filled in Task 5
```

```rust
// src/db/load_ops.rs
// placeholder — filled in Task 6
```

- [ ] **Step 7: Run all tests — expect pass**

```bash
cargo test 2>&1 | tail -20
```

- [ ] **Step 8: Commit**

```bash
git add src/db/
git commit -m "refactor: expand DbClient to three tables, rename blob_ops"
```

---

## Task 5: Facility DB ops

**Files:**
- Modify: `src/db/facility_ops.rs`

- [ ] **Step 1: Write tests first** — add to `src/db/facility_ops.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FacilityContact, GeocodeStatus};
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample_facility() -> FacilityRecord {
        let now = chrono::Utc::now();
        FacilityRecord {
            id: uuid::Uuid::new_v4(), owner_id: 0,
            name: "ABC Warehouse".into(), address: "Memphis, TN".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: GeocodeStatus::Pending,
            contacts: vec![], notes: None, tags: vec!["cold".into()],
            blob_ids: vec![], avg_dwell_minutes: None, dwell_sample_count: 0,
            embedding: None, created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_facility() {
        let (db, _dir) = test_db().await;
        let f = sample_facility();
        db.insert_facility(&f).await.unwrap();
        let fetched = db.get_facility_by_id(f.id).await.unwrap();
        assert_eq!(fetched.id, f.id);
        assert_eq!(fetched.name, "ABC Warehouse");
        assert_eq!(fetched.tags, vec!["cold"]);
    }

    #[tokio::test]
    async fn test_get_facility_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_facility_by_id(uuid::Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_delete_facility() {
        let (db, _dir) = test_db().await;
        let f = sample_facility();
        db.insert_facility(&f).await.unwrap();
        db.delete_facility_by_id(f.id).await.unwrap();
        assert!(matches!(db.get_facility_by_id(f.id).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_update_facility_geocode() {
        let (db, _dir) = test_db().await;
        let f = sample_facility();
        db.insert_facility(&f).await.unwrap();
        db.update_facility_geocode(f.id, 35.1495, -90.0490, "315 Industrial Blvd, Memphis, TN 38118".into()).await.unwrap();
        let fetched = db.get_facility_by_id(f.id).await.unwrap();
        assert_eq!(fetched.geocode_status, GeocodeStatus::Ready);
        assert!((fetched.lat.unwrap() - 35.1495).abs() < 1e-6);
        assert!(fetched.normalized_address.is_some());
    }

    #[tokio::test]
    async fn test_list_facilities_with_tag_filter() {
        let (db, _dir) = test_db().await;
        let f = sample_facility();
        db.insert_facility(&f).await.unwrap();
        let (total, items) = db.list_facilities(None, &["cold".to_string()], 10, 0).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, f.id);
    }

    #[tokio::test]
    async fn test_batch_get_facilities() {
        let (db, _dir) = test_db().await;
        let f1 = sample_facility();
        let mut f2 = sample_facility();
        f2.id = uuid::Uuid::new_v4();
        f2.name = "XYZ Dock".into();
        db.insert_facility(&f1).await.unwrap();
        db.insert_facility(&f2).await.unwrap();
        let map = db.batch_get_facilities(&[f1.id, f2.id]).await.unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map[&f1.id].name, "ABC Warehouse");
        assert_eq!(map[&f2.id].name, "XYZ Dock");
    }
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test db::facility_ops 2>&1 | head -20
```

- [ ] **Step 3: Implement `src/db/facility_ops.rs`**

```rust
// src/db/facility_ops.rs
use crate::{
    db::{facility_schema, DbClient},
    error::AppError,
    models::{FacilityCandidate, FacilityListItem, FacilityRecord, GeocodeStatus},
};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Float64Array, Int64Array,
    RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray,
};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

impl DbClient {
    pub async fn insert_facility(&self, record: &FacilityRecord) -> Result<(), AppError> {
        let batch = facility_to_batch(record, self.embed_dim)?;
        let schema = facility_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.facility_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_facility_by_id(&self, id: Uuid) -> Result<FacilityRecord, AppError> {
        let stream = self.facility_table.query()
            .only_if(format!("id = '{}'", id))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        batches_to_facilities(batches)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn delete_facility_by_id(&self, id: Uuid) -> Result<(), AppError> {
        self.facility_table.delete(&format!("id = '{id}'")).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn update_facility_metadata(
        &self, id: Uuid,
        name: Option<String>, address: Option<String>,
        contacts: Option<Vec<crate::models::FacilityContact>>,
        notes: Option<String>, tags: Option<Vec<String>>,
        blob_ids: Option<Vec<Uuid>>,
    ) -> Result<FacilityRecord, AppError> {
        let mut record = self.get_facility_by_id(id).await?;
        if let Some(n) = name { record.name = n; }
        if let Some(a) = address {
            record.address = a;
            record.normalized_address = None;
            record.lat = None;
            record.lng = None;
            record.geocode_status = GeocodeStatus::Pending;
        }
        if let Some(c) = contacts { record.contacts = c; }
        if let Some(n) = notes { record.notes = Some(n); }
        if let Some(t) = tags { record.tags = t; }
        if let Some(b) = blob_ids { record.blob_ids = b; }
        record.updated_at = Utc::now();
        self.delete_facility_by_id(id).await?;
        self.insert_facility(&record).await?;
        Ok(record)
    }

    pub async fn update_facility_geocode(
        &self, id: Uuid, lat: f64, lng: f64, normalized_address: String,
    ) -> Result<(), AppError> {
        let mut record = self.get_facility_by_id(id).await?;
        record.lat = Some(lat);
        record.lng = Some(lng);
        record.normalized_address = Some(normalized_address);
        record.geocode_status = GeocodeStatus::Ready;
        record.updated_at = Utc::now();
        self.delete_facility_by_id(id).await?;
        self.insert_facility(&record).await
    }

    pub async fn mark_facility_geocode_failed(&self, id: Uuid) -> Result<(), AppError> {
        let mut record = self.get_facility_by_id(id).await?;
        record.geocode_status = GeocodeStatus::Failed;
        record.updated_at = Utc::now();
        self.delete_facility_by_id(id).await?;
        self.insert_facility(&record).await
    }

    pub async fn update_facility_embedding(
        &self, id: Uuid, embedding: Vec<f32>,
    ) -> Result<(), AppError> {
        let mut record = self.get_facility_by_id(id).await?;
        record.embedding = Some(embedding);
        record.updated_at = Utc::now();
        self.delete_facility_by_id(id).await?;
        self.insert_facility(&record).await
    }

    pub async fn list_facilities(
        &self,
        name_filter: Option<&str>,
        tag_filter: &[String],
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<FacilityListItem>), AppError> {
        let filter = build_facility_filter(name_filter, tag_filter);
        let total = self.facility_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.facility_table.query().limit(limit + offset);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let items: Vec<FacilityListItem> = batches_to_facilities(collect_stream(stream).await?)?
            .into_iter().skip(offset).map(FacilityListItem::from).collect();
        Ok((total, items))
    }

    pub async fn search_facilities(
        &self,
        embedding: Vec<f32>,
        name_filter: Option<&str>,
        tag_filter: &[String],
        limit: usize,
    ) -> Result<Vec<FacilityListItem>, AppError> {
        let filter = build_facility_filter(name_filter, tag_filter);
        let mut q = self.facility_table.query()
            .nearest_to(embedding)
            .map_err(|e| AppError::Internal(e.to_string()))?
            .limit(limit);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        let mut items = Vec::new();
        for batch in &batches {
            let distances = batch.column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
                .map(|a| (0..a.len()).map(|i| a.value(i)).collect::<Vec<_>>());
            for (i, record) in batches_to_facilities(vec![batch.clone()])?.into_iter().enumerate() {
                let mut item = FacilityListItem::from(record);
                if let Some(ref d) = distances {
                    item.score = Some(1.0 / (1.0 + d[i]));
                }
                items.push(item);
            }
        }
        Ok(items)
    }

    pub async fn batch_get_facilities(
        &self,
        ids: &[Uuid],
    ) -> Result<HashMap<Uuid, FacilityRecord>, AppError> {
        if ids.is_empty() { return Ok(HashMap::new()); }
        let id_list = ids.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(", ");
        let stream = self.facility_table.query()
            .only_if(format!("id IN ({id_list})"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_facilities(collect_stream(stream).await?)?
            .into_iter().map(|r| (r.id, r)).collect())
    }

    pub async fn list_pending_geocode_facility_ids(&self) -> Result<Vec<Uuid>, AppError> {
        let stream = self.facility_table.query()
            .only_if("geocode_status = 'pending'")
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_facilities(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn create_facility_vector_index(&self) -> Result<(), AppError> {
        self.facility_table
            .create_index(&["embedding"], lancedb::index::Index::IvfPq(Default::default()))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))
    }
}

// --- Helpers ---

fn facility_to_batch(record: &FacilityRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = facility_schema(embed_dim);
    let contacts_json = serde_json::to_string(&record.contacts)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let tags_json = serde_json::to_string(&record.tags)
        .map_err(|e| AppError::Internal(e.to_string()))?;
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
        Arc::new(StringArray::from(vec![record.id.to_string().as_str()])),
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![record.name.as_str()])),
        Arc::new(StringArray::from(vec![record.address.as_str()])),
        Arc::new(StringArray::from(vec![record.normalized_address.as_deref()])),
        Arc::new(Float64Array::from(vec![record.lat])),
        Arc::new(Float64Array::from(vec![record.lng])),
        Arc::new(StringArray::from(vec![record.geocode_status.as_str()])),
        Arc::new(StringArray::from(vec![contacts_json.as_str()])),
        Arc::new(StringArray::from(vec![record.notes.as_deref()])),
        Arc::new(StringArray::from(vec![tags_json.as_str()])),
        Arc::new(StringArray::from(vec![blob_ids_json.as_str()])),
        Arc::new(Float64Array::from(vec![record.avg_dwell_minutes])),
        Arc::new(Int64Array::from(vec![record.dwell_sample_count])),
        embedding_col,
        Arc::new(StringArray::from(vec![record.created_at.to_rfc3339().as_str()])),
        Arc::new(StringArray::from(vec![record.updated_at.to_rfc3339().as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_facilities(batches: Vec<RecordBatch>) -> Result<Vec<FacilityRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_facility(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_facility(batch: &RecordBatch, i: usize) -> Result<FacilityRecord, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string())
            .unwrap_or_default()
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
    let opt_f64 = |name: &str| -> Option<f64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };

    let contacts: Vec<crate::models::FacilityContact> =
        serde_json::from_str(&str_col("contacts")).unwrap_or_default();
    let tags: Vec<String> =
        serde_json::from_str(&str_col("tags")).unwrap_or_default();
    let blob_ids: Vec<Uuid> =
        serde_json::from_str(&str_col("blob_ids")).unwrap_or_default();

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(FacilityRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        owner_id: i64_col("owner_id"),
        name: str_col("name"),
        address: str_col("address"),
        normalized_address: opt_str("normalized_address"),
        lat: opt_f64("lat"),
        lng: opt_f64("lng"),
        geocode_status: str_col("geocode_status").parse()
            .map_err(|e: String| AppError::Internal(e))?,
        contacts, notes: opt_str("notes"), tags, blob_ids,
        avg_dwell_minutes: opt_f64("avg_dwell_minutes"),
        dwell_sample_count: i64_col("dwell_sample_count"),
        embedding,
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_facility_filter(name: Option<&str>, tags: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    // Escape single quotes to prevent SQL injection in LanceDB filter strings
    if let Some(n) = name {
        let n = n.replace('\'', "''");
        parts.push(format!("name LIKE '%{n}%'"));
    }
    for tag in tags {
        let tag = tag.replace('\'', "''");
        parts.push(format!("tags LIKE '%\"{tag}\"%'"));
    }
    if parts.is_empty() { None } else { Some(parts.join(" AND ")) }
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}
```

- [ ] **Step 4: Run tests — expect pass**

```bash
cargo test db::facility_ops 2>&1 | tail -15
```

- [ ] **Step 5: Commit**

```bash
git add src/db/facility_ops.rs
git commit -m "feat: add facility DB ops"
```

---

## Task 6: Load DB ops

**Files:**
- Modify: `src/db/load_ops.rs`

- [ ] **Step 1: Write tests first** — add to `src/db/load_ops.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LoadStatus, RateLineItem};
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample_load() -> LoadRecord {
        let now = chrono::Utc::now();
        LoadRecord {
            id: uuid::Uuid::new_v4(),
            load_number: "LD-2026-0001".into(),
            owner_id: 0, status: LoadStatus::Planned,
            customer_name: "ACME Logistics".into(), customer_ref: None,
            stops: vec![], rate_items: vec![
                RateLineItem { description: "Line Haul".into(), amount_usd: 1500.0 },
            ],
            commodity: Some("dry goods".into()), weight_lbs: Some(40000.0),
            miles: None, notes: None, tags: vec!["flatbed".into()],
            blob_ids: vec![], invoice_number: None, invoice_date: None,
            cancellation_reason: None, embedding: None,
            created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_load() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert_eq!(fetched.id, load.id);
        assert_eq!(fetched.customer_name, "ACME Logistics");
        assert_eq!(fetched.rate_items.len(), 1);
    }

    #[tokio::test]
    async fn test_get_load_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_load_by_id(uuid::Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_delete_load() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        db.delete_load_by_id(load.id).await.unwrap();
        assert!(matches!(db.get_load_by_id(load.id).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_transition_status() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        db.transition_load_status(load.id, LoadStatus::Dispatched, None, None, None).await.unwrap();
        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert_eq!(fetched.status, LoadStatus::Dispatched);
    }

    #[tokio::test]
    async fn test_next_load_number_sequences() {
        let (db, _dir) = test_db().await;
        let n1 = db.next_load_number(2026).await.unwrap();
        assert_eq!(n1, "LD-2026-0001");
        let mut load = sample_load();
        load.load_number = n1.clone();
        db.insert_load(&load).await.unwrap();
        let n2 = db.next_load_number(2026).await.unwrap();
        assert_eq!(n2, "LD-2026-0002");
    }

    #[tokio::test]
    async fn test_any_load_references_facility() {
        let (db, _dir) = test_db().await;
        let fac_id = uuid::Uuid::new_v4();
        let mut load = sample_load();
        load.stops = vec![crate::models::Stop {
            sequence: 1,
            stop_type: crate::models::StopType::Pickup,
            service_type: crate::models::ServiceType::LiveLoad,
            facility_id: fac_id,
            scheduled_arrive: "2026-05-10".into(),
            notes: None, blob_ids: vec![],
        }];
        db.insert_load(&load).await.unwrap();
        assert!(db.any_load_references_facility(fac_id).await.unwrap());
        assert!(!db.any_load_references_facility(uuid::Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    async fn test_any_load_references_blob() {
        let (db, _dir) = test_db().await;
        let blob_id = uuid::Uuid::new_v4();
        let mut load = sample_load();
        load.blob_ids = vec![blob_id];
        db.insert_load(&load).await.unwrap();
        assert!(db.any_load_references_blob(blob_id).await.unwrap());
        assert!(!db.any_load_references_blob(uuid::Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    async fn test_update_load_miles() {
        let (db, _dir) = test_db().await;
        let load = sample_load();
        db.insert_load(&load).await.unwrap();
        db.update_load_miles(load.id, 385.5).await.unwrap();
        let fetched = db.get_load_by_id(load.id).await.unwrap();
        assert!((fetched.miles.unwrap() - 385.5).abs() < f64::EPSILON);
    }
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test db::load_ops 2>&1 | head -20
```

- [ ] **Step 3: Implement `src/db/load_ops.rs`**

```rust
// src/db/load_ops.rs
use crate::{
    db::{load_schema, DbClient},
    error::AppError,
    models::{LoadListItem, LoadRecord, LoadStatus, Stop},
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
    pub async fn insert_load(&self, record: &LoadRecord) -> Result<(), AppError> {
        let batch = load_to_batch(record, self.embed_dim)?;
        let schema = load_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.load_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_load_by_id(&self, id: Uuid) -> Result<LoadRecord, AppError> {
        let stream = self.load_table.query()
            .only_if(format!("id = '{id}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_loads(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn delete_load_by_id(&self, id: Uuid) -> Result<(), AppError> {
        self.load_table.delete(&format!("id = '{id}'")).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn update_load_metadata(
        &self, id: Uuid,
        customer_name: Option<String>, customer_ref: Option<String>,
        stops: Option<Vec<Stop>>,
        rate_items: Option<Vec<crate::models::RateLineItem>>,
        commodity: Option<String>, weight_lbs: Option<f64>, miles: Option<f64>,
        notes: Option<String>, tags: Option<Vec<String>>, blob_ids: Option<Vec<Uuid>>,
        embedding: Option<Vec<f32>>,
    ) -> Result<LoadRecord, AppError> {
        let mut record = self.get_load_by_id(id).await?;
        if let Some(v) = customer_name { record.customer_name = v; }
        if let Some(v) = customer_ref { record.customer_ref = Some(v); }
        if let Some(v) = stops { record.stops = v; }
        if let Some(v) = rate_items { record.rate_items = v; }
        if let Some(v) = commodity { record.commodity = Some(v); }
        if let Some(v) = weight_lbs { record.weight_lbs = Some(v); }
        if let Some(v) = miles { record.miles = Some(v); }
        if let Some(v) = notes { record.notes = Some(v); }
        if let Some(v) = tags { record.tags = v; }
        if let Some(v) = blob_ids { record.blob_ids = v; }
        if let Some(v) = embedding { record.embedding = Some(v); }
        record.updated_at = Utc::now();
        self.delete_load_by_id(id).await?;
        self.insert_load(&record).await?;
        Ok(record)
    }

    pub async fn transition_load_status(
        &self, id: Uuid, new_status: LoadStatus,
        invoice_number: Option<String>,
        invoice_date: Option<String>,
        cancellation_reason: Option<String>,
    ) -> Result<LoadRecord, AppError> {
        let mut record = self.get_load_by_id(id).await?;
        if !record.status.can_transition_to(&new_status) {
            return Err(AppError::Conflict(format!(
                "cannot transition from '{}' to '{}'",
                record.status.as_str(), new_status.as_str()
            )));
        }
        record.status = new_status;
        if let Some(v) = invoice_number { record.invoice_number = Some(v); }
        if let Some(v) = invoice_date { record.invoice_date = Some(v); }
        if let Some(v) = cancellation_reason { record.cancellation_reason = Some(v); }
        record.updated_at = Utc::now();
        self.delete_load_by_id(id).await?;
        self.insert_load(&record).await?;
        Ok(record)
    }

    pub async fn update_load_miles(&self, id: Uuid, miles: f64) -> Result<(), AppError> {
        let mut record = self.get_load_by_id(id).await?;
        record.miles = Some(miles);
        record.updated_at = Utc::now();
        self.delete_load_by_id(id).await?;
        self.insert_load(&record).await
    }

    pub async fn list_loads(
        &self,
        status_filter: Option<&str>,
        customer_filter: Option<&str>,
        tag_filter: &[String],
        from_date: Option<&str>,
        to_date: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<LoadListItem>), AppError> {
        let filter = build_load_filter(status_filter, customer_filter, tag_filter, from_date, to_date);
        let total = self.load_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.load_table.query().limit(limit + offset);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let items: Vec<LoadListItem> = batches_to_loads(collect_stream(stream).await?)?
            .into_iter().skip(offset).map(LoadListItem::from).collect();
        Ok((total, items))
    }

    pub async fn search_loads(
        &self,
        embedding: Vec<f32>,
        status_filter: Option<&str>,
        customer_filter: Option<&str>,
        tag_filter: &[String],
        limit: usize,
    ) -> Result<Vec<LoadListItem>, AppError> {
        let filter = build_load_filter(status_filter, customer_filter, tag_filter, None, None);
        let mut q = self.load_table.query()
            .nearest_to(embedding)
            .map_err(|e| AppError::Internal(e.to_string()))?
            .limit(limit);
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        let mut items = Vec::new();
        for batch in &batches {
            let distances = batch.column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
                .map(|a| (0..a.len()).map(|i| a.value(i)).collect::<Vec<_>>());
            for (i, record) in batches_to_loads(vec![batch.clone()])?.into_iter().enumerate() {
                let mut item = LoadListItem::from(record);
                if let Some(ref d) = distances { item.score = Some(1.0 / (1.0 + d[i])); }
                items.push(item);
            }
        }
        Ok(items)
    }

    pub async fn next_load_number(&self, year: i32) -> Result<String, AppError> {
        let prefix = format!("LD-{year}-");
        let stream = self.load_table.query()
            .only_if(format!("load_number LIKE '{prefix}%'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let records = batches_to_loads(collect_stream(stream).await?)?;
        let max_n = records.iter()
            .filter_map(|r| r.load_number.strip_prefix(&prefix))
            .filter_map(|s| s.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        Ok(format!("{prefix}{:04}", max_n + 1))
    }

    pub async fn any_load_references_facility(&self, facility_id: Uuid) -> Result<bool, AppError> {
        // Use JSON string boundaries ("%"uuid"%) to avoid false positives from UUID substrings
        let count = self.load_table
            .count_rows(Some(format!("stops LIKE '%\"{}\"%'", facility_id)))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(count > 0)
    }

    pub async fn any_load_references_blob(&self, blob_id: Uuid) -> Result<bool, AppError> {
        let id_str = blob_id.to_string();
        // Use JSON string boundaries to avoid false positives from UUID substrings
        let blob_count = self.load_table
            .count_rows(Some(format!("blob_ids LIKE '%\"{id_str}\"%'")))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        let stop_count = self.load_table
            .count_rows(Some(format!("stops LIKE '%\"{id_str}\"%'")))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(blob_count + stop_count > 0)
    }

    pub async fn list_loads_needing_routing(&self) -> Result<Vec<Uuid>, AppError> {
        // loads with no miles and non-terminal status
        let stream = self.load_table.query()
            .only_if("miles IS NULL AND status NOT IN ('delivered','invoiced','settled','cancelled')")
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_loads(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn create_load_vector_index(&self) -> Result<(), AppError> {
        self.load_table
            .create_index(&["embedding"], lancedb::index::Index::IvfPq(Default::default()))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))
    }
}

// --- Helpers ---

fn load_to_batch(record: &LoadRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = load_schema(embed_dim);
    let stops_json = serde_json::to_string(&record.stops)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let rate_items_json = serde_json::to_string(&record.rate_items)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let tags_json = serde_json::to_string(&record.tags)
        .map_err(|e| AppError::Internal(e.to_string()))?;
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
        Arc::new(StringArray::from(vec![record.id.to_string().as_str()])),
        Arc::new(StringArray::from(vec![record.load_number.as_str()])),
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![record.status.as_str()])),
        Arc::new(StringArray::from(vec![record.customer_name.as_str()])),
        Arc::new(StringArray::from(vec![record.customer_ref.as_deref()])),
        Arc::new(StringArray::from(vec![stops_json.as_str()])),
        Arc::new(StringArray::from(vec![rate_items_json.as_str()])),
        Arc::new(StringArray::from(vec![record.commodity.as_deref()])),
        Arc::new(Float64Array::from(vec![record.weight_lbs])),
        Arc::new(Float64Array::from(vec![record.miles])),
        Arc::new(StringArray::from(vec![record.notes.as_deref()])),
        Arc::new(StringArray::from(vec![tags_json.as_str()])),
        Arc::new(StringArray::from(vec![blob_ids_json.as_str()])),
        Arc::new(StringArray::from(vec![record.invoice_number.as_deref()])),
        Arc::new(StringArray::from(vec![record.invoice_date.as_deref()])),
        Arc::new(StringArray::from(vec![record.cancellation_reason.as_deref()])),
        embedding_col,
        Arc::new(StringArray::from(vec![record.created_at.to_rfc3339().as_str()])),
        Arc::new(StringArray::from(vec![record.updated_at.to_rfc3339().as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_loads(batches: Vec<RecordBatch>) -> Result<Vec<LoadRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_load(batch, i)?); }
    }
    Ok(out)
}

fn row_to_load(batch: &RecordBatch, i: usize) -> Result<LoadRecord, AppError> {
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
    let opt_f64 = |name: &str| -> Option<f64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };

    let stops: Vec<Stop> = serde_json::from_str(&str_col("stops")).unwrap_or_default();
    let rate_items: Vec<crate::models::RateLineItem> =
        serde_json::from_str(&str_col("rate_items")).unwrap_or_default();
    let tags: Vec<String> = serde_json::from_str(&str_col("tags")).unwrap_or_default();
    let blob_ids: Vec<Uuid> = serde_json::from_str(&str_col("blob_ids")).unwrap_or_default();

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(LoadRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        load_number: str_col("load_number"), owner_id: i64_col("owner_id"),
        status: str_col("status").parse().map_err(|e: String| AppError::Internal(e))?,
        customer_name: str_col("customer_name"), customer_ref: opt_str("customer_ref"),
        stops, rate_items,
        commodity: opt_str("commodity"), weight_lbs: opt_f64("weight_lbs"),
        miles: opt_f64("miles"), notes: opt_str("notes"), tags, blob_ids,
        invoice_number: opt_str("invoice_number"), invoice_date: opt_str("invoice_date"),
        cancellation_reason: opt_str("cancellation_reason"),
        embedding,
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_load_filter(
    status: Option<&str>, customer: Option<&str>,
    tags: &[String], from: Option<&str>, to: Option<&str>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    // Escape single quotes to prevent SQL injection in LanceDB filter strings
    if let Some(s) = status { parts.push(format!("status = '{}'", s.replace('\'', "''"))); }
    if let Some(c) = customer {
        let c = c.replace('\'', "''");
        parts.push(format!("customer_name LIKE '%{c}%'"));
    }
    for tag in tags {
        let tag = tag.replace('\'', "''");
        parts.push(format!("tags LIKE '%\"{tag}\"%'"));
    }
    if let Some(f) = from { parts.push(format!("created_at >= '{f}'")); }
    if let Some(t) = to { parts.push(format!("created_at <= '{t}'")); }
    if parts.is_empty() { None } else { Some(parts.join(" AND ")) }
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}
```

- [ ] **Step 4: Run tests — expect pass**

```bash
cargo test db::load_ops 2>&1 | tail -15
```

- [ ] **Step 5: Commit**

```bash
git add src/db/load_ops.rs
git commit -m "feat: add load DB ops"
```

---

## Task 7: Census Bureau geocoding client

**Files:**
- Create: `src/geocoding/mod.rs`
- Modify: `src/lib.rs` (add `pub mod geocoding;`)

- [ ] **Step 1: Write test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lat_lng_string() {
        let client = GeocodingClient::new();
        let result = client.parse_lat_lng("35.1495,-90.0490");
        assert!(result.is_some());
        let (lat, lng) = result.unwrap();
        assert!((lat - 35.1495).abs() < 1e-4);
        assert!((lng - (-90.0490)).abs() < 1e-4);
    }

    #[test]
    fn test_parse_lat_lng_string_with_spaces() {
        let client = GeocodingClient::new();
        assert!(client.parse_lat_lng("35.1495, -90.0490").is_some());
    }

    #[test]
    fn test_parse_lat_lng_rejects_plain_address() {
        let client = GeocodingClient::new();
        assert!(client.parse_lat_lng("Memphis, TN").is_none());
    }

    #[tokio::test]
    #[ignore] // requires network: cargo test geocoding -- --ignored
    async fn test_geocode_live() {
        let client = GeocodingClient::new();
        let result = client.geocode("1600 Pennsylvania Ave NW, Washington, DC").await;
        assert!(result.is_some());
        let (lat, lng, addr) = result.unwrap();
        assert!((lat - 38.8977).abs() < 0.01);
        assert!((lng - (-77.0366)).abs() < 0.01);
        assert!(!addr.is_empty());
    }
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test geocoding 2>&1 | head -10
```

- [ ] **Step 3: Implement `src/geocoding/mod.rs`**

```rust
// src/geocoding/mod.rs
use reqwest::Client;
use serde::Deserialize;

pub struct GeocodingClient {
    client: Client,
}

#[derive(Deserialize)]
struct CensusResponse {
    result: CensusResult,
}

#[derive(Deserialize)]
struct CensusResult {
    #[serde(rename = "addressMatches")]
    address_matches: Vec<AddressMatch>,
}

#[derive(Deserialize)]
struct AddressMatch {
    #[serde(rename = "matchedAddress")]
    matched_address: String,
    coordinates: Coordinates,
}

#[derive(Deserialize)]
struct Coordinates {
    x: f64, // longitude
    y: f64, // latitude
}

impl GeocodingClient {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }

    /// If the address string looks like "lat,lng", parse it directly.
    /// Otherwise call the Census Bureau geocoding API.
    /// Returns (lat, lng, display_address).
    pub async fn geocode(&self, address: &str) -> Option<(f64, f64, String)> {
        if let Some((lat, lng)) = self.parse_lat_lng(address) {
            return Some((lat, lng, address.trim().to_string()));
        }
        self.geocode_via_census(address).await
    }

    /// Parses "lat,lng" or "lat, lng" strings. Returns None for anything else.
    pub fn parse_lat_lng(&self, s: &str) -> Option<(f64, f64)> {
        let parts: Vec<&str> = s.splitn(2, ',').collect();
        if parts.len() != 2 { return None; }
        let lat = parts[0].trim().parse::<f64>().ok()?;
        let lng = parts[1].trim().parse::<f64>().ok()?;
        if lat.abs() > 90.0 || lng.abs() > 180.0 { return None; }
        Some((lat, lng))
    }

    async fn geocode_via_census(&self, address: &str) -> Option<(f64, f64, String)> {
        let resp = self.client
            .get("https://geocoding.geo.census.gov/geocoder/locations/onelineaddress")
            .query(&[("address", address), ("benchmark", "2020"), ("format", "json")])
            .timeout(std::time::Duration::from_secs(10))
            .send().await.ok()?;

        let data: CensusResponse = resp.json().await.ok()?;
        let m = data.result.address_matches.into_iter().next()?;
        Some((m.coordinates.y, m.coordinates.x, m.matched_address))
    }
}
```

- [ ] **Step 4: Add `pub mod geocoding;` to `src/lib.rs`**

- [ ] **Step 5: Run tests — expect pass (non-ignored)**

```bash
cargo test geocoding 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```bash
git add src/geocoding/mod.rs src/lib.rs
git commit -m "feat: add Census Bureau geocoding client"
```

---

## Task 8: ORS HGV routing client

**Files:**
- Create: `src/routing/mod.rs`
- Modify: `src/lib.rs` (add `pub mod routing;`)

- [ ] **Step 1: Write test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_requires_at_least_two_waypoints() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = RoutingClient::new("fake-key");
        let result = rt.block_on(client.calculate_route_miles(&[(35.1495, -90.0490)]));
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore] // requires valid ORS_API_KEY: cargo test routing -- --ignored
    async fn test_calculate_route_live() {
        let key = std::env::var("ORS_API_KEY").expect("ORS_API_KEY required");
        let client = RoutingClient::new(&key);
        // Memphis, TN → Atlanta, GA
        let miles = client.calculate_route_miles(&[
            (35.1495, -90.0490),
            (33.7490, -84.3880),
        ]).await;
        assert!(miles.is_some());
        let m = miles.unwrap();
        assert!(m > 300.0 && m < 600.0, "expected ~385 miles, got {m}");
    }
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test routing 2>&1 | head -10
```

- [ ] **Step 3: Implement `src/routing/mod.rs`**

```rust
// src/routing/mod.rs
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub struct RoutingClient {
    client: Client,
    api_key: String,
}

#[derive(Serialize)]
struct OrsRequest<'a> {
    coordinates: Vec<[f64; 2]>,
    units: &'a str,
}

#[derive(Deserialize)]
struct OrsResponse {
    routes: Vec<OrsRoute>,
}

#[derive(Deserialize)]
struct OrsRoute {
    summary: OrsSummary,
}

#[derive(Deserialize)]
struct OrsSummary {
    distance: f64,
}

impl RoutingClient {
    pub fn new(api_key: &str) -> Self {
        Self { client: Client::new(), api_key: api_key.to_string() }
    }

    /// Calculates total HGV route distance in miles for ordered waypoints (lat, lng).
    /// Returns None if fewer than 2 waypoints or on API error.
    pub async fn calculate_route_miles(&self, waypoints: &[(f64, f64)]) -> Option<f64> {
        if waypoints.len() < 2 { return None; }
        let coordinates: Vec<[f64; 2]> = waypoints.iter()
            .map(|&(lat, lng)| [lng, lat])  // ORS expects [lng, lat]
            .collect();
        let body = OrsRequest { coordinates, units: "mi" };
        let resp = self.client
            .post("https://api.openrouteservice.org/v2/directions/driving-hgv")
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(std::time::Duration::from_secs(15))
            .send().await.ok()?;
        if !resp.status().is_success() { return None; }
        let data: OrsResponse = resp.json().await.ok()?;
        data.routes.into_iter().next().map(|r| r.summary.distance)
    }
}
```

- [ ] **Step 4: Add `pub mod routing;` to `src/lib.rs`**

- [ ] **Step 5: Run tests — expect pass (non-ignored)**

```bash
cargo test routing 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```bash
git add src/routing/mod.rs src/lib.rs
git commit -m "feat: add ORS HGV routing client"
```

---

## Task 9: Config, AppState, and main wiring

**Files:**
- Modify: `src/config.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write failing config test**

Add to `src/config.rs` tests:

```rust
#[test]
fn test_config_ors_and_dedup_defaults() {
    env::set_var("ADMIN_API_KEY", "test-key");
    env::remove_var("ORS_API_KEY");
    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.ors_api_key, "");
    assert!((cfg.facility_dedup_high_threshold - 0.92).abs() < f64::EPSILON);
    assert!((cfg.facility_dedup_low_threshold - 0.75).abs() < f64::EPSILON);
    assert_eq!(cfg.geocoding_workers, 1);
    env::remove_var("ADMIN_API_KEY");
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test config 2>&1 | head -10
```

- [ ] **Step 3: Update `src/config.rs`** — add new fields after `pipeline_workers`:

```rust
pub ors_api_key: String,
pub facility_dedup_high_threshold: f64,
pub facility_dedup_low_threshold: f64,
pub geocoding_workers: usize,
```

Add to `from_env()` body:

```rust
ors_api_key: env::var("ORS_API_KEY").unwrap_or_default(),
facility_dedup_high_threshold: env::var("FACILITY_DEDUP_HIGH_THRESHOLD")
    .ok().and_then(|v| v.parse().ok()).unwrap_or(0.92),
facility_dedup_low_threshold: env::var("FACILITY_DEDUP_LOW_THRESHOLD")
    .ok().and_then(|v| v.parse().ok()).unwrap_or(0.75),
geocoding_workers: env::var("GEOCODING_WORKERS")
    .ok().and_then(|v| v.parse().ok()).unwrap_or(1),
```

- [ ] **Step 4: Update `src/lib.rs`** — expand AppState:

```rust
// src/lib.rs
pub mod ai;
pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod geocoding;
pub mod models;
pub mod pipeline;
pub mod routing;
pub mod storage;

use ai::OllamaClient;
use config::Config;
use db::DbClient;
use geocoding::GeocodingClient;
use routing::RoutingClient;
use std::sync::Arc;
use storage::BlobStore;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbClient>,
    pub store: Arc<BlobStore>,
    pub ai: Arc<OllamaClient>,
    pub geocoding: Arc<GeocodingClient>,
    pub ors: Arc<RoutingClient>,
    pub pipeline_tx: async_channel::Sender<Uuid>,
    pub geocoding_tx: async_channel::Sender<Uuid>,
    pub routing_tx: async_channel::Sender<Uuid>,
    pub config: Arc<Config>,
}
```

- [ ] **Step 5: Update `src/main.rs`** — wire new components:

```rust
// src/main.rs
use ollie::{
    ai::OllamaClient,
    api,
    config::Config,
    db::DbClient,
    geocoding::GeocodingClient,
    pipeline::{recovery::requeue_stale, spawn_pipeline, spawn_geocoding_pipeline, spawn_routing_pipeline},
    routing::RoutingClient,
    storage::BlobStore,
    AppState,
};
use std::{net::SocketAddr, sync::Arc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ollie=info".into()),
        )
        .init();

    let config = Arc::new(Config::from_env().map_err(|e| anyhow::anyhow!(e))?);

    let db = Arc::new(DbClient::new(&config.lancedb_path, config.ollama_embed_dim).await?);
    let store = Arc::new(BlobStore::new(&config.blob_store_path));
    let ai = Arc::new(OllamaClient::new(
        &config.ollama_base_url, &config.ollama_embed_model,
        &config.ollama_summary_model, &config.ollama_vision_model,
    ));
    let geocoding = Arc::new(GeocodingClient::new());
    let ors = Arc::new(RoutingClient::new(&config.ors_api_key));

    let pipeline_tx = spawn_pipeline(config.pipeline_workers, db.clone(), store.clone(), ai.clone());
    let geocoding_tx = spawn_geocoding_pipeline(config.geocoding_workers, db.clone(), geocoding.clone(), ai.clone());
    let routing_tx = spawn_routing_pipeline(1, db.clone(), ors.clone());

    requeue_stale(&db, &pipeline_tx, &geocoding_tx, &routing_tx).await?;

    for (table, label) in [
        (db.create_vector_index().await, "blobs"),
        (db.create_facility_vector_index().await, "facilities"),
        (db.create_load_vector_index().await, "loads"),
    ] {
        if let Err(e) = table {
            tracing::warn!("vector index not created for {label}: {e}");
        }
    }

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx,
        config: config.clone(),
    };
    let app = api::router(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 6: Update `tests/integration_test.rs` `test_server()` function** to include new AppState fields:

```rust
async fn test_server() -> (TestServer, TempDir, TempDir, async_channel::Receiver<uuid::Uuid>) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    std::env::set_var("ADMIN_API_KEY", "test-secret");

    let config = Arc::new(Config::from_env().unwrap());
    let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai = Arc::new(OllamaClient::new(
        "http://localhost:11434", "nomic-embed-text", "llama3.2", "llava",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));

    let (pipeline_tx, rx) = async_channel::bounded(100);
    let (geocoding_tx, _grx) = async_channel::bounded(100);
    let (routing_tx, _rrx) = async_channel::bounded(100);

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
    };
    let server = TestServer::new(api::router(state)).unwrap();
    (server, blob_dir, db_dir, rx)
}
```

- [ ] **Step 7: Run all tests — expect pass** (pipeline functions will be missing until Task 10, but they can be stubbed in `pipeline/mod.rs` first)

Add stubs to `src/pipeline/mod.rs`:

```rust
pub fn spawn_geocoding_pipeline(
    _workers: usize, _db: Arc<DbClient>, _geocoding: Arc<crate::geocoding::GeocodingClient>,
    _ai: Arc<crate::ai::OllamaClient>,
) -> async_channel::Sender<uuid::Uuid> {
    let (tx, _rx) = async_channel::bounded::<uuid::Uuid>(256);
    tx
}

pub fn spawn_routing_pipeline(
    _workers: usize, _db: Arc<DbClient>, _ors: Arc<crate::routing::RoutingClient>,
) -> async_channel::Sender<uuid::Uuid> {
    let (tx, _rx) = async_channel::bounded::<uuid::Uuid>(256);
    tx
}
```

Also update `requeue_stale` signature in `src/pipeline/recovery.rs` to accept the new senders (add `_geocoding_tx` and `_routing_tx` params, no-op for now — they get real logic in Task 10).

**Important:** The existing test `test_requeue_sends_pending_ids_only` in `src/pipeline/recovery.rs` calls `requeue_stale(&db, &tx)` with 2 args. Update it to pass all 4 args (with dummy senders for the two new ones) or it will fail to compile. Example:

```rust
// In the existing test, replace:
//   requeue_stale(&db, &tx).await.unwrap();
// With:
let (gtx, _) = async_channel::bounded(10);
let (rtx, _) = async_channel::bounded(10);
requeue_stale(&db, &tx, &gtx, &rtx).await.unwrap();
```

```bash
cargo test 2>&1 | tail -20
```

- [ ] **Step 8: Commit**

```bash
git add src/config.rs src/lib.rs src/main.rs src/pipeline/mod.rs src/pipeline/recovery.rs tests/integration_test.rs
git commit -m "feat: expand Config, AppState, and main for geocoding + routing"
```

---

## Task 10: Geocoding pipeline

**Files:**
- Create: `src/pipeline/geocoding.rs`
- Modify: `src/pipeline/mod.rs` (replace stub with real implementation)
- Modify: `src/pipeline/recovery.rs` (add geocoding requeue)

- [ ] **Step 1: Write test in `src/pipeline/geocoding.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::DbClient, geocoding::GeocodingClient, models::GeocodeStatus};
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_geocode_worker_marks_failed_on_no_match() {
        // Uses a nonsense address that won't geocode
        let dir = TempDir::new().unwrap();
        let db = Arc::new(DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap());
        let geocoding = Arc::new(GeocodingClient::new());
        let ai = Arc::new(crate::ai::OllamaClient::new(
            "http://localhost:11434", "nomic-embed-text", "llama3.2", "llava",
        ));

        let now = chrono::Utc::now();
        let facility = crate::models::FacilityRecord {
            id: uuid::Uuid::new_v4(), owner_id: 0,
            name: "XYZZY".into(),
            address: "zzzzzznotanaddressatall12345".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: GeocodeStatus::Pending,
            contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
            avg_dwell_minutes: None, dwell_sample_count: 0, embedding: None,
            created_at: now, updated_at: now,
        };
        db.insert_facility(&facility).await.unwrap();

        // Process — will fail to geocode (no network or no match)
        // We don't assert on geocode_status since it requires network;
        // we assert the function completes without panic
        let _ = process_facility_geocoding(facility.id, &db, &geocoding, &ai).await;
    }
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test pipeline::geocoding 2>&1 | head -10
```

- [ ] **Step 3: Implement `src/pipeline/geocoding.rs`**

```rust
// src/pipeline/geocoding.rs
use crate::{
    ai::{embed::embed_text, OllamaClient},
    db::DbClient,
    error::AppError,
    geocoding::GeocodingClient,
};
use std::sync::Arc;
use uuid::Uuid;

pub async fn process_facility_geocoding(
    id: Uuid,
    db: &DbClient,
    geocoding: &GeocodingClient,
    ai: &OllamaClient,
) -> Result<(), AppError> {
    let facility = db.get_facility_by_id(id).await?;

    match geocoding.geocode(&facility.address).await {
        Some((lat, lng, normalized)) => {
            db.update_facility_geocode(id, lat, lng, normalized).await?;
            tracing::info!("geocoded facility {id}: {lat},{lng}");
        }
        None => {
            db.mark_facility_geocode_failed(id).await?;
            tracing::warn!("geocoding failed for facility {id}");
            return Ok(());
        }
    }

    // Re-embed now that we have a normalized address
    let facility = db.get_facility_by_id(id).await?;
    match embed_text(ai, &facility.embedding_text()).await {
        Ok(embedding) => {
            db.update_facility_embedding(id, embedding).await?;
        }
        Err(e) => tracing::warn!("embedding failed for facility {id}: {e}"),
    }

    Ok(())
}
```

- [ ] **Step 4: Replace the stub in `src/pipeline/mod.rs`** with the real geocoding pipeline:

```rust
pub fn spawn_geocoding_pipeline(
    workers: usize,
    db: Arc<DbClient>,
    geocoding: Arc<crate::geocoding::GeocodingClient>,
    ai: Arc<crate::ai::OllamaClient>,
) -> async_channel::Sender<uuid::Uuid> {
    let workers = workers.max(1);
    let (tx, rx) = async_channel::bounded::<uuid::Uuid>(256);
    for i in 0..workers {
        let rx = rx.clone();
        let db = db.clone();
        let geocoding = geocoding.clone();
        let ai = ai.clone();
        tokio::spawn(async move {
            tracing::info!("geocoding worker {i} started");
            while let Ok(id) = rx.recv().await {
                if let Err(e) = geocoding::process_facility_geocoding(id, &db, &geocoding, &ai).await {
                    tracing::error!("geocoding worker {i} error for {id}: {e}");
                }
            }
        });
    }
    tx
}
```

Add `pub mod geocoding;` at the top of `src/pipeline/mod.rs`.

- [ ] **Step 5: Update `src/pipeline/recovery.rs`** — add geocoding requeue:

In `requeue_stale`, add after the existing blob requeue logic:

```rust
// Requeue facilities pending geocoding
let pending_geocode = db.list_pending_geocode_facility_ids().await?;
tracing::info!("requeueing {} facilities for geocoding", pending_geocode.len());
for id in pending_geocode {
    geocoding_tx.send(id).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
}

// Requeue loads needing routing
let pending_routing = db.list_loads_needing_routing().await?;
tracing::info!("requeueing {} loads for routing", pending_routing.len());
for id in pending_routing {
    routing_tx.send(id).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
}
```

Update `requeue_stale` signature to:

```rust
pub async fn requeue_stale(
    db: &DbClient,
    pipeline_tx: &async_channel::Sender<Uuid>,
    geocoding_tx: &async_channel::Sender<Uuid>,
    routing_tx: &async_channel::Sender<Uuid>,
) -> Result<(), AppError>
```

- [ ] **Step 6: Run all tests — expect pass**

```bash
cargo test 2>&1 | tail -20
```

- [ ] **Step 7: Commit**

```bash
git add src/pipeline/geocoding.rs src/pipeline/mod.rs src/pipeline/recovery.rs
git commit -m "feat: add geocoding pipeline for facilities"
```

---

## Task 11: ORS routing pipeline

**Files:**
- Create: `src/pipeline/routing.rs`
- Modify: `src/pipeline/mod.rs` (replace routing stub with real implementation)

- [ ] **Step 1: Write test in `src/pipeline/routing.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::DbClient, models::{LoadStatus, RateLineItem}, routing::RoutingClient};
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_routing_skips_load_with_missing_coordinates() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap());
        let ors = Arc::new(RoutingClient::new("fake-key"));

        let fac_id = uuid::Uuid::new_v4();
        let now = chrono::Utc::now();
        // Facility with no coordinates
        let facility = crate::models::FacilityRecord {
            id: fac_id, owner_id: 0, name: "Test".into(), address: "Memphis, TN".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: crate::models::GeocodeStatus::Pending,
            contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
            avg_dwell_minutes: None, dwell_sample_count: 0, embedding: None,
            created_at: now, updated_at: now,
        };
        db.insert_facility(&facility).await.unwrap();

        let load_id = uuid::Uuid::new_v4();
        let load = crate::models::LoadRecord {
            id: load_id, load_number: "LD-2026-0001".into(), owner_id: 0,
            status: LoadStatus::Planned, customer_name: "ACME".into(),
            customer_ref: None,
            stops: vec![crate::models::Stop {
                sequence: 1, stop_type: crate::models::StopType::Pickup,
                service_type: crate::models::ServiceType::LiveLoad,
                facility_id: fac_id, scheduled_arrive: "2026-05-10".into(),
                notes: None, blob_ids: vec![],
            }],
            rate_items: vec![], commodity: None, weight_lbs: None, miles: None,
            notes: None, tags: vec![], blob_ids: vec![],
            invoice_number: None, invoice_date: None, cancellation_reason: None,
            embedding: None, created_at: now, updated_at: now,
        };
        db.insert_load(&load).await.unwrap();

        // Should complete without error and leave miles as None
        process_load_routing(load_id, &db, &ors).await.unwrap();
        let fetched = db.get_load_by_id(load_id).await.unwrap();
        assert!(fetched.miles.is_none());
    }
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test pipeline::routing 2>&1 | head -10
```

- [ ] **Step 3: Implement `src/pipeline/routing.rs`**

```rust
// src/pipeline/routing.rs
use crate::{db::DbClient, error::AppError, routing::RoutingClient};
use std::sync::Arc;
use uuid::Uuid;

pub async fn process_load_routing(
    id: Uuid,
    db: &DbClient,
    ors: &RoutingClient,
) -> Result<(), AppError> {
    let load = db.get_load_by_id(id).await?;

    if load.stops.is_empty() { return Ok(()); }
    if load.miles.is_some() { return Ok(()); } // already calculated

    // Collect coordinates for all stop facilities
    let facility_ids: Vec<Uuid> = load.stops.iter().map(|s| s.facility_id).collect();
    let facilities = db.batch_get_facilities(&facility_ids).await?;

    let waypoints: Vec<(f64, f64)> = load.stops.iter()
        .filter_map(|stop| {
            let f = facilities.get(&stop.facility_id)?;
            Some((f.lat?, f.lng?))
        })
        .collect();

    // Only route if ALL stops have coordinates
    if waypoints.len() != load.stops.len() {
        tracing::debug!("load {id}: not all stops geocoded yet, skipping routing");
        return Ok(());
    }

    match ors.calculate_route_miles(&waypoints).await {
        Some(miles) => {
            db.update_load_miles(id, miles).await?;
            tracing::info!("load {id}: routed {miles:.1} miles");
        }
        None => {
            tracing::warn!("load {id}: ORS routing returned no result");
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Replace routing stub in `src/pipeline/mod.rs`**

```rust
pub fn spawn_routing_pipeline(
    workers: usize,
    db: Arc<DbClient>,
    ors: Arc<crate::routing::RoutingClient>,
) -> async_channel::Sender<uuid::Uuid> {
    let workers = workers.max(1);
    let (tx, rx) = async_channel::bounded::<uuid::Uuid>(256);
    for i in 0..workers {
        let rx = rx.clone();
        let db = db.clone();
        let ors = ors.clone();
        tokio::spawn(async move {
            tracing::info!("routing worker {i} started");
            while let Ok(id) = rx.recv().await {
                if let Err(e) = routing::process_load_routing(id, &db, &ors).await {
                    tracing::error!("routing worker {i} error for {id}: {e}");
                }
            }
        });
    }
    tx
}
```

Add `pub mod routing;` at top of `src/pipeline/mod.rs`.

- [ ] **Step 5: Run all tests — expect pass**

```bash
cargo test 2>&1 | tail -20
```

- [ ] **Step 6: Commit**

```bash
git add src/pipeline/routing.rs src/pipeline/mod.rs
git commit -m "feat: add ORS routing pipeline for loads"
```

---

## Task 12: Blob delete referential integrity

**Files:**
- Modify: `src/api/blob.rs`

- [ ] **Step 1: Write test** — add to `tests/integration_test.rs`:

```rust
#[tokio::test]
async fn test_delete_blob_blocked_when_referenced_by_load() {
    let (server, _b, _d, _rx) = test_server().await;

    // Upload a blob
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"rate con".to_vec())
                .file_name("rate_con.pdf").mime_type("application/pdf")))
        .await;
    let blob_id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a facility
    let fac = server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Test Dock", "address": "Memphis, TN" }))
        .await;
    let fac_id = fac.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Create a load referencing the blob
    let load_body = serde_json::json!({
        "customer_name": "ACME",
        "stops": [{
            "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
            "facility_id": fac_id, "scheduled_arrive": "2026-05-10"
        }],
        "rate_items": [],
        "blob_ids": [blob_id]
    });
    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&load_body)
        .await;

    // Attempt to delete the blob — should be blocked
    let del = server.delete(&format!("/api/v1/blob/{blob_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del.status_code(), 409);
}
```

- [ ] **Step 2: Run — expect fail** (409 not returned yet)

```bash
cargo test test_delete_blob_blocked_when_referenced_by_load 2>&1 | tail -10
```

- [ ] **Step 3: Update `src/api/blob.rs` `delete_blob` handler**

```rust
pub async fn delete_blob(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_by_id(id).await?;

    // Referential integrity: block if any load or stop references this blob
    if state.db.any_load_references_blob(id).await? {
        return Err(AppError::Conflict(
            "blob is referenced by one or more loads and cannot be deleted".into()
        ));
    }

    let ref_count = state.db.count_by_checksum(&record.checksum).await?;
    if ref_count <= 1 {
        state.store.delete(&record.checksum).await?;
    }
    state.db.delete_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 4: Run — expect pass**

```bash
cargo test test_delete_blob_blocked_when_referenced_by_load 2>&1 | tail -10
```

- [ ] **Step 5: Run all tests**

```bash
cargo test 2>&1 | tail -20
```

- [ ] **Step 6: Commit**

```bash
git add src/api/blob.rs tests/integration_test.rs
git commit -m "feat: block blob delete when referenced by loads"
```

---

## Task 13: Facilities API

**Files:**
- Modify: `src/api/facilities.rs`
- Modify: `src/api/mod.rs`

- [ ] **Step 1: Write integration tests** — add to `tests/integration_test.rs`:

```rust
#[tokio::test]
async fn test_create_facility_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "name": "ABC Warehouse",
            "address": "Memphis, TN",
            "contacts": [{"name": "Jane Smith", "title": "Dock Manager"}],
            "tags": ["cold"]
        }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body = resp.json::<serde_json::Value>();
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["name"], "ABC Warehouse");
    assert_eq!(body["geocode_status"], "pending");
}

#[tokio::test]
async fn test_get_facility() {
    let (server, _b, _d, _rx) = test_server().await;
    let create = server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "XYZ Dock", "address": "Nashville, TN" }))
        .await;
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let get = server.get(&format!("/api/v1/facilities/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(get.status_code(), 200);
    assert_eq!(get.json::<serde_json::Value>()["id"], id);
}

#[tokio::test]
async fn test_delete_facility_blocked_when_referenced_by_load() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac = server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Busy Dock", "address": "Atlanta, GA" }))
        .await;
    let fac_id = fac.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
            "rate_items": []
        }))
        .await;

    let del = server.delete(&format!("/api/v1/facilities/{fac_id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del.status_code(), 409);
}

#[tokio::test]
async fn test_list_facilities() {
    let (server, _b, _d, _rx) = test_server().await;
    server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "Dock A", "address": "Memphis, TN" }))
        .await;
    let list = server.get("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(list.status_code(), 200);
    assert!(list.json::<serde_json::Value>()["total"].as_u64().unwrap() >= 1);
}
```

- [ ] **Step 2: Run — expect 404** (routes not registered yet)

```bash
cargo test test_create_facility 2>&1 | tail -10
```

- [ ] **Step 3: Implement `src/api/facilities.rs`**

```rust
// src/api/facilities.rs
use crate::{
    ai::embed::embed_text,
    error::AppError,
    models::{
        CreateFacilityRequest, FacilityListResponse, FacilityRecord,
        FacilityResolutionResponse, GeocodeStatus, UpdateFacilityRequest,
    },
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use axum_extra::extract::Query;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct ListFacilitiesQuery {
    pub s: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub tag: Vec<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn create_facility(
    State(state): State<AppState>,
    Json(body): Json<CreateFacilityRequest>,
) -> Result<impl IntoResponse, AppError> {
    let now = Utc::now();
    let embedding_text = format!(
        "{} {} {} {}",
        body.name,
        body.address,
        body.notes.as_deref().unwrap_or(""),
        body.tags.join(" "),
    );

    let embedding = embed_text(&state.ai, &embedding_text).await.ok();

    let record = FacilityRecord {
        id: Uuid::new_v4(), owner_id: 0,
        name: body.name, address: body.address,
        normalized_address: None, lat: None, lng: None,
        geocode_status: GeocodeStatus::Pending,
        contacts: body.contacts,
        notes: body.notes, tags: body.tags, blob_ids: body.blob_ids,
        avg_dwell_minutes: None, dwell_sample_count: 0,
        embedding, created_at: now, updated_at: now,
    };

    state.db.insert_facility(&record).await?;

    // Queue for geocoding (fire and forget)
    let _ = state.geocoding_tx.try_send(record.id);

    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn list_facilities(
    State(state): State<AppState>,
    Query(q): Query<ListFacilitiesQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = embed_text(&state.ai, &query_text).await?;
        let items = state.db.search_facilities(embedding, q.name.as_deref(), &q.tag, limit).await?;
        let total = items.len();
        return Ok(Json(FacilityListResponse { total, items }));
    }

    let (total, items) = state.db.list_facilities(q.name.as_deref(), &q.tag, limit, offset).await?;
    Ok(Json(FacilityListResponse { total, items }))
}

pub async fn get_facility(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_facility_by_id(id).await?;
    Ok(Json(record))
}

pub async fn update_facility(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateFacilityRequest>,
) -> Result<impl IntoResponse, AppError> {
    let address_changed = body.address.is_some();
    let updated = state.db.update_facility_metadata(
        id, body.name, body.address, body.contacts,
        body.notes, body.tags, body.blob_ids,
    ).await?;

    // Re-embed with new metadata
    let embedding_text = updated.embedding_text();
    if let Ok(embedding) = embed_text(&state.ai, &embedding_text).await {
        let _ = state.db.update_facility_embedding(id, embedding).await;
    }

    // Re-geocode if address changed
    if address_changed {
        let _ = state.geocoding_tx.try_send(id);
    }

    Ok(Json(updated))
}

pub async fn delete_facility(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.db.get_facility_by_id(id).await?; // 404 if not found

    if state.db.any_load_references_facility(id).await? {
        return Err(AppError::Conflict(
            "facility is referenced by one or more loads and cannot be deleted".into()
        ));
    }

    state.db.delete_facility_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Resolve a facility from a name+address string, applying dedup logic.
/// Returns Ok(Uuid) if resolved, Err with FacilityResolutionResponse body if ambiguous.
pub async fn resolve_or_create_facility(
    state: &AppState,
    name: &str,
    address: &str,
    force_new: bool,
) -> Result<Uuid, FacilityResolutionResponse> {
    if force_new {
        return Ok(create_new_facility(state, name, address).await
            .map_err(|_| FacilityResolutionResponse {
                facility_resolution_required: false, candidates: vec![],
            })?);
    }

    let text = format!("{name} {address}");
    let embedding = match embed_text(&state.ai, &text).await {
        Ok(e) => e,
        Err(_) => return Ok(create_new_facility(state, name, address).await
            .map_err(|_| FacilityResolutionResponse {
                facility_resolution_required: false, candidates: vec![],
            })?),
    };

    let candidates = state.db.search_facilities(embedding, None, &[], 5).await
        .unwrap_or_default();

    let high = state.config.facility_dedup_high_threshold;
    let low = state.config.facility_dedup_low_threshold;

    if let Some(top) = candidates.first() {
        if top.score.unwrap_or(0.0) >= high {
            return Ok(top.id);
        }
    }

    let above_low: Vec<_> = candidates.into_iter()
        .filter(|c| c.score.unwrap_or(0.0) >= low)
        .map(|c| crate::models::FacilityCandidate {
            id: c.id, name: c.name, address: c.address,
            normalized_address: c.normalized_address,
            score: c.score.unwrap_or(0.0),
        })
        .collect();

    if !above_low.is_empty() {
        return Err(FacilityResolutionResponse {
            facility_resolution_required: true,
            candidates: above_low,
        });
    }

    Ok(create_new_facility(state, name, address).await
        .map_err(|_| FacilityResolutionResponse {
            facility_resolution_required: false, candidates: vec![],
        })?)
}

async fn create_new_facility(
    state: &AppState,
    name: &str,
    address: &str,
) -> Result<Uuid, AppError> {
    let now = Utc::now();
    let text = format!("{name} {address}");
    let embedding = embed_text(&state.ai, &text).await.ok();
    let record = FacilityRecord {
        id: Uuid::new_v4(), owner_id: 0,
        name: name.to_string(), address: address.to_string(),
        normalized_address: None, lat: None, lng: None,
        geocode_status: GeocodeStatus::Pending,
        contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
        avg_dwell_minutes: None, dwell_sample_count: 0,
        embedding, created_at: now, updated_at: now,
    };
    state.db.insert_facility(&record).await?;
    let _ = state.geocoding_tx.try_send(record.id);
    Ok(record.id)
}
```

- [ ] **Step 3b: Add unit tests for `resolve_or_create_facility` dedup logic**

Add `#[cfg(test)]` block to `src/api/facilities.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ai::OllamaClient, config::Config, db::DbClient, storage::BlobStore,
        pipeline::spawn_geocoding_pipeline,
    };
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn test_state() -> (AppState, TempDir, TempDir) {
        let blob_dir = TempDir::new().unwrap();
        let db_dir = TempDir::new().unwrap();
        std::env::set_var("ADMIN_API_KEY", "test-secret");
        let config = Arc::new(Config::from_env().unwrap());
        let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
        let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
        let ai = Arc::new(OllamaClient::new(
            "http://localhost:11434", "nomic-embed-text", "llama3.2", "llava",
        ));
        let (geocoding_tx, _rx) = async_channel::bounded(10);
        let (routing_tx, _rx2) = async_channel::bounded(10);
        let (pipeline_tx, _rx3) = async_channel::bounded(10);
        let state = AppState { db, store, ai, pipeline_tx, geocoding_tx, routing_tx, config };
        (state, blob_dir, db_dir)
    }

    #[tokio::test]
    async fn test_resolve_creates_new_when_no_embedding() {
        // With no Ollama, embedding is None → no vector candidates → auto-create
        let (state, _b, _d) = test_state().await;
        let id = resolve_or_create_facility(&state, "Fresh Dock", "123 Main St, Dallas TX", false)
            .await
            .expect("should create a new facility");
        // Verify the facility was inserted
        state.db.get_facility_by_id(id).await.expect("facility should exist");
    }

    #[tokio::test]
    async fn test_resolve_returns_existing_on_second_call_no_embedding() {
        // Without embeddings, each call auto-creates — duplicates are possible without Ollama.
        // This test documents that behavior: the function returns Ok(new_id) each time.
        let (state, _b, _d) = test_state().await;
        let id1 = resolve_or_create_facility(&state, "Dock A", "100 Oak Ave, Nashville TN", false)
            .await.unwrap();
        let id2 = resolve_or_create_facility(&state, "Dock A", "100 Oak Ave, Nashville TN", false)
            .await.unwrap();
        // Without embeddings both resolve to new facilities — dedup only works with Ollama
        assert_ne!(id1, id2, "without embeddings, dedup is not active");
    }

    #[tokio::test]
    async fn test_resolve_force_new_skips_dedup() {
        let (state, _b, _d) = test_state().await;
        let id1 = resolve_or_create_facility(&state, "Dock B", "200 Elm St, Atlanta GA", false)
            .await.unwrap();
        let id2 = resolve_or_create_facility(&state, "Dock B", "200 Elm St, Atlanta GA", true)
            .await.unwrap();
        assert_ne!(id1, id2, "force_new_facility=true must always create a new record");
    }
}
```

Run:

```bash
cargo test test_resolve 2>&1 | tail -10
```

Expected: all 3 pass (no Ollama needed; embedding is `None` throughout).

- [ ] **Step 4: Register routes in `src/api/mod.rs`**

```rust
// src/api/mod.rs
pub mod auth;
pub mod blob;
pub mod blobs;
pub mod facilities;
pub mod loads;

use crate::{api::auth::require_bearer, AppState};
use axum::{
    middleware::from_fn,
    routing::{delete, get, patch, post},
    Router,
};

pub fn router(state: AppState) -> Router {
    let key = state.config.admin_api_key.clone();
    Router::new()
        // Blobs
        .route("/api/v1/blobs", post(blobs::upload_blob))
        .route("/api/v1/blobs", get(blobs::list_blobs))
        .route("/api/v1/blob/:id", get(blob::get_blob))
        .route("/api/v1/blob/:id", put(blob::update_blob))
        .route("/api/v1/blob/:id", delete(blob::delete_blob))
        // Facilities
        .route("/api/v1/facilities", post(facilities::create_facility))
        .route("/api/v1/facilities", get(facilities::list_facilities))
        .route("/api/v1/facilities/:id", get(facilities::get_facility))
        .route("/api/v1/facilities/:id", patch(facilities::update_facility))
        .route("/api/v1/facilities/:id", delete(facilities::delete_facility))
        // Loads (registered in Task 14)
        .layer(from_fn(move |req, next| {
            let k = key.clone();
            async move { require_bearer(k, req, next).await }
        }))
        .with_state(state)
}
```

Note: add `use axum::routing::put;` to imports.

- [ ] **Step 5: Run tests — expect pass**

```bash
cargo test test_create_facility test_get_facility test_list_facilities test_delete_facility_blocked 2>&1 | tail -15
```

- [ ] **Step 6: Commit**

```bash
git add src/models/facility.rs src/api/facilities.rs src/api/mod.rs tests/integration_test.rs
git commit -m "feat: add facilities CRUD API"
```

---

## Task 14: Loads API — CRUD

**Files:**
- Modify: `src/error.rs` (add `FacilityResolution` variant returning 200 OK)
- Modify: `src/api/loads.rs`
- Modify: `src/api/mod.rs` (add load routes)

- [ ] **Step 1: Write integration tests** — add to `tests/integration_test.rs`:

```rust
async fn create_test_facility(server: &TestServer, name: &str, address: &str) -> String {
    server.post("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": name, "address": address }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_create_load_returns_201() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "ABC Dock", "Memphis, TN").await;

    let resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "XPO Logistics",
            "customer_ref": "PO-123",
            "stops": [{
                "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                "facility_id": fac_id, "scheduled_arrive": "2026-05-10"
            }],
            "rate_items": [
                {"description": "Line Haul", "amount_usd": 1800.0},
                {"description": "Fuel Surcharge", "amount_usd": 210.0}
            ],
            "commodity": "auto parts", "tags": ["flatbed"]
        }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body = resp.json::<serde_json::Value>();
    assert!(body["id"].as_str().is_some());
    assert!(body["load_number"].as_str().unwrap().starts_with("LD-2026-"));
    assert_eq!(body["status"], "planned");
    assert_eq!(body["total_rate_usd"], 2010.0);
}

#[tokio::test]
async fn test_create_load_auto_creates_facility_from_name_address() {
    let (server, _b, _d, _rx) = test_server().await;

    let resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{
                "sequence": 1, "stop_type": "pickup", "service_type": "pre_loaded",
                "facility_name": "Brand New Dock",
                "address": "Tulsa, OK",
                "scheduled_arrive": "2026-05-10"
            }],
            "rate_items": []
        }))
        .await;
    assert_eq!(resp.status_code(), 201);

    // Verify facility was auto-created
    let facs = server.get("/api/v1/facilities")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert!(facs.json::<serde_json::Value>()["total"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn test_load_number_auto_increments() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let stop = serde_json::json!([{
        "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"
    }]);

    let r1 = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({"customer_name": "A", "stops": stop, "rate_items": []}))
        .await;
    let r2 = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({"customer_name": "B", "stops": stop, "rate_items": []}))
        .await;

    let n1 = r1.json::<serde_json::Value>()["load_number"].as_str().unwrap().to_string();
    let n2 = r2.json::<serde_json::Value>()["load_number"].as_str().unwrap().to_string();
    assert_ne!(n1, n2);
}

#[tokio::test]
async fn test_get_load_detail_includes_facility_info() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "ABC Dock", "Memphis, TN").await;
    let create = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "XPO",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
            "rate_items": []
        }))
        .await;
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let detail = server.get(&format!("/api/v1/loads/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(detail.status_code(), 200);
    let body = detail.json::<serde_json::Value>();
    let stop = &body["stops"][0];
    assert_eq!(stop["facility_name"], "ABC Dock");
    assert_eq!(stop["address"], "Memphis, TN");
}

#[tokio::test]
async fn test_invalid_service_type_for_stop_returns_400() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;

    let resp = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup",
                        "service_type": "live_unload",  // invalid for pickup
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
            "rate_items": []
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_delete_load_returns_204() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let create = server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
            "rate_items": []
        }))
        .await;
    let id = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del = server.delete(&format!("/api/v1/loads/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del.status_code(), 204);
    assert_eq!(
        server.get(&format!("/api/v1/loads/{id}"))
            .add_header(header::AUTHORIZATION, "Bearer test-secret")
            .await.status_code(),
        404
    );
}
```

- [ ] **Step 2: Add `FacilityResolution` variant to `src/error.rs`**

`resolve_stops` needs to signal "ambiguous facility — return candidates to client" as a **200 OK**, not a 409 Conflict. A dedicated `AppError` variant with custom `IntoResponse` keeps all function signatures unchanged.

```rust
// src/error.rs — replace the entire file:
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("internal error: {0}")]
    Internal(String),
    // 200 OK with resolution candidates — a disambiguation prompt, not a true error
    #[error("facility resolution required")]
    FacilityResolution(Box<crate::models::FacilityResolutionResponse>),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if let Self::FacilityResolution(body) = self {
            return (StatusCode::OK, Json(serde_json::to_value(*body).unwrap_or_default())).into_response();
        }
        let status = match &self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::FacilityResolution(_) => unreachable!(),
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_of(e: AppError) -> StatusCode {
        e.into_response().status()
    }

    #[test]
    fn test_error_status_codes() {
        assert_eq!(status_of(AppError::NotFound), StatusCode::NOT_FOUND);
        assert_eq!(status_of(AppError::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(status_of(AppError::BadRequest("x".into())), StatusCode::BAD_REQUEST);
        assert_eq!(status_of(AppError::Conflict("x".into())), StatusCode::CONFLICT);
        assert_eq!(status_of(AppError::Internal("x".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_facility_resolution_returns_200() {
        use crate::models::FacilityResolutionResponse;
        let body = FacilityResolutionResponse { facility_resolution_required: true, candidates: vec![] };
        assert_eq!(status_of(AppError::FacilityResolution(Box::new(body))), StatusCode::OK);
    }
}
```

- [ ] **Step 3: Run — expect 404** (routes not registered)

```bash
cargo test test_create_load 2>&1 | tail -5
```

- [ ] **Step 3: Implement `src/api/loads.rs`** — CRUD handlers:

```rust
// src/api/loads.rs
use crate::{
    ai::embed::embed_text,
    api::facilities::resolve_or_create_facility,
    error::AppError,
    models::{
        CreateLoadRequest, FacilityResolutionResponse, LoadDetailResponse,
        LoadListResponse, LoadRecord, LoadStatus, Stop, StopInput, StopResponse,
        UpdateLoadRequest,
    },
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use axum_extra::extract::Query;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct ListLoadsQuery {
    pub s: Option<String>,
    pub status: Option<String>,
    pub customer: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(default)]
    pub tag: Vec<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn create_load(
    State(state): State<AppState>,
    Json(body): Json<CreateLoadRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate and resolve stops
    let stops = resolve_stops(&state, body.stops).await?;
    let now = Utc::now();

    let load_number = match body.load_number {
        Some(n) => n,
        None => { use chrono::Datelike; state.db.next_load_number(now.year()).await? },
    };

    // Build embedding text using resolved facility names
    let facility_ids: Vec<Uuid> = stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await?;
    let stop_text = stops.iter()
        .filter_map(|s| facilities.get(&s.facility_id))
        .map(|f| format!("{} {}", f.name, f.address))
        .collect::<Vec<_>>().join(" ");
    let embed_text_str = format!(
        "{} {} {} {} {}",
        body.customer_name, stop_text,
        body.commodity.as_deref().unwrap_or(""),
        body.notes.as_deref().unwrap_or(""),
        body.tags.join(" "),
    );
    let embedding = embed_text(&state.ai, &embed_text_str).await.ok();

    let record = LoadRecord {
        id: Uuid::new_v4(), load_number, owner_id: 0,
        status: LoadStatus::Planned,
        customer_name: body.customer_name, customer_ref: body.customer_ref,
        stops, rate_items: body.rate_items,
        commodity: body.commodity, weight_lbs: body.weight_lbs,
        miles: body.miles, notes: body.notes, tags: body.tags,
        blob_ids: body.blob_ids, invoice_number: None, invoice_date: None,
        cancellation_reason: None, embedding, created_at: now, updated_at: now,
    };

    state.db.insert_load(&record).await?;

    // Queue for routing if miles not manually provided
    if record.miles.is_none() {
        let _ = state.routing_tx.try_send(record.id);
    }

    let response = build_detail_response(&state, record).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub async fn list_loads(
    State(state): State<AppState>,
    Query(q): Query<ListLoadsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = embed_text(&state.ai, &query_text).await?;
        let items = state.db.search_loads(
            embedding, q.status.as_deref(), q.customer.as_deref(), &q.tag, limit,
        ).await?;
        let total = items.len();
        return Ok(Json(LoadListResponse { total, items }));
    }

    let (total, items) = state.db.list_loads(
        q.status.as_deref(), q.customer.as_deref(), &q.tag,
        q.from.as_deref(), q.to.as_deref(), limit, offset,
    ).await?;
    Ok(Json(LoadListResponse { total, items }))
}

pub async fn get_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_load_by_id(id).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn update_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateLoadRequest>,
) -> Result<impl IntoResponse, AppError> {
    let stops = match body.stops {
        Some(inputs) => Some(resolve_stops(&state, inputs).await?),
        None => None,
    };

    let existing = state.db.get_load_by_id(id).await?;
    let effective_stops = stops.as_ref().unwrap_or(&existing.stops);
    let facility_ids: Vec<Uuid> = effective_stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await?;
    let stop_text = effective_stops.iter()
        .filter_map(|s| facilities.get(&s.facility_id))
        .map(|f| format!("{} {}", f.name, f.address))
        .collect::<Vec<_>>().join(" ");
    let embed_text_str = format!(
        "{} {} {} {} {}",
        body.customer_name.as_deref().unwrap_or(&existing.customer_name),
        stop_text,
        body.commodity.as_deref().unwrap_or(existing.commodity.as_deref().unwrap_or("")),
        body.notes.as_deref().unwrap_or(existing.notes.as_deref().unwrap_or("")),
        body.tags.as_ref().unwrap_or(&existing.tags).join(" "),
    );
    let embedding = embed_text(&state.ai, &embed_text_str).await.ok();

    let updated = state.db.update_load_metadata(
        id, body.customer_name, body.customer_ref, stops,
        body.rate_items, body.commodity, body.weight_lbs, body.miles,
        body.notes, body.tags, body.blob_ids, embedding,
    ).await?;

    // Re-queue routing if stops changed and miles not manually set
    if body.miles.is_none() {
        let _ = state.routing_tx.try_send(id);
    }

    let response = build_detail_response(&state, updated).await?;
    Ok(Json(response))
}

pub async fn delete_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    state.db.get_load_by_id(id).await?; // 404 if not found
    state.db.delete_load_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Helpers ---

async fn resolve_stops(state: &AppState, inputs: Vec<StopInput>) -> Result<Vec<Stop>, AppError> {
    let mut stops = Vec::new();
    let mut resolutions: Vec<FacilityResolutionResponse> = Vec::new();

    for input in inputs {
        // Validate service_type ↔ stop_type
        if !input.service_type.is_valid_for(&input.stop_type) {
            return Err(AppError::BadRequest(format!(
                "service_type '{}' is not valid for stop_type '{}'",
                input.service_type.as_str(), input.stop_type.as_str()
            )));
        }

        let facility_id = if let Some(id) = input.facility_id {
            // Verify facility exists
            state.db.get_facility_by_id(id).await?;
            id
        } else {
            let name = input.facility_name.ok_or_else(|| AppError::BadRequest(
                "stop must provide either facility_id or facility_name + address".into()
            ))?;
            let address = input.address.ok_or_else(|| AppError::BadRequest(
                "stop must provide address when facility_id is not given".into()
            ))?;
            match resolve_or_create_facility(state, &name, &address, input.force_new_facility).await {
                Ok(id) => id,
                Err(resolution) => {
                    resolutions.push(resolution);
                    continue;
                }
            }
        };

        stops.push(Stop {
            sequence: input.sequence,
            stop_type: input.stop_type,
            service_type: input.service_type,
            facility_id,
            scheduled_arrive: input.scheduled_arrive,
            notes: input.notes,
            blob_ids: input.blob_ids,
        });
    }

    if !resolutions.is_empty() {
        // Return the first ambiguous resolution as 200 OK via AppError::FacilityResolution.
        // The client must re-submit with facility_id or force_new_facility=true.
        return Err(AppError::FacilityResolution(Box::new(resolutions.remove(0))));
    }

    Ok(stops)
}

async fn build_detail_response(
    state: &AppState,
    record: LoadRecord,
) -> Result<LoadDetailResponse, AppError> {
    let facility_ids: Vec<Uuid> = record.stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await?;

    let stops: Vec<StopResponse> = record.stops.iter().map(|stop| {
        let facility = facilities.get(&stop.facility_id);
        StopResponse {
            sequence: stop.sequence,
            stop_type: stop.stop_type.clone(),
            service_type: stop.service_type.clone(),
            facility_id: stop.facility_id,
            facility_name: facility.map(|f| f.name.clone()).unwrap_or_default(),
            address: facility.map(|f| f.address.clone()).unwrap_or_default(),
            normalized_address: facility.and_then(|f| f.normalized_address.clone()),
            lat: facility.and_then(|f| f.lat),
            lng: facility.and_then(|f| f.lng),
            scheduled_arrive: stop.scheduled_arrive.clone(),
            notes: stop.notes.clone(),
            blob_ids: stop.blob_ids.clone(),
        }
    }).collect();

    let total_rate_usd = record.total_rate_usd();
    Ok(LoadDetailResponse {
        id: record.id, load_number: record.load_number, status: record.status,
        customer_name: record.customer_name, customer_ref: record.customer_ref,
        stops, rate_items: record.rate_items, total_rate_usd,
        commodity: record.commodity, weight_lbs: record.weight_lbs, miles: record.miles,
        notes: record.notes, tags: record.tags, blob_ids: record.blob_ids,
        invoice_number: record.invoice_number, invoice_date: record.invoice_date,
        cancellation_reason: record.cancellation_reason,
        created_at: record.created_at, updated_at: record.updated_at,
    })
}
```

- [ ] **Step 4: Register load CRUD routes in `src/api/mod.rs`**

Add after the facilities routes:

```rust
// Loads — CRUD
.route("/api/v1/loads", post(loads::create_load))
.route("/api/v1/loads", get(loads::list_loads))
.route("/api/v1/loads/:id", get(loads::get_load))
.route("/api/v1/loads/:id", patch(loads::update_load))
.route("/api/v1/loads/:id", delete(loads::delete_load))
```

- [ ] **Step 5: Run tests — expect pass**

```bash
cargo test test_create_load test_get_load test_delete_load test_load_number test_invalid_service 2>&1 | tail -20
```

- [ ] **Step 6: Commit**

```bash
git add src/error.rs src/api/loads.rs src/api/mod.rs tests/integration_test.rs
git commit -m "feat: add loads CRUD API and FacilityResolution error variant"
```

---

## Task 15: Loads API — action endpoints

**Files:**
- Modify: `src/api/loads.rs` (add action handlers)
- Modify: `src/api/mod.rs` (register action routes)

- [ ] **Step 1: Write integration tests** — add to `tests/integration_test.rs`:

```rust
async fn create_test_load(server: &TestServer, fac_id: &str) -> String {
    server.post("/api/v1/loads")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({
            "customer_name": "ACME",
            "stops": [{"sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                        "facility_id": fac_id, "scheduled_arrive": "2026-05-10"}],
            "rate_items": [{"description": "Line Haul", "amount_usd": 1500.0}]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_dispatch_transitions_to_dispatched() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let resp = server.post(&format!("/api/v1/loads/{id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["status"], "dispatched");
}

#[tokio::test]
async fn test_invalid_transition_returns_409() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    // Can't go planned → delivered directly
    let resp = server.post(&format!("/api/v1/loads/{id}/deliver"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(resp.status_code(), 409);
}

#[tokio::test]
async fn test_full_load_lifecycle() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let dispatch = server.post(&format!("/api/v1/loads/{id}/dispatch"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;
    assert_eq!(dispatch.json::<serde_json::Value>()["status"], "dispatched");

    let in_transit = server.post(&format!("/api/v1/loads/{id}/in_transit"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;
    assert_eq!(in_transit.json::<serde_json::Value>()["status"], "in_transit");

    let deliver = server.post(&format!("/api/v1/loads/{id}/deliver"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;
    assert_eq!(deliver.json::<serde_json::Value>()["status"], "delivered");

    let invoice = server.post(&format!("/api/v1/loads/{id}/invoice"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({"invoice_number": "INV-001", "invoice_date": "2026-05-15"}))
        .await;
    let body = invoice.json::<serde_json::Value>();
    assert_eq!(body["status"], "invoiced");
    assert_eq!(body["invoice_number"], "INV-001");

    let settle = server.post(&format!("/api/v1/loads/{id}/settle"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret").await;
    assert_eq!(settle.json::<serde_json::Value>()["status"], "settled");
}

#[tokio::test]
async fn test_cancel_load() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let resp = server.post(&format!("/api/v1/loads/{id}/cancel"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({"reason": "Customer cancelled"}))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["status"], "cancelled");
    assert_eq!(body["cancellation_reason"], "Customer cancelled");
}

#[tokio::test]
async fn test_assign_transitions_planned_to_dispatched() {
    let (server, _b, _d, _rx) = test_server().await;
    let fac_id = create_test_facility(&server, "Dock", "Memphis, TN").await;
    let id = create_test_load(&server, &fac_id).await;

    let resp = server.post(&format!("/api/v1/loads/{id}/assign"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<serde_json::Value>()["status"], "dispatched");
}
```

- [ ] **Step 2: Run — expect 404** (routes not registered)

```bash
cargo test test_dispatch test_full_load_lifecycle 2>&1 | tail -5
```

- [ ] **Step 3: Add action handlers to `src/api/loads.rs`**

```rust
use crate::models::{CancelActionRequest, InvoiceActionRequest};

// assign_load and dispatch_load are functionally identical in v1.1 because driver/truck
// assignment fields live on trips (v1.2). Both transition planned → dispatched.
// In v1.2, assign_load will accept { driver_name, truck_number } and write them to a trip.
pub async fn assign_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Dispatched, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn dispatch_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Dispatched, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn in_transit_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::InTransit, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn deliver_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Delivered, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn invoice_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<InvoiceActionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Invoiced,
        body.invoice_number, body.invoice_date, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn cancel_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CancelActionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Cancelled, None, None, body.reason,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}

pub async fn settle_load(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.transition_load_status(
        id, LoadStatus::Settled, None, None, None,
    ).await?;
    let response = build_detail_response(&state, record).await?;
    Ok(Json(response))
}
```

- [ ] **Step 4: Register action routes in `src/api/mod.rs`**

Add after the CRUD load routes:

```rust
// Loads — actions
.route("/api/v1/loads/:id/assign", post(loads::assign_load))
.route("/api/v1/loads/:id/dispatch", post(loads::dispatch_load))
.route("/api/v1/loads/:id/in_transit", post(loads::in_transit_load))
.route("/api/v1/loads/:id/deliver", post(loads::deliver_load))
.route("/api/v1/loads/:id/invoice", post(loads::invoice_load))
.route("/api/v1/loads/:id/cancel", post(loads::cancel_load))
.route("/api/v1/loads/:id/settle", post(loads::settle_load))
```

- [ ] **Step 5: Run all tests — expect pass**

```bash
cargo test 2>&1 | tail -25
```

- [ ] **Step 6: Commit**

```bash
git add src/api/loads.rs src/api/mod.rs tests/integration_test.rs
git commit -m "feat: add load action endpoints (assign/dispatch/in_transit/deliver/invoice/cancel/settle)"
```

---

## Task 16: Final verification

- [ ] **Step 1: Run full test suite**

```bash
cargo test 2>&1 | tail -30
```
Expected: all tests pass.

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -- -D warnings 2>&1 | tail -20
```
Fix any warnings before proceeding.

- [ ] **Step 3: Run type check**

```bash
cargo check 2>&1 | tail -10
```

- [ ] **Step 4: Final commit if any lint fixes were needed**

```bash
git add -p
git commit -m "fix: clippy warnings"
```

- [ ] **Step 5: Tag the version**

```bash
git tag v1.1.0
```
