// src/api/fleet_portal/terminal_writes.rs
//
// Fleet-portal terminal CRUD endpoints (#185):
//   POST   /fleet/api/v1/terminals
//   GET    /fleet/api/v1/terminals
//   GET    /fleet/api/v1/terminals/{id}
//   PUT    /fleet/api/v1/terminals/{id}
//   DELETE /fleet/api/v1/terminals/{id}

use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Extension, Json};
use chrono::Utc;
use uuid::Uuid;

use super::jwt::FleetUserClaims;

use crate::error::AppError;
use crate::models::terminal::{CreateTerminalRequest, UpdateTerminalRequest};
use crate::models::TerminalRecord;
use crate::AppState;

fn validate_tz(tz: &str) -> Result<(), AppError> {
    tz.parse::<chrono_tz::Tz>()
        .map(|_| ())
        .map_err(|_| AppError::UnprocessableEntity(format!("invalid IANA timezone: {tz}")))
}

/// Shared create writer — used by the HTTP handler (and optionally MCP).
pub async fn apply_terminal_create(
    state: &AppState,
    req: CreateTerminalRequest,
) -> Result<TerminalRecord, AppError> {
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

/// Shared patch writer — used by the HTTP handler (and optionally MCP).
pub async fn apply_terminal_patch(
    state: &AppState,
    id: Uuid,
    req: UpdateTerminalRequest,
) -> Result<TerminalRecord, AppError> {
    let mut t = state.db.get_terminal_by_id(id).await?;
    if let Some(tz) = req.timezone {
        validate_tz(&tz)?;
        t.timezone = tz;
    }
    if let Some(v) = req.name {
        t.name = v;
    }
    // Some(Some(s)) = set, Some(None) = clear to null, None = leave unchanged.
    if let Some(addr) = req.address {
        t.address = addr;
    }
    if let Some(v) = req.is_default {
        // Preserve the single-default invariant: you can't directly clear the
        // default flag (that would leave zero defaults). Promote another terminal
        // to default instead (set_terminal clears the previous one).
        if !v && t.is_default {
            return Err(AppError::Conflict(
                "cannot unset the default terminal; set another terminal as default instead".into(),
            ));
        }
        t.is_default = v;
    }
    if let Some(v) = req.loaded_rate_per_mile {
        t.loaded_rate_per_mile = v;
    }
    if let Some(v) = req.deadhead_rate_per_mile {
        t.deadhead_rate_per_mile = v;
    }
    if let Some(v) = req.extra_stop_fee {
        t.extra_stop_fee = v;
    }
    if let Some(v) = req.detention_rate_per_hour {
        t.detention_rate_per_hour = v;
    }
    if let Some(v) = req.free_dwell_minutes {
        t.free_dwell_minutes = v;
    }
    t.updated_at = Utc::now();
    state.db.set_terminal(&t).await?;
    Ok(t)
}

#[utoipa::path(
    post,
    path = "/fleet/api/v1/terminals",
    request_body(content = CreateTerminalRequest, description = "Terminal to create"),
    responses(
        (status = 201, description = "Created terminal record", body = TerminalRecord),
        (status = 401, description = "Unauthorized"),
        (status = 422, description = "Invalid timezone or request"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn create_terminal(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Json(req): Json<CreateTerminalRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("terminals:write")?;
    let r = apply_terminal_create(&state, req).await?;
    Ok((StatusCode::CREATED, Json(r)))
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/terminals",
    responses(
        (status = 200, description = "List of terminals", body = [TerminalListItem]),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn list_terminals(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("terminals:read")?;
    Ok(Json(state.db.list_terminals().await?))
}

#[utoipa::path(
    get,
    path = "/fleet/api/v1/terminals/{id}",
    params(("id" = Uuid, Path, description = "Terminal UUID")),
    responses(
        (status = 200, description = "Terminal record", body = TerminalRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Terminal not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn get_terminal(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("terminals:read")?;
    Ok(Json(state.db.get_terminal_by_id(id).await?))
}

#[utoipa::path(
    put,
    path = "/fleet/api/v1/terminals/{id}",
    params(("id" = Uuid, Path, description = "Terminal UUID")),
    request_body(content = UpdateTerminalRequest, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated terminal record", body = TerminalRecord),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Terminal not found"),
        (status = 422, description = "Invalid timezone"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn update_terminal(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateTerminalRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("terminals:write")?;
    Ok(Json(apply_terminal_patch(&state, id, req).await?))
}

#[utoipa::path(
    delete,
    path = "/fleet/api/v1/terminals/{id}",
    params(("id" = Uuid, Path, description = "Terminal UUID")),
    responses(
        (status = 204, description = "Terminal deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Terminal not found"),
        (status = 409, description = "Conflict — default terminal or has assigned drivers"),
    ),
    security(("BearerAuth" = [])),
    tag = "fleet"
)]
pub async fn delete_terminal(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("terminals:delete")?;
    let t = state.db.get_terminal_by_id(id).await?;
    if t.is_default {
        return Err(AppError::Conflict(
            "cannot delete the default terminal".into(),
        ));
    }
    if state.db.count_drivers_for_terminal(id).await? > 0 {
        return Err(AppError::Conflict(
            "terminal has assigned drivers".into(),
        ));
    }
    state.db.delete_terminal(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
