// src/api/dispatcher_portal/users.rs
//
// Fleet users management surface (#331), on the dispatcher portal — HTTP + MCP.
// This replaces the admin `/api/v1/dispatchers*` provisioning path with a
// dispatcher-portal surface gated by `users:*` scopes (owner + fleet_manager
// only) and layered with owner-protection rules.
//
// The underlying model is still the existing `DispatcherRecord` (users ==
// dispatchers-with-roles). The dispatcher→user rename is a separate future
// issue; here we keep the record/table name and expose it as "users".
//
// Business logic lives in the `apply_*` shared fns so the MCP tools in mcp.rs
// reuse the exact same DB ops, bcrypt(12) credential creation, and owner
// protection. Each HTTP handler calls `claims.require_scope("users:...")?`
// (chunk-2 mechanism) and passes the caller's `Role` for transfer checks.

use crate::{
    error::AppError,
    models::{
        permission::Role, DispatcherCredentials, DispatcherRecord, DispatcherStatus,
    },
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::jwt::DispatcherClaims;

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUserRequest {
    pub email: String,
    pub name: String,
    pub password: String,
    pub role: Role,
    #[serde(default)]
    pub extra_scopes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateUserRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<DispatcherStatus>,
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub extra_scopes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetUserPasswordRequest {
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserListResponse {
    pub users: Vec<DispatcherRecord>,
    pub returned: usize,
}

// ---------------------------------------------------------------------------
// Owner-protection helpers (shared by HTTP + MCP)
// ---------------------------------------------------------------------------

/// Count the active owners in the fleet. The at-least-one-owner invariant is
/// stated in terms of *active* owners — an Inactive owner does not satisfy it.
fn count_active_owners(users: &[DispatcherRecord]) -> usize {
    users
        .iter()
        .filter(|u| u.role == Role::Owner && u.status == DispatcherStatus::Active)
        .count()
}

/// Validate that the caller may grant each requested `extra_scopes` entry.
/// A caller can never grant a capability they do not themselves hold, and only
/// the owner may grant user-management (`users:*`) or superuser (`*`) scopes —
/// this prevents a fleet_manager from minting a de-facto admin (shadow admin).
fn validate_grantable_scopes(
    caller_effective: &[String],
    caller_role: Role,
    requested: &[String],
) -> Result<(), AppError> {
    for s in requested {
        // Can't grant a capability you don't yourself hold.
        if !crate::models::permission::scope_granted(caller_effective, s) {
            return Err(AppError::Forbidden(format!(
                "cannot grant a scope you do not hold: {s}"
            )));
        }
        // Only the owner may grant user-management or superuser scopes (prevents shadow admins).
        let elevated = s == "*" || s.starts_with("users:");
        if elevated && caller_role != Role::Owner {
            return Err(AppError::Forbidden(format!(
                "only the owner can grant scope: {s}"
            )));
        }
    }
    Ok(())
}

/// bcrypt(12) a password off the async runtime, mirroring admin create_dispatcher.
async fn hash_password(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || bcrypt::hash(&password, 12u32))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(|e| AppError::Internal(e.to_string()))
}

// ---------------------------------------------------------------------------
// Shared apply_* logic (HTTP handlers + MCP tools both call these)
// ---------------------------------------------------------------------------

pub async fn apply_list_users(state: &AppState) -> Result<Vec<DispatcherRecord>, AppError> {
    state.db.list_dispatchers().await
}

pub async fn apply_get_user(state: &AppState, id: Uuid) -> Result<DispatcherRecord, AppError> {
    state.db.get_dispatcher_by_id(id).await
}

/// Create a user. `role=owner` is rejected — the owner is established by
/// bootstrap or by ownership transfer, never by create.
pub async fn apply_create_user(
    state: &AppState,
    caller_effective: &[String],
    caller_role: Role,
    req: CreateUserRequest,
) -> Result<DispatcherRecord, AppError> {
    if req.role == Role::Owner {
        return Err(AppError::Forbidden(
            "cannot create a user with role=owner; ownership is established by bootstrap or transfer".into(),
        ));
    }

    if let Some(extra) = &req.extra_scopes {
        validate_grantable_scopes(caller_effective, caller_role, extra)?;
    }

    let email = normalize_email(&req.email);
    if state.db.get_dispatcher_by_email(&email).await?.is_some() {
        return Err(AppError::Conflict("email already in use".into()));
    }

    let now = Utc::now();
    let id = Uuid::new_v4();
    let record = DispatcherRecord {
        id,
        email,
        name: req.name,
        status: DispatcherStatus::Active,
        role: req.role,
        extra_scopes: req.extra_scopes.unwrap_or_default(),
        created_at: now,
        updated_at: now,
    };
    state.db.insert_dispatcher(&record).await?;

    let password_hash = hash_password(req.password).await?;
    let creds = DispatcherCredentials {
        dispatcher_id: id,
        password_hash,
        token_version: 0,
        failed_attempts: 0,
        locked_until: None,
        updated_at: now,
    };
    state.db.upsert_dispatcher_credentials(&creds).await?;

    Ok(record)
}

/// Update a user. Enforces owner-protection:
/// - demoting/deactivating the sole active owner is rejected;
/// - a non-owner caller cannot demote/deactivate the owner at all;
/// - setting a *different* user's role to `owner` is an ownership transfer,
///   permitted only when the caller is the current owner (promote target, then
///   demote the calling owner to fleet_manager).
pub async fn apply_update_user(
    state: &AppState,
    caller_effective: &[String],
    caller_id: Option<Uuid>,
    caller_role: Role,
    id: Uuid,
    req: UpdateUserRequest,
) -> Result<DispatcherRecord, AppError> {
    if let Some(extra) = &req.extra_scopes {
        validate_grantable_scopes(caller_effective, caller_role, extra)?;
    }

    let mut record = state.db.get_dispatcher_by_id(id).await?;
    let was_owner = record.role == Role::Owner;

    // Ownership transfer: a PATCH setting role=owner on a DIFFERENT user.
    if req.role == Some(Role::Owner) && !was_owner {
        if caller_role != Role::Owner {
            return Err(AppError::Forbidden(
                "only the current owner can transfer ownership".into(),
            ));
        }
        return apply_ownership_transfer(state, caller_id, id, req).await;
    }

    // Guard against demoting or deactivating the owner.
    if was_owner {
        let demoting = matches!(req.role, Some(r) if r != Role::Owner);
        let deactivating = req.status == Some(DispatcherStatus::Inactive);
        if demoting || deactivating {
            // Only an ownership transfer can change who the owner is; a plain
            // update never demotes/deactivates an owner — not even by an owner,
            // and certainly not by a fleet_manager.
            return Err(AppError::Forbidden(
                "the owner cannot be demoted or deactivated except via ownership transfer".into(),
            ));
        }
    }

    if let Some(name) = req.name {
        record.name = name;
    }
    if let Some(status) = req.status {
        record.status = status;
    }
    if let Some(role) = req.role {
        record.role = role;
    }
    if let Some(extra_scopes) = req.extra_scopes {
        record.extra_scopes = extra_scopes;
    }
    record.updated_at = Utc::now();
    state.db.upsert_dispatcher(&record).await?;
    Ok(record)
}

/// Promote `target_id` to owner, then demote the calling owner to fleet_manager.
/// Promote-then-demote so we never pass through a zero-owner state; a mid-failure
/// leaves a brief two-owner window (the safer failure mode — recoverable by
/// re-running the transfer), which we accept rather than risk zero owners.
async fn apply_ownership_transfer(
    state: &AppState,
    caller_id: Option<Uuid>,
    target_id: Uuid,
    req: UpdateUserRequest,
) -> Result<DispatcherRecord, AppError> {
    let caller_id = caller_id.ok_or_else(|| {
        AppError::Forbidden("ownership transfer requires an identified owner caller".into())
    })?;
    if caller_id == target_id {
        // Caller is already owner (guarded by was_owner check upstream); promoting
        // self is a no-op that would orphan ownership on demote. Reject.
        return Err(AppError::BadRequest(
            "cannot transfer ownership to yourself".into(),
        ));
    }

    let mut target = state.db.get_dispatcher_by_id(target_id).await?;
    let mut caller = state.db.get_dispatcher_by_id(caller_id).await?;
    if caller.role != Role::Owner {
        return Err(AppError::Forbidden(
            "only the current owner can transfer ownership".into(),
        ));
    }

    // Promote target to owner (also apply any other requested fields).
    if let Some(name) = req.name {
        target.name = name;
    }
    if let Some(status) = req.status {
        target.status = status;
    }
    if let Some(extra_scopes) = req.extra_scopes {
        target.extra_scopes = extra_scopes;
    }
    target.role = Role::Owner;
    target.updated_at = Utc::now();
    state.db.upsert_dispatcher(&target).await?;

    // Demote the prior owner to fleet_manager.
    caller.role = Role::FleetManager;
    caller.updated_at = Utc::now();
    state.db.upsert_dispatcher(&caller).await?;

    Ok(target)
}

/// Reset a user's password and bump token_version (invalidating outstanding JWTs).
pub async fn apply_reset_password(
    state: &AppState,
    caller_role: Role,
    id: Uuid,
    password: String,
) -> Result<(), AppError> {
    let target = state.db.get_dispatcher_by_id(id).await?;

    // Only the current owner may reset the owner's password — otherwise a
    // fleet_manager (who holds users:write) could take over the owner account.
    if target.role == Role::Owner && caller_role != Role::Owner {
        return Err(AppError::Forbidden(
            "only the current owner can reset the owner's password".into(),
        ));
    }

    let password_hash = hash_password(password).await?;
    let now = Utc::now();
    let new_token_version = match state.db.get_dispatcher_credentials(id).await? {
        Some(existing) => existing.token_version + 1,
        None => 0,
    };
    let creds = DispatcherCredentials {
        dispatcher_id: id,
        password_hash,
        token_version: new_token_version,
        failed_attempts: 0,
        locked_until: None,
        updated_at: now,
    };
    state.db.upsert_dispatcher_credentials(&creds).await?;
    Ok(())
}

/// Deactivate a user (status → Inactive + bump token_version). Soft delete,
/// mirroring the driver delete. The sole active owner cannot be deactivated.
pub async fn apply_delete_user(state: &AppState, id: Uuid) -> Result<(), AppError> {
    let mut record = state.db.get_dispatcher_by_id(id).await?;

    if record.role == Role::Owner {
        let users = state.db.list_dispatchers().await?;
        if count_active_owners(&users) <= 1 {
            return Err(AppError::Conflict(
                "cannot deactivate the only owner; transfer ownership first".into(),
            ));
        }
    }

    if record.status != DispatcherStatus::Inactive {
        record.status = DispatcherStatus::Inactive;
        record.updated_at = Utc::now();
        state.db.upsert_dispatcher(&record).await?;
    }

    // Invalidate outstanding JWTs by bumping the credential token_version.
    if let Some(mut creds) = state.db.get_dispatcher_credentials(id).await? {
        creds.token_version += 1;
        creds.updated_at = Utc::now();
        state.db.upsert_dispatcher_credentials(&creds).await?;
    }
    Ok(())
}

/// Resolve the caller's identity + current role from their claims, for
/// owner-protection/transfer checks. The caller's `dispatcher_id` (absent on an
/// API-key principal with no parseable id) is looked up to read `role` fresh
/// from the DB; failures fall back to the least-privileged `Dispatcher`.
pub async fn caller_identity(
    state: &AppState,
    claims: &DispatcherClaims,
) -> (Option<Uuid>, Role) {
    let Ok(id) = claims.dispatcher_id.parse::<Uuid>() else {
        return (None, Role::Dispatcher);
    };
    match state.db.get_dispatcher_by_id(id).await {
        Ok(record) => (Some(id), record.role),
        Err(_) => (Some(id), Role::Dispatcher),
    }
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/users",
    responses(
        (status = 200, description = "List of fleet users", body = UserListResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing users:read"),
    ),
    security(("BearerAuth" = [])),
    tag = "users"
)]
pub async fn list_users(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("users:read")?;
    let users = apply_list_users(&state).await?;
    let returned = users.len();
    Ok(Json(UserListResponse { users, returned }))
}

#[utoipa::path(
    get,
    path = "/dispatch/api/v1/users/{id}",
    params(("id" = Uuid, Path, description = "User UUID")),
    responses(
        (status = 200, description = "User record", body = DispatcherRecord),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing users:read"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "users"
)]
pub async fn get_user(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("users:read")?;
    let record = apply_get_user(&state, id).await?;
    Ok(Json(record))
}

#[utoipa::path(
    post,
    path = "/dispatch/api/v1/users",
    request_body(content = CreateUserRequest, description = "User to create"),
    responses(
        (status = 201, description = "Created user record", body = DispatcherRecord),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing users:write, or role=owner"),
        (status = 409, description = "Email already in use"),
    ),
    security(("BearerAuth" = [])),
    tag = "users"
)]
pub async fn create_user(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Json(body): Json<CreateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("users:write")?;
    let (_caller_id, caller_role) = caller_identity(&state, &claims).await;
    let record =
        apply_create_user(&state, &claims.effective_scopes, caller_role, body).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[utoipa::path(
    patch,
    path = "/dispatch/api/v1/users/{id}",
    params(("id" = Uuid, Path, description = "User UUID")),
    request_body(content = UpdateUserRequest, description = "Fields to update — all optional"),
    responses(
        (status = 200, description = "Updated user record", body = DispatcherRecord),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing users:write or owner-protection"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "users"
)]
pub async fn update_user(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("users:write")?;
    let (caller_id, caller_role) = caller_identity(&state, &claims).await;
    let record = apply_update_user(
        &state,
        &claims.effective_scopes,
        caller_id,
        caller_role,
        id,
        body,
    )
    .await?;
    Ok(Json(record))
}

#[utoipa::path(
    put,
    path = "/dispatch/api/v1/users/{id}/password",
    params(("id" = Uuid, Path, description = "User UUID")),
    request_body(content = ResetUserPasswordRequest, description = "New password"),
    responses(
        (status = 204, description = "Password reset; outstanding JWTs invalidated"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing users:write"),
        (status = 404, description = "Not found"),
    ),
    security(("BearerAuth" = [])),
    tag = "users"
)]
pub async fn reset_user_password(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
    Json(body): Json<ResetUserPasswordRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("users:write")?;
    let (_caller_id, caller_role) = caller_identity(&state, &claims).await;
    apply_reset_password(&state, caller_role, id, body.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/dispatch/api/v1/users/{id}",
    params(("id" = Uuid, Path, description = "User UUID")),
    responses(
        (status = 204, description = "Deactivated (status → inactive); outstanding JWTs invalidated"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing users:delete"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — cannot deactivate the only owner"),
    ),
    security(("BearerAuth" = [])),
    tag = "users"
)]
pub async fn delete_user(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("users:delete")?;
    apply_delete_user(&state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    use axum::routing::{get, put};
    Router::new()
        .route("/dispatch/api/v1/users", get(list_users).post(create_user))
        .route(
            "/dispatch/api/v1/users/{id}",
            get(get_user).patch(update_user).delete(delete_user),
        )
        .route(
            "/dispatch/api/v1/users/{id}/password",
            put(reset_user_password),
        )
}
