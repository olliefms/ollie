# Expense Tracking — Design Spec

**Date:** 2026-07-21
**Status:** Approved for planning

## Purpose

Track money spent operating the fleet: driver-submitted receipts, fleet-entered
invoices, approval with partial amounts, derived reimbursements/deductions for a
future driver-settlements feature, and two-way linkage with maintenance records so
a repair invoice is both a service-history record and a financial record.

## Core concepts

### The Expense entity

New first-class entity following the per-entity trio convention
(`models/expense.rs`, `db/expense_ops.rs`, API handlers). merge_insert upsert
pattern from day one.

| Field | Type | Notes |
|---|---|---|
| `id` | Uuid | |
| `status` | enum | `submitted` \| `reviewed` \| `settled` |
| `category` | enum | `fuel` \| `tolls` \| `scales` \| `lumper` \| `parking` \| `repair` \| `supplies` \| `permit` \| `other` |
| `driver_id` | Option\<Uuid\> | who incurred it; always set for driver-submitted |
| `trip_id` | Option\<Uuid\> | auto-set from driver upload context ("the trip they were on"); optional for fleet-entered |
| `equipment_type` + `equipment_id` | Option | truck/trailer the expense applies to |
| `maintenance_id` | Option\<Uuid\> | cross-link to maintenance record |
| `blob_ids` | Vec\<Uuid\> | receipt/invoice images and PDFs |
| `submitted_by` | string | ownership: `driver:<uuid>` or `fleet_user:<id>` — drives the own-record ACL |
| `expense_date` | Option\<String\> | set at review (AI-suggested) |
| `vendor` | Option\<String\> | set at review (AI-suggested) |
| `amount` | Option\<f64\> | receipt total, USD; set at review |
| `approved_amount` | Option\<f64\> | 0 ≤ approved_amount ≤ amount; set at review |
| `payment_method` | Option\<enum\> | `company` \| `personal`; set at review. `company` covers card, check, comcheck, ACH — any company funds |
| `suggested_amount` / `suggested_date` / `suggested_vendor` / `suggested_card_last4` | Option | staged by AI pipeline; never authoritative; **cleared at review** (manager promotes into real fields or discards) |
| `reviewed_by` / `reviewed_at` / `review_note` | Option | review metadata |
| `settlement_id` | Option\<Uuid\> | set by the future settlements feature; locks the record |
| `embedding` | Option\<Vec\<f32\>\> | via `embedding_text()`, like maintenance — semantic search over expenses |
| `created_at` / `updated_at` | DateTime\<Utc\> | |

### Derived money effects (never stored)

Partial approval is the general case; approve-all and reject-all are its endpoints.

- **Reimbursement** (add to driver settlement): `approved_amount` when `payment_method = personal`
- **Deduction** (subtract from driver settlement): `amount − approved_amount` when `payment_method = company`
- **Disposition** (approved / partial / rejected) derives from `approved_amount` vs `amount`

| | Approved portion | Denied portion |
|---|---|---|
| **company** | normal business expense, no pay impact | deduction from settlement |
| **personal** | reimbursement on settlement | no pay impact |

### Lifecycle

```
submitted → reviewed → settled
```

- **submitted:** created by driver upload or fleet entry. No amount yet — review
  is where money fields get set. The fleet review queue is the heartbeat.
- **reviewed:** manager set `amount`, `approved_amount`, `payment_method`,
  optional `review_note`; suggested_* fields cleared. Managers may re-edit
  reviewed records until settled.
- **settled:** future settlements feature sets `settlement_id`; record locks
  permanently. Nothing else about settlements is built now — this is the hook.

Totals and reports only count reviewed/settled expenses (submitted records have
no authoritative amount).

### Permissions

Fleet scopes (fleet users; scope-based ACL):

- `expenses:read` — view queue and full history
- `expenses:write` — create expenses, attach blobs, edit/delete **own**
  (submitted_by self) un-reviewed records
- `expenses:approve` — review (set amounts/payment method, approve/reject),
  edit/delete **any** record until settled

Role defaults: dispatcher = read + write; fleet manager = all three.

Drivers use the driver portal (separate JWT surface, self-scoped by
construction): create via receipt upload; list own expenses including reviewed
outcomes (status, approved amount, review note — read-only once reviewed);
edit/delete own records only while `submitted`.

## AI extraction

Pipeline addition: blobs tagged with the expense doctype get a structured-
extraction pass after the normal summary — prompt the model for JSON
`{amount, date, vendor, card_last4}` and stage the results as `suggested_*` on
the linked expense. Extraction failure is non-fatal (blank suggestions; manager
enters values manually). Suggestions are review-time scaffolding only and are
cleared when the manager reviews.

## Driver PWA

- Doctype picker: **Scale Ticket is replaced by Expense** (scales lives on as an
  expense category). Existing scale-ticket blobs keep their old tag.
- Expense upload flow: pick Expense → pick category → snap/attach photo. No
  amount or payment-method entry by the driver.
- Upload creates blob + expense (`submitted`, auto-linked to driver and current
  trip).
- Driver-facing expense list shows their submissions with status/outcome and
  allows delete-before-review.

## Fleet SPA + API

New Expenses section:

- **Review queue** — submitted expenses, oldest first.
- **History** — all expenses with filters: driver, truck/trailer, trip,
  category, status, date range.
- **Detail view** — receipt preview, AI suggestions, review form: amount,
  approve all / reject all / partial (prompts for approved amount), payment
  method, note. When category = repair and equipment is set, offers
  **"also create maintenance record"** pre-filled from the expense.
- **New expense form** — fleet-created expense with blob attach and optional
  driver/trip/equipment links.

MCP tools mirroring REST: `create_expense`, `list_expenses`, `get_expense`,
`update_expense`, `review_expense`, `delete_expense`. All endpoints get utoipa
annotations, ApiDoc registration, and an llms.txt update per house rules.

## Maintenance linkage

- Cross-reference both ways: `expense.maintenance_id` and
  `maintenance.expense_id`.
- When linked, maintenance cost defers to the linked expense (read-only mirror
  in responses); the expense is the single source of financial truth.
- Semantics of maintenance records:
  - `expense_id` set → cost lives on the expense
  - `expense_id` unset, `cost: Some(0.0)` → explicit no-charge (warranty,
    goodwill)
  - `expense_id` unset, `cost: Some(x)` → legacy record, not yet migrated
    (valid; migration is opt-in)
  - `expense_id` unset, `cost: None` → cost unknown/unrecorded
- `update_maintenance` accepts `expense_id`; `create_expense` accepts
  `maintenance_id` (pre-filling vendor/date/blobs from the maintenance record).
  Historical backfill is done conversationally via MCP by a fleet manager or
  their agent — never forced.

## Migration

- Expense table is new — no migration.
- Maintenance table gains `expense_id`: dedicated `open_or_create_maintenance`
  migration via `add_columns(SqlExpressions)` using `CAST(NULL AS string)`
  (DataFusion SQL type names, never Arrow names), and `tests/migration_test.rs`
  extended per the standing rule (seed pre-change schema, assert round-trip).

## Events

Append journal events for the ops feed: `expense_submitted`,
`expense_reviewed`, `expense_deleted` (and `expense_settled` when settlements
arrive).

## Testing

Integration tests across both surfaces:

- Driver: submit via upload, list own, delete own un-reviewed, blocked from
  editing reviewed.
- Fleet: create, review with full/partial/zero approval, derived
  reimbursement/deduction math, suggested_* cleared on review, scope
  enforcement (write vs approve, own vs any).
- Settlement lock: record with `settlement_id` rejects all mutation.
- Maintenance link round-trip both directions; cost-mirror behavior.
- Migration test extension for `maintenance.expense_id`.

## Out of scope (v1)

- Driver settlements themselves (only the `settlement_id` hook ships).
- Fuel-card (EFS) transaction import/reconciliation.
- Driver resubmit/dispute loop after rejection (handled out of band).
- Multi-currency (USD assumed).
