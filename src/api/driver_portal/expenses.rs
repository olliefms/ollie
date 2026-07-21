// src/api/driver_portal/expenses.rs
//
// Driver-portal expense endpoints: a driver may list their own submitted
// expenses (created via receipt upload, see documents.rs) and delete their
// own un-reviewed ones.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    api::driver_portal::jwt::DriverClaims,
    db::expense_ops::ExpenseFilter,
    error::AppError,
    events,
    models::{ExpenseListResponse, ExpenseResponse, ExpenseStatus},
    AppState,
};

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct DriverExpenseListQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[utoipa::path(
    get,
    path = "/driver/api/v1/expenses",
    params(
        ("status" = Option<String>, Query, description = "Filter by status (submitted/reviewed/settled)"),
        ("limit" = Option<usize>, Query, description = "Maximum results (default 100, max 1000)"),
        ("offset" = Option<usize>, Query, description = "Pagination offset (default 0)"),
    ),
    responses(
        (status = 200, description = "This driver's own expense records", body = ExpenseListResponse),
        (status = 400, description = "Bad request — unknown status"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn list_expenses(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Query(q): Query<DriverExpenseListQuery>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims
        .driver_id
        .parse::<Uuid>()
        .map_err(|_| AppError::Unauthorized)?;

    if let Some(ref s) = q.status {
        s.parse::<ExpenseStatus>().map_err(AppError::BadRequest)?;
    }

    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let offset = q.offset.unwrap_or(0);

    let filter = ExpenseFilter {
        submitted_by: Some(format!("driver:{driver_id}")),
        status: q.status,
        ..Default::default()
    };

    let (total, items) = state.db.list_expenses(&filter, limit, offset).await?;
    Ok(Json(ExpenseListResponse {
        returned: items.len(),
        total,
        items: items.into_iter().map(ExpenseResponse::from).collect(),
    }))
}

#[utoipa::path(
    delete,
    path = "/driver/api/v1/expenses/{id}",
    params(("id" = Uuid, Path, description = "Expense UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — reviewed expenses can no longer be deleted"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — expense is settled and locked"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn delete_expense(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims
        .driver_id
        .parse::<Uuid>()
        .map_err(|_| AppError::Unauthorized)?;
    let actor = format!("driver:{driver_id}");

    let record = state.db.get_expense_by_id(id).await?;
    if record.submitted_by != actor {
        return Err(AppError::NotFound);
    }
    if matches!(record.status, ExpenseStatus::Settled) || record.is_locked() {
        return Err(AppError::Conflict("expense is settled and locked".into()));
    }
    if matches!(record.status, ExpenseStatus::Reviewed) {
        return Err(AppError::Forbidden(
            "reviewed expenses can no longer be deleted".into(),
        ));
    }

    // Sever the 1:1 back-link so the maintenance entry isn't left pointing at a
    // deleted expense (which would brick its cost). Best-effort.
    if let Some(maintenance_id) = record.maintenance_id {
        let _ = state.db.clear_maintenance_expense_link(maintenance_id).await;
    }
    state.db.delete_expense(id).await?;
    events::expense_deleted(&state.db, id, Some(actor)).await;
    Ok(StatusCode::NO_CONTENT)
}
