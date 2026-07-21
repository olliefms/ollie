// src/api/fleet_portal/expenses.rs
//
// Fleet-portal expense endpoints:
//   - POST /fleet/api/v1/expenses
//   - GET  /fleet/api/v1/expenses
//   - GET  /fleet/api/v1/expenses/{id}
//
// `apply_expense_create` is shared with MCP/driver-portal callers so validation
// and side effects (embedding, maintenance back-link, event) stay in one place.

use axum::{
    extract::{Path, Query, State},
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
    db::expense_ops::ExpenseFilter,
    error::AppError,
    events,
    models::{
        permission::scope_granted, EquipmentType, ExpenseCategory, ExpenseListResponse,
        ExpenseRecord, ExpenseResponse, ExpenseStatus, PaymentMethod,
    },
    AppState,
};

use super::maintenance_writes::resolve_equipment_unit;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateExpenseBody {
    pub category: ExpenseCategory,
    #[serde(default)]
    pub driver_id: Option<Uuid>,
    #[serde(default)]
    pub trip_id: Option<Uuid>,
    #[serde(default)]
    pub equipment_type: Option<EquipmentType>,
    #[serde(default)]
    pub equipment_id: Option<Uuid>,
    #[serde(default)]
    pub maintenance_id: Option<Uuid>,
    #[serde(default)]
    pub blob_ids: Vec<Uuid>,
    #[serde(default)]
    pub expense_date: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub amount: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct ExpenseListQuery {
    pub status: Option<String>,
    pub category: Option<String>,
    pub driver_id: Option<Uuid>,
    pub trip_id: Option<Uuid>,
    pub equipment_id: Option<Uuid>,
    pub submitted_by: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn apply_expense_create(
    state: &AppState,
    body: Value,
    submitted_by: String,
) -> Result<ExpenseRecord, AppError> {
    let parsed: CreateExpenseBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if let Some(amount) = parsed.amount {
        if amount < 0.0 {
            return Err(AppError::BadRequest("amount must not be negative".into()));
        }
    }

    if parsed.equipment_type.is_some() != parsed.equipment_id.is_some() {
        return Err(AppError::BadRequest(
            "equipment_type and equipment_id must be provided together".into(),
        ));
    }
    if let (Some(equipment_type), Some(equipment_id)) = (parsed.equipment_type, parsed.equipment_id) {
        resolve_equipment_unit(state, equipment_type, equipment_id).await?;
    }

    if let Some(driver_id) = parsed.driver_id {
        state.db.get_driver_by_id(driver_id).await
            .map_err(|_| AppError::BadRequest(format!("unknown driver: {driver_id}")))?;
    }
    if let Some(trip_id) = parsed.trip_id {
        state.db.get_trip(trip_id).await
            .map_err(|_| AppError::BadRequest(format!("unknown trip: {trip_id}")))?;
    }
    if let Some(maintenance_id) = parsed.maintenance_id {
        state.db.get_maintenance_by_id(maintenance_id).await
            .map_err(|_| AppError::BadRequest(format!("unknown maintenance entry: {maintenance_id}")))?;
    }

    let now = Utc::now();
    let record = ExpenseRecord {
        id: Uuid::new_v4(),
        status: ExpenseStatus::Submitted,
        category: parsed.category,
        driver_id: parsed.driver_id,
        trip_id: parsed.trip_id,
        equipment_type: parsed.equipment_type,
        equipment_id: parsed.equipment_id,
        maintenance_id: parsed.maintenance_id,
        blob_ids: parsed.blob_ids,
        submitted_by,
        expense_date: parsed.expense_date,
        vendor: parsed.vendor,
        amount: parsed.amount,
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

    let embedding = embed_text(&state.ai, &record.embedding_text()).await.ok();
    let record = ExpenseRecord { embedding, ..record };

    state.db.insert_expense(&record).await?;

    if let Some(maintenance_id) = record.maintenance_id {
        state.db.update_maintenance_metadata(
            maintenance_id,
            None, None, None, None, None, None, None, None,
            Some(record.id),
        ).await?;
    }

    events::expense_submitted(&state.db, record.id, Some(record.submitted_by.clone())).await;

    Ok(record)
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/expenses",
    request_body(content = CreateExpenseBody, description = "Expense to submit"),
    responses(
        (status = 201, description = "Created expense record", body = ExpenseResponse),
        (status = 400, description = "Bad request — invalid body, unknown category, or unknown link"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
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

#[utoipa::path(
    get,
    path = "/fleet/api/v1/expenses",
    params(
        ("status" = Option<String>, Query, description = "Filter by status (submitted/reviewed/settled)"),
        ("category" = Option<String>, Query, description = "Filter by category"),
        ("driver_id" = Option<Uuid>, Query, description = "Filter by driver UUID"),
        ("trip_id" = Option<Uuid>, Query, description = "Filter by trip UUID"),
        ("equipment_id" = Option<Uuid>, Query, description = "Filter by equipment UUID"),
        ("submitted_by" = Option<String>, Query, description = "Filter by submitter marker"),
        ("from" = Option<String>, Query, description = "Effective date range start (YYYY-MM-DD, inclusive)"),
        ("to" = Option<String>, Query, description = "Effective date range end (YYYY-MM-DD, inclusive)"),
        ("limit" = Option<usize>, Query, description = "Maximum results (default 100, max 1000)"),
        ("offset" = Option<usize>, Query, description = "Pagination offset (default 0)"),
    ),
    responses(
        (status = 200, description = "List of expenses", body = ExpenseListResponse),
        (status = 400, description = "Bad request — unknown status or category"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn list_expenses_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Query(q): Query<ExpenseListQuery>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("expenses:read")?;

    if let Some(ref s) = q.status {
        s.parse::<ExpenseStatus>().map_err(AppError::BadRequest)?;
    }
    if let Some(ref c) = q.category {
        c.parse::<ExpenseCategory>().map_err(AppError::BadRequest)?;
    }

    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let offset = q.offset.unwrap_or(0);

    let filter = ExpenseFilter {
        status: q.status,
        category: q.category,
        driver_id: q.driver_id.map(|id| id.to_string()),
        trip_id: q.trip_id.map(|id| id.to_string()),
        equipment_id: q.equipment_id.map(|id| id.to_string()),
        submitted_by: q.submitted_by,
        from: q.from,
        to: q.to,
    };

    let (total, items) = state.db.list_expenses(&filter, limit, offset).await?;
    Ok(Json(ExpenseListResponse {
        returned: items.len(),
        total,
        items: items.into_iter().map(ExpenseResponse::from).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/expenses/{id}",
    params(("id" = Uuid, Path, description = "Expense UUID")),
    responses(
        (status = 200, description = "Expense record", body = ExpenseResponse),
        (status = 404, description = "Not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_expense_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("expenses:read")?;
    let record = state.db.get_expense_by_id(id).await?;
    Ok(Json(ExpenseResponse::from(record)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewExpenseBody {
    pub amount: f64,
    pub approved_amount: f64,
    pub payment_method: PaymentMethod,
    #[serde(default)]
    pub expense_date: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub review_note: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchExpenseBody {
    #[serde(default)]
    pub category: Option<ExpenseCategory>,
    #[serde(default)]
    pub driver_id: Option<Uuid>,
    #[serde(default)]
    pub trip_id: Option<Uuid>,
    #[serde(default)]
    pub equipment_type: Option<EquipmentType>,
    #[serde(default)]
    pub equipment_id: Option<Uuid>,
    #[serde(default)]
    pub maintenance_id: Option<Uuid>,
    #[serde(default)]
    pub blob_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    pub expense_date: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default)]
    pub approved_amount: Option<f64>,
    #[serde(default)]
    pub payment_method: Option<PaymentMethod>,
    #[serde(default)]
    pub review_note: Option<String>,
}

/// True when `actor` (a `driver:<uuid>` / `fleet_user:<uuid>` marker) submitted
/// this record.
pub(crate) fn is_expense_owner(actor: &str, record: &ExpenseRecord) -> bool {
    record.submitted_by == actor
}

/// Mutation authorization. `actor` is "fleet_user:<uuid>" or "driver:<uuid>".
/// Settled -> Conflict. Reviewed -> needs expenses:approve. Submitted -> approve
/// OR (expenses:write AND owner).
pub(crate) fn authorize_expense_mutation(
    record: &ExpenseRecord,
    actor: &str,
    scopes: &[String],
) -> Result<(), AppError> {
    if record.is_locked() || matches!(record.status, ExpenseStatus::Settled) {
        return Err(AppError::Conflict("expense is settled and locked".into()));
    }
    if scope_granted(scopes, "expenses:approve") {
        return Ok(());
    }
    if matches!(record.status, ExpenseStatus::Reviewed) {
        return Err(AppError::Forbidden(
            "reviewed expenses require expenses:approve".into(),
        ));
    }
    if scope_granted(scopes, "expenses:write") && is_expense_owner(actor, record) {
        return Ok(());
    }
    Err(AppError::Forbidden("not the submitter of this expense".into()))
}

pub(crate) async fn apply_expense_review(
    state: &AppState,
    id: Uuid,
    body: Value,
    reviewer_fleet_user_id: String,
) -> Result<ExpenseRecord, AppError> {
    let parsed: ReviewExpenseBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    if parsed.amount < 0.0 {
        return Err(AppError::BadRequest("amount must not be negative".into()));
    }
    if parsed.approved_amount < 0.0 || parsed.approved_amount > parsed.amount {
        return Err(AppError::BadRequest(
            "approved_amount must be between 0 and amount".into(),
        ));
    }

    let mut record = state.db.get_expense_by_id(id).await?;
    // The handler has already required expenses:approve; only the settlement lock
    // can still block the review here.
    if record.is_locked() || matches!(record.status, ExpenseStatus::Settled) {
        return Err(AppError::Conflict("expense is settled and locked".into()));
    }

    record.amount = Some(parsed.amount);
    record.approved_amount = Some(parsed.approved_amount);
    record.payment_method = Some(parsed.payment_method);
    if let Some(d) = parsed.expense_date {
        record.expense_date = Some(d);
    }
    if let Some(v) = parsed.vendor {
        record.vendor = Some(v);
    }
    if let Some(n) = parsed.review_note {
        record.review_note = Some(n);
    }
    record.status = ExpenseStatus::Reviewed;
    let reviewer_marker = format!("fleet_user:{reviewer_fleet_user_id}");
    record.reviewed_by = Some(reviewer_fleet_user_id);
    record.reviewed_at = Some(Utc::now());
    record.suggested_amount = None;
    record.suggested_date = None;
    record.suggested_vendor = None;
    record.suggested_card_last4 = None;
    record.updated_at = Utc::now();

    state.db.update_expense(&record).await?;

    // Maintenance cost mirror: a reviewed repair's amount is the entry's cost.
    if let Some(maintenance_id) = record.maintenance_id {
        state.db.update_maintenance_metadata(
            maintenance_id,
            None, None, None, record.amount, None, None, None, None, None,
        ).await?;
    }

    if let Ok(embedding) = embed_text(&state.ai, &record.embedding_text()).await {
        let _ = state.db.update_expense_embedding(record.id, embedding).await;
    }

    let payload = serde_json::json!({
        "amount": parsed.amount,
        "approved_amount": parsed.approved_amount,
        "payment_method": parsed.payment_method.as_str(),
    });
    events::expense_reviewed(&state.db, id, payload, Some(reviewer_marker)).await;

    // Re-fetch after multiple mutations (embedding update touched the row).
    state.db.get_expense_by_id(id).await
}

pub(crate) async fn apply_expense_patch(
    state: &AppState,
    id: Uuid,
    body: Value,
    actor: String,
    scopes: &[String],
) -> Result<ExpenseRecord, AppError> {
    let parsed: PatchExpenseBody = serde_json::from_value(body)
        .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?;

    let mut record = state.db.get_expense_by_id(id).await?;
    authorize_expense_mutation(&record, &actor, scopes)?;

    let touches_money = parsed.amount.is_some()
        || parsed.approved_amount.is_some()
        || parsed.payment_method.is_some()
        || parsed.review_note.is_some();
    if touches_money && !scope_granted(scopes, "expenses:approve") {
        return Err(AppError::Forbidden(
            "editing money fields requires expenses:approve".into(),
        ));
    }

    // Some-wins field updates.
    if let Some(v) = parsed.category {
        record.category = v;
    }
    if let Some(v) = parsed.driver_id {
        record.driver_id = Some(v);
    }
    if let Some(v) = parsed.trip_id {
        record.trip_id = Some(v);
    }
    if let Some(v) = parsed.equipment_type {
        record.equipment_type = Some(v);
    }
    if let Some(v) = parsed.equipment_id {
        record.equipment_id = Some(v);
    }
    let maintenance_newly_set =
        parsed.maintenance_id.is_some() && parsed.maintenance_id != record.maintenance_id;
    if let Some(v) = parsed.maintenance_id {
        record.maintenance_id = Some(v);
    }
    if let Some(v) = parsed.blob_ids {
        record.blob_ids = v;
    }
    if let Some(v) = parsed.expense_date {
        record.expense_date = Some(v);
    }
    if let Some(v) = parsed.vendor {
        record.vendor = Some(v);
    }
    let amount_changed = parsed.amount.is_some();
    if let Some(v) = parsed.amount {
        record.amount = Some(v);
    }
    if let Some(v) = parsed.approved_amount {
        record.approved_amount = Some(v);
    }
    if let Some(v) = parsed.payment_method {
        record.payment_method = Some(v);
    }
    if let Some(v) = parsed.review_note {
        record.review_note = Some(v);
    }

    if let Some(a) = record.amount {
        if a < 0.0 {
            return Err(AppError::BadRequest("amount must not be negative".into()));
        }
    }
    if record.equipment_type.is_some() != record.equipment_id.is_some() {
        return Err(AppError::BadRequest(
            "equipment_type and equipment_id must be provided together".into(),
        ));
    }
    if parsed.equipment_type.is_some() || parsed.equipment_id.is_some() {
        if let (Some(t), Some(eid)) = (record.equipment_type, record.equipment_id) {
            resolve_equipment_unit(state, t, eid).await?;
        }
    }
    if let Some(driver_id) = parsed.driver_id {
        state.db.get_driver_by_id(driver_id).await
            .map_err(|_| AppError::BadRequest(format!("unknown driver: {driver_id}")))?;
    }
    if let Some(trip_id) = parsed.trip_id {
        state.db.get_trip(trip_id).await
            .map_err(|_| AppError::BadRequest(format!("unknown trip: {trip_id}")))?;
    }
    if let Some(maintenance_id) = parsed.maintenance_id {
        state.db.get_maintenance_by_id(maintenance_id).await
            .map_err(|_| AppError::BadRequest(format!("unknown maintenance entry: {maintenance_id}")))?;
    }
    if let (Some(t), Some(a)) = (record.amount, record.approved_amount) {
        if a > t {
            return Err(AppError::BadRequest(
                "approved_amount must not exceed amount".into(),
            ));
        }
    }

    record.updated_at = Utc::now();
    state.db.update_expense(&record).await?;

    // Newly-linked maintenance entry gets the back-reference.
    if maintenance_newly_set {
        if let Some(maintenance_id) = record.maintenance_id {
            state.db.update_maintenance_metadata(
                maintenance_id,
                None, None, None, None, None, None, None, None,
                Some(record.id),
            ).await?;
        }
    }
    // A reviewed expense whose amount moved re-mirrors cost onto its entry.
    if matches!(record.status, ExpenseStatus::Reviewed) && amount_changed {
        if let Some(maintenance_id) = record.maintenance_id {
            state.db.update_maintenance_metadata(
                maintenance_id,
                None, None, None, record.amount, None, None, None, None, None,
            ).await?;
        }
    }

    if let Ok(embedding) = embed_text(&state.ai, &record.embedding_text()).await {
        let _ = state.db.update_expense_embedding(record.id, embedding).await;
    }

    state.db.get_expense_by_id(id).await
}

pub(crate) async fn apply_expense_delete(
    state: &AppState,
    id: Uuid,
    actor: String,
    scopes: &[String],
) -> Result<(), AppError> {
    let record = state.db.get_expense_by_id(id).await?;
    authorize_expense_mutation(&record, &actor, scopes)?;
    state.db.delete_expense(id).await?;
    events::expense_deleted(&state.db, id, Some(actor)).await;
    Ok(())
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/expenses/{id}/review",
    params(("id" = Uuid, Path, description = "Expense UUID")),
    request_body(content = ReviewExpenseBody, description = "Review decision"),
    responses(
        (status = 200, description = "Reviewed expense record", body = ExpenseResponse),
        (status = 400, description = "Bad request — invalid amounts"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — expenses:approve required"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — expense is settled and locked"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn review_expense_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("expenses:approve")?;
    let record = apply_expense_review(&state, id, body, claims.fleet_user_id.clone()).await?;
    Ok(Json(ExpenseResponse::from(record)))
}

#[utoipa::path(
    patch,
    path = "/fleet/api/v1/expenses/{id}",
    params(("id" = Uuid, Path, description = "Expense UUID")),
    request_body(content = PatchExpenseBody, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated expense record", body = ExpenseResponse),
        (status = 400, description = "Bad request — invalid body or unknown link"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — not the submitter, or money fields without expenses:approve"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — expense is settled and locked"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn patch_expense_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("expenses:write")?;
    let actor = format!("fleet_user:{}", claims.fleet_user_id);
    let record = apply_expense_patch(&state, id, body, actor, &claims.effective_scopes).await?;
    Ok(Json(ExpenseResponse::from(record)))
}

#[utoipa::path(
    delete,
    path = "/fleet/api/v1/expenses/{id}",
    params(("id" = Uuid, Path, description = "Expense UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — not the submitter"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — expense is settled and locked"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn delete_expense_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("expenses:write")?;
    let actor = format!("fleet_user:{}", claims.fleet_user_id);
    apply_expense_delete(&state, id, actor, &claims.effective_scopes).await?;
    Ok(StatusCode::NO_CONTENT)
}
