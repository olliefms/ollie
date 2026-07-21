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
    models::{EquipmentType, ExpenseCategory, ExpenseListResponse, ExpenseRecord, ExpenseResponse, ExpenseStatus},
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
