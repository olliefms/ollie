# Expense Tracking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** First-class expense entity with driver receipt uploads, partial-amount fleet approval, derived reimbursements/deductions, AI-suggested fields, maintenance cross-linking, and a settlement lock hook.

**Architecture:** New per-entity trio (`models/expense.rs`, `db/expense_ops.rs`, `api/fleet_portal/expenses.rs`) plus driver-portal endpoints, an MCP tool set sharing the REST `apply_*` helpers, a pipeline extraction step that stages `suggested_*` fields, and pages in both frontends. Spec: `docs/superpowers/specs/2026-07-21-expense-tracking-design.md`.

**Tech Stack:** Rust (Axum 0.8, LanceDB 0.29, Arrow 58, utoipa 4), vanilla-JS SPAs, Vitest/happy-dom for fleet SPA tests.

## Global Constraints

- Every commit: `git commit -s` (DCO enforced) + trailer `Co-Authored-By: Claude with <model-name>`.
- **Never run `cargo fmt`** — hand-formatted repo. Match surrounding style.
- All updates use `merge_insert` (upsert), never delete+insert.
- LanceDB migration CASTs use **SQL keywords only**: `string`, `double`, `bigint`, `boolean` — never `Utf8`/`utf8`/`Float64`/`float64`.
- `.only_if(...)` not `.filter(...)`; traits `use lancedb::query::{ExecutableQuery, QueryBase};`; sort in memory after `collect_stream`, then `.skip(offset).take(limit)`.
- Bind `String`s to locals before `.as_str()` in batch builders.
- Every new endpoint: `#[utoipa::path]` with `security(("BearerAuth" = []))`, register in `ApiDoc` in `src/api/mod.rs`, `{id}` path syntax.
- Fleet SPA: `escHtml()` on every API-derived interpolation; `API_BASE` for all apiFetch calls; no raw hex colors (tokens from `base.css`).
- Driver PWA: DOM construction only in new files (no innerHTML); do **NOT** bump `CACHE_NAME` or `?v=` stamps (cut-release owns those) — just keep `sw.js` `STATIC_ASSETS` list in sync using the current stamp.
- Run task tests via `cargo test --manifest-path Cargo.toml <name>` from the worktree root; full suite + `cargo clippy` at the end. `Config::from_env` tests can flake in parallel — rerun in isolation before assuming breakage.
- Money fields are `f64` USD (matches `maintenance.cost`). Validate `amount >= 0` and `0 <= approved_amount <= amount`.

**Worktree/branch:** already on `claude/expense-tracking-reimbursement-c2ed69` in this worktree — no new branch needed.

---

### Task 1: Expense scopes

**Files:**
- Modify: `src/models/permission.rs`

**Interfaces:**
- Produces: scopes `"expenses:read"`, `"expenses:write"`, `"expenses:approve"` in `ALL_SCOPES`; dispatcher bundle gains `expenses:read` + `expenses:write` (NOT `expenses:approve`).

- [ ] **Step 1: Write the failing tests** — append to the `tests` module in `src/models/permission.rs`:

```rust
    #[test]
    fn test_dispatcher_expense_scopes() {
        let eff = effective_scopes(Role::Dispatcher, &[]);
        assert!(scope_granted(&eff, "expenses:read"));
        assert!(scope_granted(&eff, "expenses:write"));
        assert!(!scope_granted(&eff, "expenses:approve"));
    }

    #[test]
    fn test_expense_scopes_in_vocabulary() {
        for s in ["expenses:read", "expenses:write", "expenses:approve"] {
            assert!(ALL_SCOPES.contains(&s), "missing {s}");
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --manifest-path Cargo.toml test_dispatcher_expense_scopes`
Expected: FAIL (scope not granted).

- [ ] **Step 3: Implement** — in `DISPATCHER_SCOPES`, after the `"maintenance:write",` line add:

```rust
    "expenses:read",
    "expenses:write",
```

In `ALL_SCOPES`, after `"maintenance:delete",` add:

```rust
    "expenses:read",
    "expenses:write",
    "expenses:approve",
```

- [ ] **Step 4: Run tests**

Run: `cargo test --manifest-path Cargo.toml permission`
Expected: all PASS (existing owner/fleet-manager tests still pass — `*` grants the new scopes automatically).

- [ ] **Step 5: Commit** — `git add src/models/permission.rs && git commit -s -m "feat(expenses): add expenses:read/write/approve scopes"` (+ co-author trailer).

---

### Task 2: Expense model

**Files:**
- Create: `src/models/expense.rs`
- Modify: `src/models/mod.rs` (add `pub mod expense;` + `pub use expense::*;` following the existing re-export style)

**Interfaces:**
- Produces (exact, used by all later tasks):
  - `enum ExpenseCategory { Fuel, Tolls, Scales, Lumper, Parking, Repair, Supplies, Permit, Other }` with `as_str()`/`FromStr` (snake_case strings).
  - `enum ExpenseStatus { Submitted, Reviewed, Settled }` with `as_str()`/`FromStr`.
  - `enum PaymentMethod { Company, Personal }` with `as_str()`/`FromStr`.
  - `struct ExpenseRecord` (fields below), methods `reimbursement() -> Option<f64>`, `deduction() -> Option<f64>`, `disposition() -> Option<&'static str>`, `embedding_text() -> String`, `is_locked() -> bool`.
  - `struct ExpenseResponse` (record fields + derived `reimbursement`/`deduction`/`disposition`), `From<ExpenseRecord>`.
  - `struct ExpenseListResponse { returned: usize, total: usize, items: Vec<ExpenseResponse> }`.

- [ ] **Step 1: Write the file with tests.** Full content of `src/models/expense.rs`:

```rust
// src/models/expense.rs
//
// Expense tracking (see docs/superpowers/specs/2026-07-21-expense-tracking-design.md).
// Money effects are DERIVED, never stored: the review decision (approved_amount vs
// amount) combines with payment_method to yield a reimbursement (personal) or a
// deduction (company). suggested_* fields are AI-staged scaffolding cleared at review.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::models::EquipmentType;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpenseCategory {
    Fuel,
    Tolls,
    Scales,
    Lumper,
    Parking,
    Repair,
    Supplies,
    Permit,
    Other,
}

impl ExpenseCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fuel => "fuel",
            Self::Tolls => "tolls",
            Self::Scales => "scales",
            Self::Lumper => "lumper",
            Self::Parking => "parking",
            Self::Repair => "repair",
            Self::Supplies => "supplies",
            Self::Permit => "permit",
            Self::Other => "other",
        }
    }
}

impl std::str::FromStr for ExpenseCategory {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fuel" => Ok(Self::Fuel),
            "tolls" => Ok(Self::Tolls),
            "scales" => Ok(Self::Scales),
            "lumper" => Ok(Self::Lumper),
            "parking" => Ok(Self::Parking),
            "repair" => Ok(Self::Repair),
            "supplies" => Ok(Self::Supplies),
            "permit" => Ok(Self::Permit),
            "other" => Ok(Self::Other),
            other => Err(format!("unknown expense category: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExpenseStatus {
    Submitted,
    Reviewed,
    Settled,
}

impl ExpenseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Reviewed => "reviewed",
            Self::Settled => "settled",
        }
    }
}

impl std::str::FromStr for ExpenseStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "submitted" => Ok(Self::Submitted),
            "reviewed" => Ok(Self::Reviewed),
            "settled" => Ok(Self::Settled),
            other => Err(format!("unknown expense status: {other}")),
        }
    }
}

/// `company` covers ANY company funds: fleet card, check, comcheck, ACH.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaymentMethod {
    Company,
    Personal,
}

impl PaymentMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Company => "company",
            Self::Personal => "personal",
        }
    }
}

impl std::str::FromStr for PaymentMethod {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "company" => Ok(Self::Company),
            "personal" => Ok(Self::Personal),
            other => Err(format!("unknown payment method: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExpenseRecord {
    pub id: Uuid,
    pub status: ExpenseStatus,
    pub category: ExpenseCategory,
    pub driver_id: Option<Uuid>,
    pub trip_id: Option<Uuid>,
    pub equipment_type: Option<EquipmentType>,
    pub equipment_id: Option<Uuid>,
    pub maintenance_id: Option<Uuid>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    /// Ownership marker: `driver:<uuid>` or `fleet_user:<uuid>`.
    pub submitted_by: String,
    /// ISO date `YYYY-MM-DD`; set at review.
    pub expense_date: Option<String>,
    pub vendor: Option<String>,
    /// Receipt total in USD; set at review.
    pub amount: Option<f64>,
    /// 0 <= approved_amount <= amount; set at review.
    pub approved_amount: Option<f64>,
    pub payment_method: Option<PaymentMethod>,
    pub suggested_amount: Option<f64>,
    pub suggested_date: Option<String>,
    pub suggested_vendor: Option<String>,
    pub suggested_card_last4: Option<String>,
    /// Fleet user UUID string of the reviewer.
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub review_note: Option<String>,
    /// Set by the future settlements feature; locks the record permanently.
    pub settlement_id: Option<Uuid>,
    #[serde(skip)]
    #[schema(skip)]
    pub embedding: Option<Vec<f32>>,
    pub owner_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ExpenseRecord {
    /// Amount owed TO the driver on settlement (personal funds, approved portion).
    pub fn reimbursement(&self) -> Option<f64> {
        match (self.status, self.payment_method, self.approved_amount) {
            (ExpenseStatus::Submitted, _, _) => None,
            (_, Some(PaymentMethod::Personal), Some(a)) if a > 0.0 => Some(a),
            _ => None,
        }
    }

    /// Amount deducted FROM the driver on settlement (company funds, denied portion).
    pub fn deduction(&self) -> Option<f64> {
        match (self.status, self.payment_method, self.amount, self.approved_amount) {
            (ExpenseStatus::Submitted, _, _, _) => None,
            (_, Some(PaymentMethod::Company), Some(t), Some(a)) if t - a > 0.0 => Some(t - a),
            _ => None,
        }
    }

    /// "approved" | "partial" | "rejected", None until reviewed.
    pub fn disposition(&self) -> Option<&'static str> {
        if matches!(self.status, ExpenseStatus::Submitted) {
            return None;
        }
        match (self.amount, self.approved_amount) {
            (Some(t), Some(a)) if a <= 0.0 && t > 0.0 => Some("rejected"),
            (Some(t), Some(a)) if a < t => Some("partial"),
            (Some(_), Some(_)) => Some("approved"),
            _ => None,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.settlement_id.is_some()
    }

    pub fn embedding_text(&self) -> String {
        format!(
            "expense {} {} {} {}",
            self.category.as_str(),
            self.vendor.as_deref().unwrap_or(""),
            self.expense_date.as_deref().unwrap_or(""),
            self.review_note.as_deref().unwrap_or("")
        )
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ExpenseResponse {
    pub id: Uuid,
    pub status: ExpenseStatus,
    pub category: ExpenseCategory,
    pub driver_id: Option<Uuid>,
    pub trip_id: Option<Uuid>,
    pub equipment_type: Option<EquipmentType>,
    pub equipment_id: Option<Uuid>,
    pub maintenance_id: Option<Uuid>,
    pub blob_ids: Vec<Uuid>,
    pub submitted_by: String,
    pub expense_date: Option<String>,
    pub vendor: Option<String>,
    pub amount: Option<f64>,
    pub approved_amount: Option<f64>,
    pub payment_method: Option<PaymentMethod>,
    pub suggested_amount: Option<f64>,
    pub suggested_date: Option<String>,
    pub suggested_vendor: Option<String>,
    pub suggested_card_last4: Option<String>,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub review_note: Option<String>,
    pub settlement_id: Option<Uuid>,
    /// Derived: approved portion when payment_method=personal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reimbursement: Option<f64>,
    /// Derived: denied portion when payment_method=company.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deduction: Option<f64>,
    /// Derived: "approved" | "partial" | "rejected".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disposition: Option<&'static str>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ExpenseRecord> for ExpenseResponse {
    fn from(r: ExpenseRecord) -> Self {
        let reimbursement = r.reimbursement();
        let deduction = r.deduction();
        let disposition = r.disposition();
        Self {
            id: r.id,
            status: r.status,
            category: r.category,
            driver_id: r.driver_id,
            trip_id: r.trip_id,
            equipment_type: r.equipment_type,
            equipment_id: r.equipment_id,
            maintenance_id: r.maintenance_id,
            blob_ids: r.blob_ids,
            submitted_by: r.submitted_by,
            expense_date: r.expense_date,
            vendor: r.vendor,
            amount: r.amount,
            approved_amount: r.approved_amount,
            payment_method: r.payment_method,
            suggested_amount: r.suggested_amount,
            suggested_date: r.suggested_date,
            suggested_vendor: r.suggested_vendor,
            suggested_card_last4: r.suggested_card_last4,
            reviewed_by: r.reviewed_by,
            reviewed_at: r.reviewed_at,
            review_note: r.review_note,
            settlement_id: r.settlement_id,
            reimbursement,
            deduction,
            disposition,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExpenseListResponse {
    pub returned: usize,
    pub total: usize,
    pub items: Vec<ExpenseResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ExpenseRecord {
        let now = Utc::now();
        ExpenseRecord {
            id: Uuid::new_v4(),
            status: ExpenseStatus::Submitted,
            category: ExpenseCategory::Fuel,
            driver_id: Some(Uuid::new_v4()),
            trip_id: None,
            equipment_type: None,
            equipment_id: None,
            maintenance_id: None,
            blob_ids: vec![],
            submitted_by: format!("driver:{}", Uuid::new_v4()),
            expense_date: None,
            vendor: None,
            amount: None,
            approved_amount: None,
            payment_method: None,
            suggested_amount: None,
            suggested_date: None,
            suggested_vendor: None,
            suggested_card_last4: None,
            reviewed_by: None,
            reviewed_at: None,
            review_note: None,
            settlement_id: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    fn reviewed(amount: f64, approved: f64, method: PaymentMethod) -> ExpenseRecord {
        let mut r = base();
        r.status = ExpenseStatus::Reviewed;
        r.amount = Some(amount);
        r.approved_amount = Some(approved);
        r.payment_method = Some(method);
        r
    }

    #[test]
    fn test_enum_roundtrips() {
        for s in ["fuel","tolls","scales","lumper","parking","repair","supplies","permit","other"] {
            let c: ExpenseCategory = s.parse().unwrap();
            assert_eq!(c.as_str(), s);
        }
        for s in ["submitted","reviewed","settled"] {
            let c: ExpenseStatus = s.parse().unwrap();
            assert_eq!(c.as_str(), s);
        }
        for s in ["company","personal"] {
            let c: PaymentMethod = s.parse().unwrap();
            assert_eq!(c.as_str(), s);
        }
        assert!("cash".parse::<PaymentMethod>().is_err());
    }

    #[test]
    fn test_no_money_effects_until_reviewed() {
        let r = base();
        assert_eq!(r.reimbursement(), None);
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), None);
    }

    #[test]
    fn test_personal_full_approval_reimburses() {
        let r = reviewed(100.0, 100.0, PaymentMethod::Personal);
        assert_eq!(r.reimbursement(), Some(100.0));
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), Some("approved"));
    }

    #[test]
    fn test_personal_partial_reimburses_approved_portion() {
        let r = reviewed(100.0, 80.0, PaymentMethod::Personal);
        assert_eq!(r.reimbursement(), Some(80.0));
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), Some("partial"));
    }

    #[test]
    fn test_personal_rejection_no_effect() {
        let r = reviewed(100.0, 0.0, PaymentMethod::Personal);
        assert_eq!(r.reimbursement(), None);
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), Some("rejected"));
    }

    #[test]
    fn test_company_full_approval_no_effect() {
        let r = reviewed(100.0, 100.0, PaymentMethod::Company);
        assert_eq!(r.reimbursement(), None);
        assert_eq!(r.deduction(), None);
        assert_eq!(r.disposition(), Some("approved"));
    }

    #[test]
    fn test_company_partial_deducts_denied_portion() {
        let r = reviewed(100.0, 80.0, PaymentMethod::Company);
        assert_eq!(r.reimbursement(), None);
        assert!((r.deduction().unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(r.disposition(), Some("partial"));
    }

    #[test]
    fn test_company_rejection_deducts_everything() {
        let r = reviewed(100.0, 0.0, PaymentMethod::Company);
        assert_eq!(r.deduction(), Some(100.0));
        assert_eq!(r.disposition(), Some("rejected"));
    }

    #[test]
    fn test_settlement_lock() {
        let mut r = reviewed(50.0, 50.0, PaymentMethod::Personal);
        assert!(!r.is_locked());
        r.settlement_id = Some(Uuid::new_v4());
        assert!(r.is_locked());
    }

    #[test]
    fn test_embedding_skipped_and_derived_fields_serialize() {
        let r = reviewed(100.0, 80.0, PaymentMethod::Company);
        let resp: ExpenseResponse = r.into();
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("embedding").is_none());
        assert_eq!(json["disposition"], "partial");
        assert!((json["deduction"].as_f64().unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(json["payment_method"], "company");
    }
}
```

- [ ] **Step 2: Register the module** in `src/models/mod.rs` mirroring how `maintenance` is declared/re-exported there.

- [ ] **Step 3: Run tests**

Run: `cargo test --manifest-path Cargo.toml models::expense`
Expected: all PASS.

- [ ] **Step 4: Commit** — `git add src/models/ && git commit -s -m "feat(expenses): expense model with derived reimbursement/deduction"` (+ co-author trailer).

---

### Task 3: Expense table + DB ops

**Files:**
- Modify: `src/db/mod.rs` (declare `pub mod expense_ops;`, add `pub expense_table: Table` to `DbClient`, open in `DbClient::new` next to `maintenance_table`, add `open_or_create_expense`, `expense_schema`, `empty_expense_batch`)
- Create: `src/db/expense_ops.rs`

**Interfaces:**
- Produces (on `DbClient`):
  - `insert_expense(&ExpenseRecord) -> Result<(), AppError>`
  - `get_expense_by_id(Uuid) -> Result<ExpenseRecord, AppError>`
  - `update_expense(&ExpenseRecord) -> Result<(), AppError>` — whole-record upsert; **caller sets `updated_at`**
  - `update_expense_suggestions(id, Option<f64>, Option<String>, Option<String>, Option<String>) -> Result<(), AppError>` — fetch → set suggested_* only → upsert (pipeline-safe)
  - `delete_expense(Uuid) -> Result<(), AppError>`
  - `list_expenses(&ExpenseFilter, limit, offset) -> Result<(usize, Vec<ExpenseRecord>), AppError>` where `ExpenseFilter { status, category, driver_id, trip_id, equipment_id, submitted_by, from, to }` (all `Option<String>`; from/to are `YYYY-MM-DD` compared against `expense_date` falling back to `created_at` date, applied in memory)
  - `expenses_referencing_blob(Uuid) -> Result<Vec<ExpenseRecord>, AppError>`
  - `update_expense_embedding(id, Vec<f32>) -> Result<(), AppError>`

- [ ] **Step 1: Schema + table plumbing in `src/db/mod.rs`.** Copy the `maintenance` pattern exactly. Schema (this column order is canonical for the batch builders):

```rust
pub fn expense_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("category", DataType::Utf8, false),
        Field::new("driver_id", DataType::Utf8, true),
        Field::new("trip_id", DataType::Utf8, true),
        Field::new("equipment_type", DataType::Utf8, true),
        Field::new("equipment_id", DataType::Utf8, true),
        Field::new("maintenance_id", DataType::Utf8, true),
        Field::new("blob_ids", DataType::Utf8, false),          // JSON Vec<Uuid>
        Field::new("submitted_by", DataType::Utf8, false),
        Field::new("expense_date", DataType::Utf8, true),
        Field::new("vendor", DataType::Utf8, true),
        Field::new("amount", DataType::Float64, true),
        Field::new("approved_amount", DataType::Float64, true),
        Field::new("payment_method", DataType::Utf8, true),
        Field::new("suggested_amount", DataType::Float64, true),
        Field::new("suggested_date", DataType::Utf8, true),
        Field::new("suggested_vendor", DataType::Utf8, true),
        Field::new("suggested_card_last4", DataType::Utf8, true),
        Field::new("reviewed_by", DataType::Utf8, true),
        Field::new("reviewed_at", DataType::Utf8, true),
        Field::new("review_note", DataType::Utf8, true),
        Field::new("settlement_id", DataType::Utf8, true),
        Field::new("embedding", DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embed_dim as i32,
        ), true),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}
```

`open_or_create_expense` mirrors `open_or_create_maintenance` (open, on Err create with an `empty_expense_batch` — write that builder with one `StringArray`/`Float64Array`/`Int64Array`/`FixedSizeListArray` column per schema field, same style as `empty_maintenance_batch`). New table → **no migration needed**. Wire `expense_table` into `DbClient::new` and the struct.

- [ ] **Step 2: Write `src/db/expense_ops.rs`.** Follow `maintenance_ops.rs` structurally: `expense_to_batch` (bind every `to_string()` to a local first; serialize `blob_ids` to JSON; optional enums via `record.payment_method.map(|m| m.as_str())` into `StringArray::from(vec![opt])`; `reviewed_at` as `record.reviewed_at.map(|d| d.to_rfc3339())`), `batches_to_expenses`, `row_to_expense` (reuse the `str_col`/`opt_str`/`opt_f64`/`i64_col` closure helpers; parse optional enums with `opt_str("payment_method").map(|s| s.parse()).transpose().map_err(AppError::Internal)?`; parse optional uuids similarly), `build_expense_filter` (equality clauses for status/category/driver_id/trip_id/equipment_id/submitted_by with `'` → `''` escaping), and a local `collect_stream` copy.

`list_expenses` shape (equality filters in SQL, date-range + sort + paging in memory):

```rust
pub struct ExpenseFilter {
    pub status: Option<String>,
    pub category: Option<String>,
    pub driver_id: Option<String>,
    pub trip_id: Option<String>,
    pub equipment_id: Option<String>,
    pub submitted_by: Option<String>,
    pub from: Option<String>,   // YYYY-MM-DD inclusive
    pub to: Option<String>,     // YYYY-MM-DD inclusive
}

fn effective_date(r: &ExpenseRecord) -> String {
    r.expense_date.clone()
        .unwrap_or_else(|| r.created_at.format("%Y-%m-%d").to_string())
}

pub async fn list_expenses(
    &self, filter: &ExpenseFilter, limit: usize, offset: usize,
) -> Result<(usize, Vec<ExpenseRecord>), AppError> {
    let sql = build_expense_filter(filter);
    let mut q = self.expense_table.query();
    if let Some(f) = sql { q = q.only_if(f); }
    let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
    let mut records = batches_to_expenses(collect_stream(stream).await?)?;
    if let Some(ref from) = filter.from {
        records.retain(|r| effective_date(r).as_str() >= from.as_str());
    }
    if let Some(ref to) = filter.to {
        records.retain(|r| effective_date(r).as_str() <= to.as_str());
    }
    records.sort_by(|a, b| {
        effective_date(b).cmp(&effective_date(a))
            .then(b.created_at.cmp(&a.created_at))
    });
    let total = records.len();
    let items = records.into_iter().skip(offset).take(limit).collect();
    Ok((total, items))
}
```

`update_expense_suggestions` (fetch-modify-only-suggestions, like `mark_processing` discipline):

```rust
pub async fn update_expense_suggestions(
    &self, id: Uuid,
    amount: Option<f64>, date: Option<String>,
    vendor: Option<String>, card_last4: Option<String>,
) -> Result<(), AppError> {
    let mut record = self.get_expense_by_id(id).await?;
    record.suggested_amount = amount;
    record.suggested_date = date;
    record.suggested_vendor = vendor;
    record.suggested_card_last4 = card_last4;
    record.updated_at = Utc::now();
    self.upsert_expense(&record).await
}
```

`update_expense(&record)` is `upsert_expense` made public-by-wrapper (plain whole-record upsert). `expenses_referencing_blob` copies `maintenance_referencing_blob` (`blob_ids LIKE '%"<id>"%'`) but returns full records. `delete_expense` copies `delete_maintenance`. `update_expense_embedding` copies `update_maintenance_embedding`.

- [ ] **Step 3: Write ops unit tests** in `expense_ops.rs` `#[cfg(test)] mod tests` (same `test_db()` helper as maintenance_ops tests). Cover:

```rust
// - insert + get round-trip of EVERY field incl. optional enums, blob_ids JSON,
//   reviewed_at, settlement_id (build one fully-populated Reviewed record).
// - get_expense_by_id unknown id -> AppError::NotFound.
// - update_expense: mutate status->Reviewed + amounts, upsert, re-fetch, assert.
// - update_expense_suggestions touches ONLY suggested_* (assert other fields intact).
// - delete_expense then get -> NotFound.
// - list_expenses: seed 3 records (2 same driver, 1 other; different categories,
//   one reviewed with expense_date set), assert driver filter total=2, category
//   filter, status filter, submitted_by filter, from/to range using the reviewed
//   record's expense_date vs the others' created_at date, sort newest-first,
//   and offset/limit paging.
// - expenses_referencing_blob finds the record containing the blob id and not others.
```

Write these as real `#[tokio::test]` functions modeled character-for-character on `maintenance_ops.rs` tests (a `sample(driver_id)` builder mirroring `base()` from the model tests).

- [ ] **Step 4: Run**

Run: `cargo test --manifest-path Cargo.toml expense_ops`
Expected: all PASS. Also `cargo test --manifest-path Cargo.toml --test integration_test` still green (new table is additive).

- [ ] **Step 5: Commit** — `git add src/db/ && git commit -s -m "feat(expenses): expense table schema and ops"` (+ co-author trailer).

---

### Task 4: Maintenance `expense_id` column + migration

**Files:**
- Modify: `src/models/maintenance.rs` (`MaintenanceRecord` + `MaintenanceListItem` + `From` impl gain `pub expense_id: Option<Uuid>` with `#[serde(default)]`)
- Modify: `src/db/mod.rs` (`maintenance_schema` gains trailing `Field::new("expense_id", DataType::Utf8, true)`; `empty_maintenance_batch` gains matching trailing null column; `open_or_create_maintenance` gains migration)
- Modify: `src/db/maintenance_ops.rs` (`maintenance_to_batch` trailing column, `row_to_maintenance` reads `opt_str("expense_id")` parsed to Uuid, `update_maintenance_metadata` gains trailing `expense_id: Option<Uuid>` param)
- Modify: `tests/migration_test.rs` (extend per the standing rule)
- Modify: every existing construction site of `MaintenanceRecord { .. }` (compiler will list them — add `expense_id: None`)

**Interfaces:**
- Produces: `MaintenanceRecord.expense_id: Option<Uuid>`; `update_maintenance_metadata(..., expense_id: Option<Uuid>)` (set-only, appended as the LAST parameter; all existing callers pass `None`).

- [ ] **Step 1: Migration code.** Rewrite `open_or_create_maintenance` following the `open_or_create_trip` migration pattern:

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
        Ok(table) => {
            let existing = table.schema().await
                .map_err(|e| AppError::Internal(e.to_string()))?;
            let mut transforms: Vec<(String, String)> = Vec::new();
            if existing.field_with_name("expense_id").is_err() {
                // SQL type keyword, NEVER the Arrow name (see AGENTS.md recurring-failure lesson)
                transforms.push(("expense_id".into(), "CAST(NULL AS string)".into()));
            }
            if !transforms.is_empty() {
                table.add_columns(NewColumnTransform::SqlExpressions(transforms), None).await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
            }
            Ok(table)
        }
    }
}
```

(Match the exact `add_columns` invocation style used by `open_or_create_trip` at `src/db/mod.rs:437`.)

- [ ] **Step 2: Thread the field through** model, batch builders, row reader, and `update_maintenance_metadata` (inside: `if let Some(v) = expense_id { record.expense_id = Some(v); }`). Fix all compile errors from existing `MaintenanceRecord` literals by adding `expense_id: None`.

- [ ] **Step 3: Extend `tests/migration_test.rs`.** Follow the file's existing seeding pattern: add a seeded pre-change **maintenance** table (current schema MINUS `expense_id`) with one populated row; open with `DbClient::new`; assert (a) the old row is readable via `get_maintenance_by_id` with `expense_id == None`, and (b) a fresh record carrying `expense_id: Some(uuid)` round-trips through `insert_maintenance` / `get_maintenance_by_id`.

- [ ] **Step 4: Run**

Run: `cargo test --manifest-path Cargo.toml --test migration_test` then `cargo test --manifest-path Cargo.toml maintenance`
Expected: all PASS.

- [ ] **Step 5: Commit** — `git add -A src tests && git commit -s -m "feat(expenses): maintenance.expense_id column with migration"` (+ co-author trailer).

---

### Task 5: Fleet REST — create / list / get + events

**Files:**
- Create: `src/api/fleet_portal/expenses.rs`
- Modify: `src/api/fleet_portal/mod.rs` (declare module, add routes)
- Modify: `src/events/mod.rs` (expense event helpers)
- Modify: `src/api/mod.rs` (`ApiDoc`: register the three handler paths + schemas `ExpenseRecord, ExpenseResponse, ExpenseListResponse, ExpenseCategory, ExpenseStatus, PaymentMethod, CreateExpenseBody`)
- Create: `tests/expenses_test.rs`

**Interfaces:**
- Consumes: Task 2 model, Task 3 ops, Task 1 scopes.
- Produces:
  - `POST /fleet/api/v1/expenses` (scope `expenses:write`) → 201 `ExpenseResponse`
  - `GET /fleet/api/v1/expenses` (scope `expenses:read`; query `status, category, driver_id, trip_id, equipment_id, submitted_by, from, to, limit, offset`) → `ExpenseListResponse`
  - `GET /fleet/api/v1/expenses/{id}` (scope `expenses:read`) → `ExpenseResponse`
  - Shared helper `apply_expense_create(state, body: Value, submitted_by: String) -> Result<ExpenseRecord, AppError>` (MCP + driver portal reuse it)
  - `events::expense_submitted(db, id, actor)`, `events::expense_reviewed(db, id, payload)`, `events::expense_deleted(db, id, actor)`

- [ ] **Step 1: Events helpers** in `src/events/mod.rs`, same shape as the trip helpers there:

```rust
pub async fn expense_submitted(db: &crate::db::DbClient, expense_id: uuid::Uuid, actor: Option<String>) {
    let _ = db.append_event("expense", expense_id, "expense.submitted", None, actor, &now_z(), None).await;
}

pub async fn expense_reviewed(db: &crate::db::DbClient, expense_id: uuid::Uuid, payload: String, actor: Option<String>) {
    let _ = db.append_event("expense", expense_id, "expense.reviewed", Some(payload), actor, &now_z(), None).await;
}

pub async fn expense_deleted(db: &crate::db::DbClient, expense_id: uuid::Uuid, actor: Option<String>) {
    let _ = db.append_event("expense", expense_id, "expense.deleted", None, actor, &now_z(), None).await;
}
```

(Match `append_event`'s actual parameter order from the existing helpers in that file — actor is the 5th arg in some; copy an existing call that passes `Some(payload)` e.g. `stop.arrived` and one that passes an actor if present. `expense.*` types classify as "normal" severity by default — no `classify_severity` change needed.)

- [ ] **Step 2: Write the failing integration tests.** Create `tests/expenses_test.rs`. Copy the self-contained `test_server` / `setup_owner` helper block from `tests/terminals_pay_settlement_test.rs` (keep `_rx` alive!). Then:

```rust
#[tokio::test]
async fn test_create_and_get_expense() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "fuel" }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "submitted");
    assert_eq!(body["category"], "fuel");
    assert!(body["amount"].is_null());
    assert!(body["submitted_by"].as_str().unwrap().starts_with("fleet_user:"));
    let id = body["id"].as_str().unwrap();

    let got = server.get(&format!("/fleet/api/v1/expenses/{id}"))
        .authorization_bearer(&token).await;
    assert_eq!(got.status_code(), 200);
    assert_eq!(got.json::<serde_json::Value>()["id"], *id);
}

#[tokio::test]
async fn test_create_expense_validates_links() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    // unknown driver -> 400
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "category": "repair",
            "driver_id": uuid::Uuid::new_v4(),
        })).await;
    assert_eq!(resp.status_code(), 400);
    // equipment_type without equipment_id -> 400
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "repair", "equipment_type": "truck" }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_list_expenses_filters() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    for cat in ["fuel", "tolls"] {
        let r = server.post("/fleet/api/v1/expenses")
            .authorization_bearer(&token)
            .json(&serde_json::json!({ "category": cat })).await;
        assert_eq!(r.status_code(), 201);
    }
    let all = server.get("/fleet/api/v1/expenses").authorization_bearer(&token).await;
    assert_eq!(all.status_code(), 200);
    assert_eq!(all.json::<serde_json::Value>()["total"], 2);
    let fuel = server.get("/fleet/api/v1/expenses?category=fuel")
        .authorization_bearer(&token).await;
    assert_eq!(fuel.json::<serde_json::Value>()["total"], 1);
    let queue = server.get("/fleet/api/v1/expenses?status=submitted")
        .authorization_bearer(&token).await;
    assert_eq!(queue.json::<serde_json::Value>()["total"], 2);
}

#[tokio::test]
async fn test_expenses_require_auth() {
    let (server, _d1, _d2, _rx) = test_server().await;
    assert_eq!(server.get("/fleet/api/v1/expenses").await.status_code(), 401);
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test --manifest-path Cargo.toml --test expenses_test`
Expected: FAIL — 404s (routes absent).

- [ ] **Step 4: Implement `src/api/fleet_portal/expenses.rs`.** Model on `maintenance_writes.rs`. Key pieces:

```rust
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateExpenseBody {
    pub category: ExpenseCategory,
    #[serde(default)] pub driver_id: Option<Uuid>,
    #[serde(default)] pub trip_id: Option<Uuid>,
    #[serde(default)] pub equipment_type: Option<EquipmentType>,
    #[serde(default)] pub equipment_id: Option<Uuid>,
    #[serde(default)] pub maintenance_id: Option<Uuid>,
    #[serde(default)] pub blob_ids: Vec<Uuid>,
    #[serde(default)] pub expense_date: Option<String>,
    #[serde(default)] pub vendor: Option<String>,
    #[serde(default)] pub amount: Option<f64>,
}
```

`apply_expense_create(state, body: Value, submitted_by: String)`:
1. Parse `CreateExpenseBody` (400 on error). Reject `amount` < 0.
2. `equipment_type.is_some() != equipment_id.is_some()` → 400 "equipment_type and equipment_id must be provided together". If both set, validate existence via the same match used in `maintenance_writes::resolve_equipment_unit` (copy the helper or make that one `pub(crate)` and reuse — prefer reuse).
3. Validate `driver_id` via `state.db.get_driver(...)`-equivalent (find the getter used by `driver_writes.rs`), `trip_id` via `state.db.get_trip(...)`, `maintenance_id` via `get_maintenance_by_id` — each mapped to 400 "unknown …".
4. Build `ExpenseRecord` (status `Submitted`, all review/suggested fields `None`, `owner_id: 0`, now timestamps), compute embedding best-effort (`embed_text(&state.ai, &record.embedding_text()).await.ok()`), `insert_expense`, **if `maintenance_id` is set**: back-link by calling `update_maintenance_metadata(mid, None, None, None, None, None, None, None, None, Some(record.id))`.
5. `events::expense_submitted(&state.db, record.id, Some(submitted_by.clone())).await;` return record.

Handlers:

```rust
pub async fn create_expense_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("expenses:write")?;
    let submitted_by = format!("fleet_user:{}", claims.fleet_user_id);
    let record = apply_expense_create(&state, body, submitted_by).await?;
    Ok((StatusCode::CREATED, Json(ExpenseResponse::from(record))))
}
```

`list_expenses_handler`: scope `expenses:read`; `#[derive(Deserialize, IntoParams)] #[into_params(parameter_in = Query)] pub struct ExpenseListQuery { status, category, driver_id, trip_id, equipment_id, submitted_by, from, to: Option<String>, limit: Option<usize>, offset: Option<usize> }`; build `ExpenseFilter`, default limit 100 (cap 1000), map to `ExpenseListResponse { returned: items.len(), total, items: items.into_iter().map(ExpenseResponse::from).collect() }`. Validate `status`/`category` parse via `FromStr` → 400 on unknown.

`get_expense_handler`: scope `expenses:read`, `get_expense_by_id` → `ExpenseResponse`.

Full `#[utoipa::path]` annotations on all three (tag `"fleet"`, security BearerAuth, 400/401/404 responses documented).

- [ ] **Step 5: Wire routes** in `src/api/fleet_portal/mod.rs` `data_router` (before the blob routes):

```rust
.route(
    "/fleet/api/v1/expenses",
    get(expenses::list_expenses_handler).post(expenses::create_expense_handler),
)
.route(
    "/fleet/api/v1/expenses/{id}",
    get(expenses::get_expense_handler),
)
```

Register paths + schemas in `ApiDoc` in `src/api/mod.rs`.

- [ ] **Step 6: Run**

Run: `cargo test --manifest-path Cargo.toml --test expenses_test`
Expected: all PASS.

- [ ] **Step 7: Commit** — `git add -A src tests && git commit -s -m "feat(expenses): fleet REST create/list/get + expense events"` (+ co-author trailer).

---

### Task 6: Fleet REST — review / patch / delete + ACL + settlement lock

**Files:**
- Modify: `src/api/fleet_portal/expenses.rs`
- Modify: `src/api/fleet_portal/mod.rs` (add `.patch(...)`, `.delete(...)` on the `{id}` route + review route)
- Modify: `src/api/mod.rs` (ApiDoc: new paths + `ReviewExpenseBody`, `PatchExpenseBody`)
- Modify: `tests/expenses_test.rs`

**Interfaces:**
- Consumes: Task 5 handlers/helpers.
- Produces:
  - `POST /fleet/api/v1/expenses/{id}/review` (scope `expenses:approve`) → 200 `ExpenseResponse`
  - `PATCH /fleet/api/v1/expenses/{id}` → 200 `ExpenseResponse`
  - `DELETE /fleet/api/v1/expenses/{id}` → 204
  - Shared ACL helpers used verbatim by MCP + driver portal:
    - `pub(crate) fn is_expense_owner(actor: &str, record: &ExpenseRecord) -> bool` (`record.submitted_by == actor`)
    - `pub(crate) async fn apply_expense_review(state, id, body: Value, reviewer_fleet_user_id: String) -> Result<ExpenseRecord, AppError>`
    - `pub(crate) async fn apply_expense_patch(state, id, body: Value, actor: String, scopes: &[String]) -> Result<ExpenseRecord, AppError>`
    - `pub(crate) async fn apply_expense_delete(state, id, actor: String, scopes: &[String]) -> Result<(), AppError>`

- [ ] **Step 1: Write the failing tests** (append to `tests/expenses_test.rs`). Use `test_server_with_state` (copy the helper from `tests/integration_test.rs` into this file if not already present) so settlement-lock can be simulated through `state.db`:

```rust
#[tokio::test]
async fn test_review_full_partial_and_reject_derivations() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let id = create_expense(&server, &token, "fuel").await; // small local helper returning id String

    // Partial approval on company funds -> deduction of the denied portion.
    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 100.0, "approved_amount": 80.0,
            "payment_method": "company", "review_note": "20 was personal snacks"
        })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "reviewed");
    assert_eq!(body["disposition"], "partial");
    assert!((body["deduction"].as_f64().unwrap() - 20.0).abs() < 1e-9);
    assert!(body.get("reimbursement").is_none() || body["reimbursement"].is_null());
    assert!(body["reviewed_by"].as_str().is_some());

    // Re-review is allowed while unsettled: flip to personal full approval.
    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 100.0, "approved_amount": 100.0, "payment_method": "personal"
        })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["disposition"], "approved");
    assert!((body["reimbursement"].as_f64().unwrap() - 100.0).abs() < 1e-9);
}

#[tokio::test]
async fn test_review_validation() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let id = create_expense(&server, &token, "fuel").await;
    // approved > amount -> 400
    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 50.0, "approved_amount": 60.0, "payment_method": "company"
        })).await;
    assert_eq!(resp.status_code(), 400);
    // negative amount -> 400
    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": -5.0, "approved_amount": 0.0, "payment_method": "company"
        })).await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_review_clears_suggestions() {
    let (server, _d1, _d2, _rx, state) = test_server_with_state().await;
    let token = setup_owner(&server).await;
    let id = create_expense(&server, &token, "fuel").await;
    let uid: uuid::Uuid = id.parse().unwrap();
    state.db.update_expense_suggestions(
        uid, Some(42.0), Some("2026-07-01".into()), Some("Loves".into()), Some("1234".into()),
    ).await.unwrap();

    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 42.0, "approved_amount": 42.0, "payment_method": "company"
        })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert!(body["suggested_amount"].is_null());
    assert!(body["suggested_vendor"].is_null());
}

#[tokio::test]
async fn test_settled_expense_is_locked() {
    let (server, _d1, _d2, _rx, state) = test_server_with_state().await;
    let token = setup_owner(&server).await;
    let id = create_expense(&server, &token, "fuel").await;
    let uid: uuid::Uuid = id.parse().unwrap();
    // Simulate the future settlements feature.
    let mut rec = state.db.get_expense_by_id(uid).await.unwrap();
    rec.status = "settled".parse().unwrap();
    rec.settlement_id = Some(uuid::Uuid::new_v4());
    state.db.update_expense(&rec).await.unwrap();

    for resp in [
        server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
            .authorization_bearer(&token)
            .json(&serde_json::json!({
                "amount": 1.0, "approved_amount": 1.0, "payment_method": "company"
            })).await,
        server.patch(&format!("/fleet/api/v1/expenses/{id}"))
            .authorization_bearer(&token)
            .json(&serde_json::json!({ "vendor": "x" })).await,
        server.delete(&format!("/fleet/api/v1/expenses/{id}"))
            .authorization_bearer(&token).await,
    ] {
        assert_eq!(resp.status_code(), 409, "settled record must reject mutation");
    }
}

#[tokio::test]
async fn test_dispatcher_scope_enforcement() {
    // Owner creates a dispatcher user; dispatcher can create + edit own submitted,
    // cannot review, cannot edit others' records.
    let (server, _d1, _d2, _rx) = test_server().await;
    let owner = setup_owner(&server).await;
    // Create dispatcher via users API (copy request shape from users tests in
    // tests/integration_test.rs) and login to get `disp` token.
    let disp = create_dispatcher_and_login(&server, &owner).await; // local helper
    let own = create_expense(&server, &disp, "tolls").await;
    let other = create_expense(&server, &owner, "fuel").await;

    // Dispatcher edits own submitted record: OK.
    let r = server.patch(&format!("/fleet/api/v1/expenses/{own}"))
        .authorization_bearer(&disp)
        .json(&serde_json::json!({ "vendor": "PA Turnpike" })).await;
    assert_eq!(r.status_code(), 200);
    // Dispatcher edits someone else's record: 403.
    let r = server.patch(&format!("/fleet/api/v1/expenses/{other}"))
        .authorization_bearer(&disp)
        .json(&serde_json::json!({ "vendor": "nope" })).await;
    assert_eq!(r.status_code(), 403);
    // Dispatcher cannot review: 403.
    let r = server.post(&format!("/fleet/api/v1/expenses/{own}/review"))
        .authorization_bearer(&disp)
        .json(&serde_json::json!({
            "amount": 10.0, "approved_amount": 10.0, "payment_method": "company"
        })).await;
    assert_eq!(r.status_code(), 403);
    // Dispatcher deletes own submitted: 204. Owner's record: 403.
    assert_eq!(server.delete(&format!("/fleet/api/v1/expenses/{own}"))
        .authorization_bearer(&disp).await.status_code(), 204);
    assert_eq!(server.delete(&format!("/fleet/api/v1/expenses/{other}"))
        .authorization_bearer(&disp).await.status_code(), 403);
}

#[tokio::test]
async fn test_money_fields_require_approve_scope_on_patch() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let owner = setup_owner(&server).await;
    let disp = create_dispatcher_and_login(&server, &owner).await;
    let own = create_expense(&server, &disp, "fuel").await;
    let r = server.patch(&format!("/fleet/api/v1/expenses/{own}"))
        .authorization_bearer(&disp)
        .json(&serde_json::json!({ "amount": 99.0 })).await;
    assert_eq!(r.status_code(), 403);
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test --manifest-path Cargo.toml --test expenses_test` → new tests FAIL (405/404).

- [ ] **Step 3: Implement.** Bodies:

```rust
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewExpenseBody {
    pub amount: f64,
    pub approved_amount: f64,
    pub payment_method: PaymentMethod,
    #[serde(default)] pub expense_date: Option<String>,
    #[serde(default)] pub vendor: Option<String>,
    #[serde(default)] pub review_note: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchExpenseBody {
    #[serde(default)] pub category: Option<ExpenseCategory>,
    #[serde(default)] pub driver_id: Option<Uuid>,
    #[serde(default)] pub trip_id: Option<Uuid>,
    #[serde(default)] pub equipment_type: Option<EquipmentType>,
    #[serde(default)] pub equipment_id: Option<Uuid>,
    #[serde(default)] pub maintenance_id: Option<Uuid>,
    #[serde(default)] pub blob_ids: Option<Vec<Uuid>>,
    #[serde(default)] pub expense_date: Option<String>,
    #[serde(default)] pub vendor: Option<String>,
    #[serde(default)] pub amount: Option<f64>,
    #[serde(default)] pub approved_amount: Option<f64>,
    #[serde(default)] pub payment_method: Option<PaymentMethod>,
    #[serde(default)] pub review_note: Option<String>,
}
```

ACL core (single function so REST/MCP/driver agree):

```rust
/// Mutation authorization. `actor` is "fleet_user:<uuid>" or "driver:<uuid>".
/// Settled -> Conflict. Reviewed -> needs expenses:approve. Submitted -> approve
/// OR (expenses:write AND owner).
pub(crate) fn authorize_expense_mutation(
    record: &ExpenseRecord, actor: &str, scopes: &[String],
) -> Result<(), AppError> {
    use crate::models::permission::scope_granted;
    if record.is_locked() || matches!(record.status, ExpenseStatus::Settled) {
        return Err(AppError::Conflict("expense is settled and locked".into()));
    }
    if scope_granted(scopes, "expenses:approve") {
        return Ok(());
    }
    if matches!(record.status, ExpenseStatus::Reviewed) {
        return Err(AppError::Forbidden("reviewed expenses require expenses:approve".into()));
    }
    if scope_granted(scopes, "expenses:write") && record.submitted_by == actor {
        return Ok(());
    }
    Err(AppError::Forbidden("not the submitter of this expense".into()))
}
```

(Confirm `AppError::Conflict` exists in `src/error.rs` — it is used by `transition_trip_status`; reuse its exact variant name.)

`apply_expense_review`:
1. Parse body; validate `amount >= 0`, `0 <= approved_amount <= amount` → 400.
2. Fetch record; locked/settled → 409 (reuse `authorize_expense_mutation`'s lock check by calling it with the reviewer's scopes — the handler has already required `expenses:approve`, so pass scopes through).
3. Set `amount`, `approved_amount`, `payment_method`, optional `expense_date`/`vendor`/`review_note` (Some-wins), `status = Reviewed`, `reviewed_by = Some(reviewer_fleet_user_id)`, `reviewed_at = Some(Utc::now())`, clear ALL four `suggested_*`, bump `updated_at`; `update_expense`.
4. **Maintenance cost mirror:** if `record.maintenance_id` is Some, call `update_maintenance_metadata(mid, None, None, None, Some(record.amount.unwrap()), None, None, None, None, None)`.
5. Refresh embedding best-effort; `events::expense_reviewed` with payload `serde_json::json!({"amount": .., "approved_amount": .., "payment_method": ..}).to_string()`.
6. **Re-fetch before returning** (AGENTS.md rule for multi-mutation actions): `get_expense_by_id(id)`.

`apply_expense_patch`:
1. Parse; fetch; `authorize_expense_mutation(&record, &actor, scopes)?`.
2. Money fields (`amount`, `approved_amount`, `payment_method`, `review_note`) present in the patch additionally require `scope_granted(scopes, "expenses:approve")` → 403 otherwise.
3. Apply Some-wins field updates; validate equipment pairing + entity existence like create; validate resulting `approved_amount <= amount` when both end up Some → 400.
4. If `maintenance_id` newly set: back-link (same as create). If record is Reviewed and `amount` changed and `maintenance_id` set: re-mirror cost.
5. Bump `updated_at`, `update_expense`, refresh embedding best-effort, re-fetch, return.

`apply_expense_delete`: fetch; `authorize_expense_mutation`; `delete_expense`; `events::expense_deleted`.

Handlers wrap the helpers with `claims.require_scope(...)` (review → `expenses:approve`; patch/delete rely on `authorize_expense_mutation` but still require at least `expenses:write` up front) and `actor = format!("fleet_user:{}", claims.fleet_user_id)`, `scopes = &claims.effective_scopes`. Routes:

```rust
.route(
    "/fleet/api/v1/expenses/{id}",
    get(expenses::get_expense_handler)
        .patch(expenses::patch_expense_handler)
        .delete(expenses::delete_expense_handler),
)
.route("/fleet/api/v1/expenses/{id}/review", post(expenses::review_expense_handler))
```

utoipa on all new handlers; register in ApiDoc.

- [ ] **Step 4: Run** — `cargo test --manifest-path Cargo.toml --test expenses_test` → all PASS.

- [ ] **Step 5: Commit** — `git add -A src tests && git commit -s -m "feat(expenses): review/patch/delete with ownership ACL and settlement lock"` (+ co-author trailer).

---

### Task 7: Maintenance linkage hardening

**Files:**
- Modify: `src/api/fleet_portal/maintenance_writes.rs`
- Modify: `tests/expenses_test.rs`

**Interfaces:**
- Consumes: Task 4 field, Task 6 helpers.
- Produces: `CreateMaintenanceBody`/`PatchMaintenanceBody` gain `#[serde(default)] pub expense_id: Option<Uuid>`; linked-record cost immutability.

- [ ] **Step 1: Failing tests** (append to `tests/expenses_test.rs`):

```rust
#[tokio::test]
async fn test_maintenance_expense_crosslink_and_cost_mirror() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    // Create a truck (copy the minimal truck-create request shape from
    // tests/integration_test.rs truck tests).
    let truck_id = create_truck(&server, &token).await; // local helper
    // Expense with equipment link, then a maintenance record linked to it.
    let exp_id = {
        let r = server.post("/fleet/api/v1/expenses")
            .authorization_bearer(&token)
            .json(&serde_json::json!({
                "category": "repair", "equipment_type": "truck", "equipment_id": truck_id,
            })).await;
        assert_eq!(r.status_code(), 201);
        r.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
    };
    let m = server.post("/fleet/api/v1/maintenance")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "equipment_type": "truck", "equipment_id": truck_id,
            "service_date": "2026-07-20", "category": "repair",
            "description": "roadside tire replacement",
            "expense_id": exp_id,
        })).await;
    assert_eq!(m.status_code(), 201);
    let mid = m.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Cross-link written back onto the expense.
    let e = server.get(&format!("/fleet/api/v1/expenses/{exp_id}"))
        .authorization_bearer(&token).await;
    assert_eq!(e.json::<serde_json::Value>()["maintenance_id"], *mid);

    // Review mirrors amount onto maintenance.cost.
    let r = server.post(&format!("/fleet/api/v1/expenses/{exp_id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 612.0, "approved_amount": 612.0, "payment_method": "company"
        })).await;
    assert_eq!(r.status_code(), 200);
    let got = server.get(&format!("/fleet/api/v1/maintenance/{mid}"))
        .authorization_bearer(&token).await;
    assert_eq!(got.json::<serde_json::Value>()["cost"], 612.0);

    // Direct cost edits on a linked maintenance record are rejected.
    let bad = server.patch(&format!("/fleet/api/v1/maintenance/{mid}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "cost": 1.0 })).await;
    assert_eq!(bad.status_code(), 400);
}

#[tokio::test]
async fn test_warranty_maintenance_needs_no_expense() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let truck_id = create_truck(&server, &token).await;
    let m = server.post("/fleet/api/v1/maintenance")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "equipment_type": "truck", "equipment_id": truck_id,
            "service_date": "2026-07-20", "category": "repair",
            "description": "warranty turbo replacement", "cost": 0.0,
        })).await;
    assert_eq!(m.status_code(), 201);
    let body: serde_json::Value = m.json();
    assert_eq!(body["cost"], 0.0);
    assert!(body["expense_id"].is_null());
}
```

- [ ] **Step 2: Run to verify failure** — unknown field `expense_id` → 400 → tests FAIL.

- [ ] **Step 3: Implement in `maintenance_writes.rs`:**
  - Add `expense_id` to both bodies.
  - `apply_maintenance_create`: if `expense_id` set — validate via `get_expense_by_id` (400 "unknown expense"), set on record, and if that expense's `amount` is Some, use it as `cost` (ignore any caller-passed cost, 400 if caller passed both). After insert, back-link: fetch expense, set `maintenance_id = Some(record.id)`, bump `updated_at`, `update_expense`.
  - `apply_maintenance_patch`: fetch current record first; if `current.expense_id.is_some() && parsed.cost.is_some()` → 400 "cost is managed by the linked expense". If `parsed.expense_id` set: validate, pass through to `update_maintenance_metadata`'s new final param, back-link expense, and mirror its amount into cost.

- [ ] **Step 4: Run** — `cargo test --manifest-path Cargo.toml --test expenses_test` and `cargo test --manifest-path Cargo.toml --test integration_test` (existing maintenance tests must still pass — `expense_id` is optional everywhere). Expected: PASS.

- [ ] **Step 5: Commit** — `git add -A src tests && git commit -s -m "feat(expenses): maintenance cross-link with cost mirroring"` (+ co-author trailer).

---

### Task 8: MCP tools

**Files:**
- Modify: `src/api/fleet_portal/mcp.rs`

**Interfaces:**
- Consumes: `apply_expense_create/patch/review/delete`, `authorize_expense_mutation`, ops, `ExpenseResponse`.
- Produces MCP tools: `list_expenses`, `get_expense`, `create_expense`, `update_expense`, `review_expense`, `delete_expense`.

- [ ] **Step 1: Scope map.** In the `required_scope` match (near the Maintenance arm at `src/api/fleet_portal/mcp.rs:300`):

```rust
        // Expenses
        "list_expenses" | "get_expense" => "expenses:read",
        "create_expense" | "update_expense" | "delete_expense" => "expenses:write",
        "review_expense" => "expenses:approve",
```

If `mcp.rs` has a unit test asserting the tool-name list / scope map, extend it; also add the six names to the tool-name registry near `src/api/fleet_portal/mcp.rs:711` if that list drives anything.

- [ ] **Step 2: `tools_list()` entries** (next to the maintenance tools; keep JSON style identical):

```rust
            {
                "name": "list_expenses",
                "description": "List expenses. Optional filters: status (submitted/reviewed/settled), category, driver_id, trip_id, equipment_id, submitted_by, from/to (YYYY-MM-DD). Newest first. status=submitted is the review queue.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "status":       { "type": "string", "enum": ["submitted","reviewed","settled"] },
                        "category":     { "type": "string", "enum": ["fuel","tolls","scales","lumper","parking","repair","supplies","permit","other"] },
                        "driver_id":    { "type": "string", "format": "uuid" },
                        "trip_id":      { "type": "string", "format": "uuid" },
                        "equipment_id": { "type": "string", "format": "uuid" },
                        "submitted_by": { "type": "string" },
                        "from":         { "type": "string" },
                        "to":           { "type": "string" },
                        "cursor":       { "type": "string" }
                    }
                }
            },
            {
                "name": "get_expense",
                "description": "Get a single expense by UUID, including AI-suggested fields and derived reimbursement/deduction.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "expense_id": { "type": "string", "format": "uuid" } },
                    "required": ["expense_id"]
                }
            },
            {
                "name": "create_expense",
                "description": "Create an expense record (status=submitted). Attach receipt blobs via blob_ids. Optional links: driver_id, trip_id, equipment_type+equipment_id, maintenance_id (links an existing maintenance record; its cost will mirror this expense). Amounts are set at review, but amount may be pre-filled. Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "category":       { "type": "string", "enum": ["fuel","tolls","scales","lumper","parking","repair","supplies","permit","other"] },
                        "driver_id":      { "type": "string", "format": "uuid" },
                        "trip_id":        { "type": "string", "format": "uuid" },
                        "equipment_type": { "type": "string", "enum": ["truck","trailer"] },
                        "equipment_id":   { "type": "string", "format": "uuid" },
                        "maintenance_id": { "type": "string", "format": "uuid" },
                        "blob_ids":       { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "expense_date":   { "type": "string" },
                        "vendor":         { "type": "string" },
                        "amount":         { "type": "number" }
                    },
                    "required": ["category"]
                }
            },
            {
                "name": "update_expense",
                "description": "Update an un-settled expense. Non-managers may only edit their own submitted records; money fields (amount, approved_amount, payment_method, review_note) require expenses:approve. Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "expense_id":     { "type": "string", "format": "uuid" },
                        "category":       { "type": "string", "enum": ["fuel","tolls","scales","lumper","parking","repair","supplies","permit","other"] },
                        "driver_id":      { "type": "string", "format": "uuid" },
                        "trip_id":        { "type": "string", "format": "uuid" },
                        "equipment_type": { "type": "string", "enum": ["truck","trailer"] },
                        "equipment_id":   { "type": "string", "format": "uuid" },
                        "maintenance_id": { "type": "string", "format": "uuid" },
                        "blob_ids":       { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "expense_date":   { "type": "string" },
                        "vendor":         { "type": "string" },
                        "amount":         { "type": "number" },
                        "approved_amount":{ "type": "number" },
                        "payment_method": { "type": "string", "enum": ["company","personal"] },
                        "review_note":    { "type": "string" }
                    },
                    "required": ["expense_id"]
                }
            },
            {
                "name": "review_expense",
                "description": "Review an expense: set the receipt total (amount), the approved_amount (0 <= approved <= amount; equal = full approval, 0 = rejection, between = partial), and payment_method (company = any company funds; personal = driver's own money). Personal approved portion becomes a reimbursement; company denied portion becomes a deduction. Clears AI suggestions.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "expense_id":      { "type": "string", "format": "uuid" },
                        "amount":          { "type": "number" },
                        "approved_amount": { "type": "number" },
                        "payment_method":  { "type": "string", "enum": ["company","personal"] },
                        "expense_date":    { "type": "string" },
                        "vendor":          { "type": "string" },
                        "review_note":     { "type": "string" }
                    },
                    "required": ["expense_id", "amount", "approved_amount", "payment_method"]
                }
            },
            {
                "name": "delete_expense",
                "description": "Delete an un-settled expense. Non-managers may only delete their own submitted records.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "expense_id": { "type": "string", "format": "uuid" } },
                    "required": ["expense_id"]
                }
            },
```

- [ ] **Step 3: Dispatch + impls.** Add to the dispatch match (`src/api/fleet_portal/mcp.rs:1767` area) — note `update/review/delete/create` need `scopes` and `caller_id` like `create_user` does:

```rust
        "list_expenses" => tool_list_expenses(state, args).await,
        "get_expense" => tool_get_expense(state, args).await,
        "create_expense" => tool_create_expense(state, args, caller_id).await,
        "update_expense" => tool_update_expense(state, args, scopes, caller_id).await,
        "review_expense" => tool_review_expense(state, args, caller_id).await,
        "delete_expense" => tool_delete_expense(state, args, scopes, caller_id).await,
```

Implementations are thin wrappers over the Task 5/6 `apply_*` helpers (which carry all validation + ACL — this is why no separate MCP behavior tests are needed beyond the scope map; the REST integration tests exercise the same code paths). Pattern:

```rust
async fn tool_create_expense(state: &AppState, args: &Value, caller_id: &str) -> Result<Value, String> {
    let mut body = args.clone();
    if let Some(o) = body.as_object_mut() { o.remove("cursor"); }
    let submitted_by = format!("fleet_user:{caller_id}");
    let record = crate::api::fleet_portal::expenses::apply_expense_create(state, body, submitted_by)
        .await.map_err(|e| e.to_string())?;
    serde_json::to_value(crate::models::ExpenseResponse::from(record)).map_err(|e| e.to_string())
}
```

(Adapt error mapping to whatever the sibling `tool_create_maintenance` actually does — copy its `AppError`→`String`/`ToolError` conversion verbatim. `update`/`delete` pass `actor = format!("fleet_user:{caller_id}")` and `scopes` straight into `apply_expense_patch`/`apply_expense_delete`. `review` passes `caller_id.to_string()` as reviewer. `list` uses `cursor_offset(args)?` + `ExpenseFilter` from args + `paged(...)` with `PAGE_SIZE`; `get` parses `expense_id`.) Make the `apply_*` helpers and `ExpenseResponse` visible (`pub(crate)`) as needed.

- [ ] **Step 4: Run** — `cargo test --manifest-path Cargo.toml` (mcp unit tests + full lib). Expected: PASS, no warnings from `cargo clippy --manifest-path Cargo.toml` on the new code.

- [ ] **Step 5: Commit** — `git add src/api/fleet_portal/mcp.rs src/api/fleet_portal/expenses.rs && git commit -s -m "feat(expenses): MCP expense tools"` (+ co-author trailer).

---

### Task 9: Driver portal — expense upload, list, delete

**Files:**
- Modify: `src/api/driver_portal/documents.rs` (accept `doctype=expense` + `expense_category` multipart field; create expense on upload)
- Create: `src/api/driver_portal/expenses.rs`
- Modify: `src/api/driver_portal/mod.rs` (declare module + routes)
- Modify: `src/api/mod.rs` (ApiDoc paths)
- Modify: `tests/driver/` — add `tests/driver/expenses_test.rs` (or extend the existing driver test file if one covers documents; follow the existing driver test harness in `tests/driver/` for driver JWT setup)

**Interfaces:**
- Consumes: Tasks 2/3/5 (`apply` helpers not needed — driver create path builds the record directly; events helpers).
- Produces:
  - `POST /driver/api/v1/trips/{id}/documents` with `doctype=expense` (+ optional `expense_category`, default `other`) → also creates an `ExpenseRecord` (submitted, driver+trip linked, `blob_ids=[blob]`, `submitted_by=driver:<uuid>`). Response JSON becomes `{ "blob": <BlobRecord>, "expense_id": <uuid|null> }`? **No** — keep the existing `BlobRecord` response shape (changing it breaks the PWA); the expense is discoverable via the new list endpoint.
  - `GET /driver/api/v1/expenses` → `ExpenseListResponse` (own records, all statuses, newest first; optional `status` query)
  - `DELETE /driver/api/v1/expenses/{id}` → 204 (own + submitted only; reviewed → 403, settled → 409, not-own → 404)

- [ ] **Step 1: Failing tests.** Model the driver-auth setup on the existing tests in `tests/driver/` (PIN auth is the simplest path — copy the exact driver-create + PIN-set + PIN-login sequence those tests use). Tests:

```rust
// test_driver_expense_upload_creates_expense:
//   driver uploads multipart doctype=expense, expense_category=fuel to their trip
//   -> 201/202; GET /driver/api/v1/expenses -> total 1, category fuel,
//   status submitted, trip_id set, blob_ids length 1, submitted_by "driver:<id>".
// test_driver_expense_upload_rejects_bad_category:
//   expense_category=snacks -> 400.
// test_scale_ticket_doctype_still_accepted:
//   doctype=scale_ticket upload -> 201/202 and does NOT create an expense.
// test_driver_sees_reviewed_outcome_but_cannot_delete:
//   fleet owner reviews the expense (REST, amount 40/approved 40/personal);
//   driver GET list shows status reviewed + approved_amount 40 + reimbursement 40;
//   driver DELETE -> 403.
// test_driver_can_delete_own_submitted:
//   upload -> DELETE own expense -> 204; list -> total 0. (Blob is NOT deleted.)
// test_driver_cannot_see_or_delete_others:
//   second driver's expense invisible in first driver's list; DELETE by
//   non-owner driver -> 404.
```

Write these fully, reusing the harness helpers verbatim from the neighboring driver tests (multipart via `axum_test`'s `.multipart(...)` builder — copy the exact upload call shape from the existing document-upload test).

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement.**
  - `documents.rs`: `ALLOWED_DOCTYPES` becomes `&["bol", "pod", "scale_ticket", "expense", "other"]` (scale_ticket stays accepted for API compat — the PWA picker swap happens in Task 12). Parse optional `expense_category` field in the multipart loop (validate via `ExpenseCategory::from_str` → 400). After the blob insert + load-attach block, when `doctype == "expense"`:

```rust
    let expense_created = if doctype == "expense" {
        let now = Utc::now();
        let expense = crate::models::ExpenseRecord {
            id: Uuid::new_v4(),
            status: crate::models::ExpenseStatus::Submitted,
            category: expense_category.unwrap_or(crate::models::ExpenseCategory::Other),
            driver_id: Some(driver_id),
            trip_id: Some(trip_id),
            equipment_type: None,
            equipment_id: None,
            maintenance_id: None,
            blob_ids: vec![record.id],
            submitted_by: format!("driver:{driver_id}"),
            expense_date: None,
            vendor: None,
            amount: None,
            approved_amount: None,
            payment_method: None,
            suggested_amount: None,
            suggested_date: None,
            suggested_vendor: None,
            suggested_card_last4: None,
            reviewed_by: None,
            reviewed_at: None,
            review_note: None,
            settlement_id: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        };
        state.db.insert_expense(&expense).await?;
        crate::events::expense_submitted(&state.db, expense.id, Some(expense.submitted_by.clone())).await;
        true
    } else { false };
    let _ = expense_created;
```

  - `expenses.rs` (driver): list handler builds `ExpenseFilter { submitted_by: Some(format!("driver:{driver_id}")), status: <query>, ..Default::default() }` (derive `Default` on `ExpenseFilter` in Task 3 if not already), maps to `ExpenseListResponse`. Delete handler: fetch; `submitted_by != format!("driver:{driver_id}")` → `AppError::NotFound`; settled → 409; reviewed → `AppError::Forbidden("reviewed expenses can no longer be deleted".into())`; else `delete_expense` + `events::expense_deleted`. Full utoipa annotations, tag `"driver"`.
  - Routes in `driver_portal/mod.rs` inside the JWT-protected `data` router:

```rust
        .route("/expenses", get(expenses::list_expenses))
        .route("/expenses/{id}", axum_delete(expenses::delete_expense))
```

- [ ] **Step 4: Run** — driver tests + full `cargo test --manifest-path Cargo.toml`. Expected: PASS.

- [ ] **Step 5: Commit** — `git add -A src tests && git commit -s -m "feat(expenses): driver expense upload, list, and delete"` (+ co-author trailer).

---

### Task 10: AI receipt extraction pipeline

**Files:**
- Create: `src/ai/expense_fields.rs`
- Modify: `src/ai/mod.rs` (declare module)
- Modify: `src/pipeline/worker.rs` (post-summary hook for `doctype:expense` blobs)

**Interfaces:**
- Consumes: `extract_content`/`Extractable` (`src/ai/extract.rs`), `OllamaClient.generate()` + the vision path used by `describe_image` (`src/ai/summarize.rs`), Task 3 `expenses_referencing_blob` + `update_expense_suggestions`.
- Produces:
  - `pub struct SuggestedExpenseFields { pub amount: Option<f64>, pub date: Option<String>, pub vendor: Option<String>, pub card_last4: Option<String> }`
  - `pub async fn extract_expense_fields(ai: &OllamaClient, content: &Extractable) -> Option<SuggestedExpenseFields>` — best-effort, never errors.
  - `pub fn parse_expense_json(raw: &str) -> Option<SuggestedExpenseFields>` — pure, unit-testable.

- [ ] **Step 1: Write failing unit tests** for the parser in `src/ai/expense_fields.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parses_clean_json() {
        let s = parse_expense_json(r#"{"amount": 84.12, "date": "2026-07-18", "vendor": "Pilot #442", "card_last4": "9910"}"#).unwrap();
        assert_eq!(s.amount, Some(84.12));
        assert_eq!(s.date.as_deref(), Some("2026-07-18"));
        assert_eq!(s.vendor.as_deref(), Some("Pilot #442"));
        assert_eq!(s.card_last4.as_deref(), Some("9910"));
    }

    #[test]
    fn test_parses_json_wrapped_in_prose_and_fences() {
        let raw = "Sure! Here is the extraction:\n```json\n{\"amount\": 12.5, \"date\": null, \"vendor\": \"CAT Scale\", \"card_last4\": null}\n```";
        let s = parse_expense_json(raw).unwrap();
        assert_eq!(s.amount, Some(12.5));
        assert_eq!(s.date, None);
        assert_eq!(s.vendor.as_deref(), Some("CAT Scale"));
    }

    #[test]
    fn test_amount_as_string_is_coerced() {
        let s = parse_expense_json(r#"{"amount": "84.12", "date": null, "vendor": null, "card_last4": null}"#).unwrap();
        assert_eq!(s.amount, Some(84.12));
    }

    #[test]
    fn test_garbage_returns_none() {
        assert!(parse_expense_json("I could not read this receipt.").is_none());
        assert!(parse_expense_json("{not json").is_none());
    }

    #[test]
    fn test_all_null_fields_returns_none() {
        // Nothing extracted -> treat as no suggestion at all.
        assert!(parse_expense_json(r#"{"amount": null, "date": null, "vendor": null, "card_last4": null}"#).is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure**, then **implement**:

```rust
// src/ai/expense_fields.rs
//
// Best-effort structured extraction of receipt fields for expense review
// suggestions. Never authoritative, never fatal: any failure yields None and
// the fleet manager types the values by hand.
use serde::Deserialize;

use crate::ai::{extract::Extractable, OllamaClient};

#[derive(Debug, Clone, PartialEq)]
pub struct SuggestedExpenseFields {
    pub amount: Option<f64>,
    pub date: Option<String>,
    pub vendor: Option<String>,
    pub card_last4: Option<String>,
}

#[derive(Deserialize)]
struct RawFields {
    amount: Option<serde_json::Value>,
    date: Option<String>,
    vendor: Option<String>,
    card_last4: Option<String>,
}

const PROMPT: &str = "You are reading a purchase receipt or invoice. Extract exactly these fields and reply with ONLY a JSON object, no other text: {\"amount\": <total charged as a number, or null>, \"date\": <purchase date as YYYY-MM-DD, or null>, \"vendor\": <merchant name, or null>, \"card_last4\": <last 4 digits of the card used, or null>}";

/// Find the outermost {...} in the model reply and parse it leniently.
pub fn parse_expense_json(raw: &str) -> Option<SuggestedExpenseFields> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start { return None; }
    let parsed: RawFields = serde_json::from_str(&raw[start..=end]).ok()?;
    let amount = parsed.amount.and_then(|v| match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().trim_start_matches('$').parse().ok(),
        _ => None,
    }).filter(|a| *a >= 0.0);
    let non_empty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    let out = SuggestedExpenseFields {
        amount,
        date: non_empty(parsed.date),
        vendor: non_empty(parsed.vendor),
        card_last4: non_empty(parsed.card_last4),
    };
    if out.amount.is_none() && out.date.is_none() && out.vendor.is_none() && out.card_last4.is_none() {
        return None;
    }
    Some(out)
}

pub async fn extract_expense_fields(
    ai: &OllamaClient,
    content: &Extractable,
) -> Option<SuggestedExpenseFields> {
    let raw = match content {
        Extractable::Text(text) => {
            let capped: String = text.chars().take(6000).collect();
            ai.generate(&format!("{PROMPT}\n\nReceipt text:\n{capped}")).await.ok()?
        }
        Extractable::ImageBytes(bytes) => {
            // Same vision entry point describe_image uses — check its signature in
            // src/ai/summarize.rs and call the underlying OllamaClient method with
            // PROMPT as the instruction.
            ai.generate_vision(PROMPT, bytes).await.ok()?
        }
        Extractable::Unsupported => return None,
    };
    parse_expense_json(&raw)
}
```

**Note:** `generate`/`generate_vision` are placeholders for the ACTUAL method names on `OllamaClient` — open `src/ai/mod.rs` and `src/ai/summarize.rs` first and use the real signatures (`summarize_text`/`describe_image` wrap them; call the same underlying client methods with the custom PROMPT). Do not add new HTTP plumbing.

- [ ] **Step 3: Hook into `src/pipeline/worker.rs`.** In `process_blob`, after the summary/embedding success path (where `mark_ready` is called), add — for blobs whose `tags` contain `"doctype:expense"`:

```rust
    // Expense receipts: stage structured suggestions on any submitted expense
    // that references this blob. Best-effort — extraction failure is not a
    // processing failure.
    if record.tags.iter().any(|t| t == "doctype:expense") {
        if let Some(fields) = crate::ai::expense_fields::extract_expense_fields(ai, &content).await {
            if let Ok(expenses) = db.expenses_referencing_blob(record.id).await {
                for e in expenses {
                    if matches!(e.status, crate::models::ExpenseStatus::Submitted) {
                        let _ = db.update_expense_suggestions(
                            e.id,
                            fields.amount,
                            fields.date.clone(),
                            fields.vendor.clone(),
                            fields.card_last4.clone(),
                        ).await;
                    }
                }
            }
        }
    }
```

Fit this to `process_blob`'s actual variable names (`record`, `content`, `ai`, `db` — read the function first; the extracted `Extractable` may need to be kept in scope past the summary instead of consumed — restructure minimally, e.g. match on `&content`).

- [ ] **Step 4: Run** — `cargo test --manifest-path Cargo.toml expense_fields` and the full suite (integration tests never poll workers, so no Ollama needed). Expected: PASS.

- [ ] **Step 5: Commit** — `git add -A src && git commit -s -m "feat(expenses): AI receipt field extraction pipeline"` (+ co-author trailer).

---

### Task 11: Fleet SPA — expenses section

**Files:**
- Create: `static/fleet/pages/expenses.js`, `static/fleet/pages/expense-detail.js`, `static/fleet/pages/expense-form.js`, `static/fleet/utils/expense-meta.js`
- Modify: `static/fleet/router.js`, `static/fleet/components/nav.js`, `static/fleet/components/icons.js` (add `expensesIcon`), `static/fleet/app.js` (view wiring — mirror the maintenance view registrations), `static/fleet/pages/maintenance-form.js` (accept `expense_id` prefill param), `static/fleet/pages/maintenance-detail.js` (show linked-expense link)
- Create: `tests/fleet/expense-meta.test.js`, `tests/fleet/expenses.test.js`

**Interfaces:**
- Consumes: REST endpoints from Tasks 5–6; SPA helpers `apiFetch`/`API_BASE`, `escHtml`, `renderEntityList`, `money` (from `utils/maintenance-meta.js`), `scope-gate`.
- Produces: routes `/fleet/expenses`, `/fleet/expenses/new`, `/fleet/expenses/:id`, `/fleet/expenses/:id/edit`; nav entry gated on `expenses:read`.

**Before writing any code, read:** `static/fleet/pages/maintenance.js`, `maintenance-detail.js`, `maintenance-form.js`, `static/fleet/app.js` (view dispatch), `static/fleet/components/scope-gate.js`, and one existing test in `tests/fleet/` — copy their exact conventions.

- [ ] **Step 1: `utils/expense-meta.js`** (pure module → unit-testable):

```js
export const EXPENSE_CATEGORY_OPTIONS = [
  { value: 'fuel', label: 'Fuel' },
  { value: 'tolls', label: 'Tolls' },
  { value: 'scales', label: 'Scales' },
  { value: 'lumper', label: 'Lumper' },
  { value: 'parking', label: 'Parking' },
  { value: 'repair', label: 'Repair' },
  { value: 'supplies', label: 'Supplies' },
  { value: 'permit', label: 'Permit' },
  { value: 'other', label: 'Other' },
];

export const PAYMENT_METHOD_OPTIONS = [
  { value: 'company', label: 'Company funds' },
  { value: 'personal', label: 'Personal / cash' },
];

export function expenseCategoryLabel(v) {
  const hit = EXPENSE_CATEGORY_OPTIONS.find(o => o.value === v);
  return hit ? hit.label : (v || '—');
}

export function statusBadge(status) {
  return { submitted: 'Needs review', reviewed: 'Reviewed', settled: 'Settled' }[status] || status || '—';
}

// Mirrors the backend derivation for display-only fallbacks.
export function dispositionLabel(e) {
  if (e.disposition === 'approved') return 'Approved';
  if (e.disposition === 'partial') return 'Partially approved';
  if (e.disposition === 'rejected') return 'Rejected';
  return '—';
}
```

- [ ] **Step 2: Vitest tests.** `tests/fleet/expense-meta.test.js` covering `expenseCategoryLabel` known/unknown, `statusBadge`, `dispositionLabel` all four branches. `tests/fleet/expenses.test.js`: follow the structure of the existing page test that mocks `apiFetch` (see `tests/fleet/list.test.js` and any page-level test) — assert the list page renders rows from a mocked `/expenses` response, shows the Needs-review filter tab, and that a `submitted` row shows the status badge text. Run `npm test` — new tests fail first, then implement, then pass.

- [ ] **Step 3: List page `pages/expenses.js`.** Clone the `maintenance.js` shape with `renderEntityList`: columns Date (`expense_date` fallback `created_at` date), Category (`expenseCategoryLabel`), Driver, Amount (`money(e.amount)`), Approved (`money(e.approved_amount)`), Method, Status (`statusBadge`), Submitted by. `extraControls`: status select (All / Needs review / Reviewed / Settled → `?status=`), category select, driver select (populate from `${API_BASE}/drivers` like maintenance populates units), and two `<input type="date">` for from/to. Create button gated `expenses:write` (`createScope`), title "Expenses", `createLabel: '+ New Expense'`, `detailView: 'expense-detail'`. All interpolations through `escHtml` where `renderEntityList` cells emit strings (check how existing `cell:` fns are consumed — if they return text nodes, no escaping needed; match maintenance exactly).

- [ ] **Step 4: Detail page `pages/expense-detail.js`.** Sections:
  - Facts list (status, category, driver, trip, equipment, vendor, dates, submitted_by, amounts, derived reimbursement/deduction/disposition, review note, reviewer).
  - **AI suggestions panel** (visible only when any `suggested_*` present and status is `submitted`): shows suggested amount/date/vendor/card with an "Use suggestion" button per field that copies the value into the review form inputs.
  - Receipt blobs: for each `blob_ids` entry, a link navigating to the existing `document-detail` view (`/fleet/documents/<id>` — confirm the path in `router.js`).
  - **Review form** (scope-gated `expenses:approve`, hidden when settled): amount input, approved-amount input, payment-method select, expense-date input, vendor input, note textarea; buttons "Approve all" (approved=amount), "Reject all" (approved=0), "Save review" → `POST ${API_BASE}/expenses/${id}/review`, re-render on success, error banner on failure.
  - **Create maintenance record** button: visible when `category === 'repair' && equipment_id && !maintenance_id`, scope `maintenance:write`; navigates to `maintenance-new` with query params `equipment_type, equipment_id, expense_id` (maintenance-form prefills from them and passes `expense_id` in the create body).
  - **Delete** button per ACL (scope `expenses:approve`, or `expenses:write` when own+submitted — own check: compare `submitted_by` to `fleet_user:<current user id>`; find how the SPA exposes the logged-in user id, see `utils/auth.js`; if unavailable, show Delete only for `expenses:approve` holders — note this simplification in the PR).
  - Reviewed-record edit: for `expenses:approve` holders, the same review form stays usable (re-review) until settled.

- [ ] **Step 5: Form page `pages/expense-form.js`.** Clone `maintenance-form.js`: category select (required), driver select, trip id text input (optional), equipment type+unit selects (paired), vendor, expense date, amount (optional, manager pre-fill), blob picker if maintenance-form has one (else omit — blobs attach via driver upload or MCP for v1; note in PR). Submit → `POST ${API_BASE}/expenses` → navigate to detail.

- [ ] **Step 6: Wiring.** `router.js` entries mirroring maintenance:

```js
  { name: 'expenses',        re: /^\/fleet\/expenses$/ },
  { name: 'expense-new',     re: /^\/fleet\/expenses\/new$/ },
  { name: 'expense-edit',    re: /^\/fleet\/expenses\/([^/]+)\/edit$/, id: true },
  { name: 'expense-detail',  re: /^\/fleet\/expenses\/([^/]+)$/, id: true },
```

`nav.js`: `{ label: 'Expenses', path: '/fleet/expenses', icon: expensesIcon, scope: 'expenses:read' }` next to Maintenance. `icons.js`: add `expensesIcon` — a simple receipt/dollar SVG matching the existing icon set's stroke style (copy an existing icon's SVG attributes, change the paths; no hex colors — `currentColor`). `app.js`: register the three views exactly as maintenance views are registered. `maintenance-form.js`: read `expense_id`/`equipment_*` prefill params, include `expense_id` in POST body when present. `maintenance-detail.js`: when `expense_id` present render a "Linked expense" row linking to `/fleet/expenses/<id>`, and suppress the cost edit field if the form supports inline cost edits.

- [ ] **Step 7: Run** — `npm test` (all fleet tests green). Manual smoke optional.

- [ ] **Step 8: Commit** — `git add static/fleet tests/fleet && git commit -s -m "feat(expenses): fleet SPA expenses section"` (+ co-author trailer).

---

### Task 12: Driver PWA — expense doctype + expense list

**Files:**
- Modify: `static/driver/pages/trip-detail.js` (doctype picker: replace Scale Ticket with Expense + category sub-picker; append `expense_category` to the upload FormData)
- Create: `static/driver/pages/expenses.js`
- Modify: `static/driver/pages/account.js` (nav link to Expenses list)
- Modify: driver router/app wiring (follow how `pay.js` is registered)
- Modify: `static/driver/sw.js` (`STATIC_ASSETS` += `pages/expenses.js` with the **current** `?v=` stamp — do NOT bump `CACHE_NAME` or stamps)

**Interfaces:**
- Consumes: `GET /driver/api/v1/expenses`, `DELETE /driver/api/v1/expenses/{id}`, upload endpoint from Task 9.

**Before writing code, read:** `static/driver/pages/trip-detail.js` (the `pendingDoctype` picker around line 200), `static/driver/pages/pay.js` (page registration + fetch pattern), `static/driver/components/icons.js`, `docs/DESIGN.md` tokens. **New-file rule: DOM construction only, `.textContent` for all API-derived values, no innerHTML.**

- [ ] **Step 1: Doctype picker.** In `trip-detail.js`, find where the doctype options are defined (the array/buttons feeding `pendingDoctype`). Replace the `scale_ticket` option with `{ value: 'expense', label: 'Expense' }`. When `expense` is selected, show a category `<select>` (or button grid matching the picker's existing style) with the nine categories from the spec (Fuel, Tolls, Scales, Lumper, Parking, Repair, Supplies, Permit, Other), default Other, stored in a `pendingExpenseCategory` variable. In the upload submit (near line 209 `form.append('doctype', pendingDoctype)`), add:

```js
      if (pendingDoctype === 'expense') {
        form.append('expense_category', pendingExpenseCategory || 'other');
      }
```

The doc list label logic (line ~255 `doctype:` tag parsing) needs no change — it will render "EXPENSE".

- [ ] **Step 2: Expenses page `pages/expenses.js`.** DOM-constructed list page: app bar "Expenses" with Back to account; fetch `GET /driver/api/v1/expenses` with the JWT (copy the authenticated fetch helper used by `pay.js`); render cards per expense: category label + status chip (`Needs review` / `Reviewed` / `Settled`), date (expense_date ?? created_at date), amount + approved amount when present, review note when present, reimbursement/deduction line when present ("Reimbursement: $80.00" / "Deducted: $20.00"), and a Delete button ONLY when `status === 'submitted'` → confirm → `DELETE /driver/api/v1/expenses/{id}` → re-render. Empty state text: "No expenses submitted yet." Page container follows the fixed-bottom-nav padding rule (`padding-bottom: var(--bottom-nav-height)`) if it renders under the bottom nav — copy whichever container class `pay.js` uses.

- [ ] **Step 3: Wire navigation.** `account.js`: add an "Expenses" row/button navigating to the expenses page, matching the existing account-page row style. Register the page in the driver router the same way `pay.js` is registered.

- [ ] **Step 4: `sw.js`.** Add `pages/expenses.js` (with the current version stamp used by every other entry) to `STATIC_ASSETS`. Do not touch `CACHE_NAME`.

- [ ] **Step 5: Verify** — serve locally (`cargo run` + browser, or the project's usual static check): trip-detail upload sheet shows Expense with category picker; account → Expenses renders. If a driver-side JS test harness exists by now (check `tests/driver/` for `.test.js` files), add list-rendering coverage following it; otherwise note manual verification in the PR.

- [ ] **Step 6: Commit** — `git add static/driver && git commit -s -m "feat(expenses): driver PWA expense upload category and expense list"` (+ co-author trailer).

---

### Task 13: Docs, llms.txt, final verification

**Files:**
- Modify: `src/api/mod.rs` (`LLMS_TXT`)
- Modify: `AGENTS.md` (only if a new hard-won invariant emerged during implementation)

- [ ] **Step 1: `LLMS_TXT`.** Add a concise Expenses section: entity lifecycle (`submitted → reviewed → settled`), the derivation rule (personal approved → reimbursement; company denied → deduction), fleet REST paths, driver paths, MCP tool names, scopes (`expenses:read/write/approve`), maintenance cross-link semantics (cost mirrors linked expense; `expense_id` unset + cost 0 = warranty). Keep it to the same terseness as the maintenance section.

- [ ] **Step 2: Full verification**

Run, in order, all from the worktree root:
```bash
cargo test --manifest-path Cargo.toml
cargo clippy --manifest-path Cargo.toml
npm test
```
Expected: all green (modulo the known `Config::from_env` parallel flake — rerun isolated to confirm). Also `curl`-check `GET /openapi.json` parses if the server is run locally (optional).

- [ ] **Step 3: Commit** — `git add -A && git commit -s -m "docs(expenses): llms.txt expense surface"` (+ co-author trailer).

- [ ] **Step 4: Ship.** Push branch, open PR to `main` titled "feat: expense tracking with driver receipts, partial approval, and maintenance linking", body summarizing the spec + linking `docs/superpowers/specs/2026-07-21-expense-tracking-design.md`, noting: settlement lock is a hook only; EFS import + resubmit loop out of scope; PWA cache stamps intentionally untouched (cut-release owns them). DCO sign-off on every commit. PR body ends with the standard generated-with footer.

---

## Self-Review Notes (already applied)

- **Spec coverage check:** entity+derived money (T2), lifecycle+lock (T3/T6), permissions (T1/T6/T9), AI extraction + cleared-at-review (T10/T6), driver flow incl. scale-ticket swap (T9/T12), fleet queue/history/detail/create (T5/T6/T11), maintenance two-way link + cost mirror + warranty semantics + backfill via `update_maintenance expense_id` + `create_expense maintenance_id` (T4/T7/T8), migration + test (T4), events (T5), MCP (T8), llms.txt/OpenAPI (T5/T6/T13). Historical-backfill mechanism = MCP tools from T8 (no extra code needed).
- **Known intentional simplifications** (call out in PR): fleet-SPA delete-button ownership check may degrade to approve-only if the SPA lacks a current-user-id accessor; expense-form has no blob picker in v1 unless maintenance-form already ships one.
- Type-consistency: `ExpenseFilter` needs `#[derive(Default)]` (used by Task 9's `..Default::default()`) — include it in Task 3.
