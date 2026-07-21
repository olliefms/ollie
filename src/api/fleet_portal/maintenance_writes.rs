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
    #[serde(default)]
    pub expense_id: Option<Uuid>,
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
    #[serde(default)]
    pub expense_id: Option<Uuid>,
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/maintenance",
    request_body(content = CreateMaintenanceBody, description = "Maintenance entry to create"),
    responses(
        (status = 201, description = "Created maintenance record", body = MaintenanceRecord),
        (status = 400, description = "Bad request — unknown field, blank description, unknown equipment, unknown expense, or cost set alongside expense_id"),
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
        (status = 400, description = "Bad request — unknown field, invalid body, unknown expense, or cost edit on an expense-linked record"),
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
/// `pub(crate)` so `expenses.rs` can reuse the same equipment-existence check.
pub(crate) async fn resolve_equipment_unit(
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

    let mut cost = parsed.cost;
    if let Some(expense_id) = parsed.expense_id {
        if parsed.cost.is_some() {
            return Err(AppError::BadRequest(
                "cost cannot be set directly when linking an expense".into(),
            ));
        }
        let expense = state.db.get_expense_by_id(expense_id).await
            .map_err(|_| AppError::BadRequest("unknown expense".into()))?;
        if expense.amount.is_some() {
            cost = expense.amount;
        }
    }

    let now = Utc::now();
    let record = MaintenanceRecord {
        id: Uuid::new_v4(),
        equipment_type: parsed.equipment_type,
        equipment_id: parsed.equipment_id,
        service_date: parsed.service_date,
        category: parsed.category,
        description: parsed.description,
        cost,
        odometer: parsed.odometer,
        vendor: parsed.vendor,
        invoice_ref: parsed.invoice_ref,
        blob_ids: parsed.blob_ids,
        expense_id: parsed.expense_id,
        embedding: None,
        owner_id: 0,
        created_at: now,
        updated_at: now,
    };

    let embed_input = format!("{} {}", unit, record.embedding_text());
    let embedding = embed_text(&state.ai, &embed_input).await.ok();
    let record = MaintenanceRecord { embedding, ..record };

    state.db.insert_maintenance(&record).await?;

    if let Some(expense_id) = record.expense_id {
        let mut expense = state.db.get_expense_by_id(expense_id).await?;
        expense.maintenance_id = Some(record.id);
        expense.updated_at = Utc::now();
        state.db.update_expense(&expense).await?;
    }

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

    let current = state.db.get_maintenance_by_id(id).await?;
    if current.expense_id.is_some() && parsed.cost.is_some() {
        return Err(AppError::BadRequest(
            "cost is managed by the linked expense".into(),
        ));
    }

    let mut cost = parsed.cost;
    if let Some(expense_id) = parsed.expense_id {
        let expense = state.db.get_expense_by_id(expense_id).await
            .map_err(|_| AppError::BadRequest("unknown expense".into()))?;
        if expense.amount.is_some() {
            cost = expense.amount;
        }
    }

    let updated = state.db.update_maintenance_metadata(
        id,
        parsed.service_date,
        parsed.category,
        parsed.description,
        cost,
        parsed.odometer,
        parsed.vendor,
        parsed.invoice_ref,
        parsed.blob_ids,
        parsed.expense_id,
    ).await?;

    if let Some(expense_id) = parsed.expense_id {
        let mut expense = state.db.get_expense_by_id(expense_id).await?;
        expense.maintenance_id = Some(updated.id);
        expense.updated_at = Utc::now();
        state.db.update_expense(&expense).await?;
    }

    // Refresh embedding best-effort, prepending unit number for searchability.
    if let Ok(unit) = resolve_equipment_unit(state, updated.equipment_type, updated.equipment_id).await {
        let embed_input = format!("{} {}", unit, updated.embedding_text());
        if let Ok(embedding) = embed_text(&state.ai, &embed_input).await {
            let _ = state.db.update_maintenance_embedding(id, embedding).await;
        }
    }

    Ok(updated)
}
