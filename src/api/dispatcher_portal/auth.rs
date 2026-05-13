// src/api/dispatcher_portal/auth.rs
use crate::{
    AppState,
    error::AppError,
    models::DispatcherStatus,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use utoipa::ToSchema;
use uuid::Uuid;

use super::jwt::{decode_dispatcher_jwt, encode_dispatcher_jwt};

// Pre-computed dummy hash used to equalise response time for unknown-email logins.
// Computed once at cost 12 (same as real passwords) so the timing profile matches.
static DUMMY_HASH: OnceLock<String> = OnceLock::new();
fn dummy_hash() -> &'static str {
    DUMMY_HASH.get_or_init(|| bcrypt::hash("dummy-sentinel", 12).expect("bcrypt init failed"))
}

// --- Request/Response types ---

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    pub token: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LockResponse {
    pub error: String,
    pub locked_until: String,
}

// --- Helpers ---

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers.get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

// --- Handlers ---

/// Login as a dispatcher using email and password. Returns a JWT on success.
#[utoipa::path(
    post,
    path = "/dispatch/auth/login",
    request_body(content = LoginRequest, description = "Dispatcher credentials"),
    responses(
        (status = 200, description = "JWT token", body = LoginResponse),
        (status = 401, description = "Invalid credentials or account inactive"),
        (status = 423, description = "Account locked due to too many failed attempts", body = LockResponse),
    ),
    tag = "dispatch-auth"
)]
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse, AppError> {
    let email = normalize_email(&req.email);

    let dispatcher_opt = state.db.get_dispatcher_by_email(&email).await?;
    let dispatcher = match dispatcher_opt {
        Some(d) => d,
        None => {
            // Run bcrypt on a dummy hash to equalise timing for unknown vs wrong-password (#107).
            let pwd = req.password.clone();
            let _ = tokio::task::spawn_blocking(move || bcrypt::verify(&pwd, dummy_hash())).await;
            return Err(AppError::Unauthorized);
        }
    };

    if dispatcher.status == DispatcherStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    let mut creds = state.db.get_dispatcher_credentials(dispatcher.id).await?
        .ok_or(AppError::Unauthorized)?;

    // Check lockout
    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Ok((
                StatusCode::LOCKED,
                Json(LockResponse {
                    error: "account locked".into(),
                    locked_until: locked_until.to_rfc3339(),
                }),
            ).into_response());
        }
    }

    // Always run bcrypt to avoid timing oracle
    let password = req.password.clone();
    let hash = creds.password_hash.clone();
    let password_valid = tokio::task::spawn_blocking(move || bcrypt::verify(&password, &hash))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(|e| AppError::Internal(format!("bcrypt error: {e}")))?;

    if !password_valid {
        creds.failed_attempts += 1;
        if creds.failed_attempts >= 5 {
            let extra_failures = creds.failed_attempts - 5;
            let backoff_mins = 15u64 * 2u64.pow(extra_failures as u32);
            let backoff_mins = backoff_mins.min(24 * 60);
            creds.locked_until = Some(Utc::now() + chrono::Duration::minutes(backoff_mins as i64));
            tracing::warn!(
                dispatcher_id = %dispatcher.id,
                failed_attempts = creds.failed_attempts,
                locked_until = ?creds.locked_until,
                "dispatcher login lockout"
            );
        }
        creds.updated_at = Utc::now();
        state.db.upsert_dispatcher_credentials(&creds).await?;
        return Err(AppError::Unauthorized);
    }

    // Valid password — reset lockout state
    creds.failed_attempts = 0;
    creds.locked_until = None;
    creds.updated_at = Utc::now();
    state.db.upsert_dispatcher_credentials(&creds).await?;

    let token = encode_dispatcher_jwt(dispatcher.id, creds.token_version, &state.config.dispatcher_jwt_secret)?;

    tracing::info!(dispatcher_id = %dispatcher.id, "dispatcher login succeeded");

    Ok((StatusCode::OK, Json(LoginResponse { token })).into_response())
}

/// Refresh a dispatcher JWT. The existing token must be valid and issued within the last 7 days.
#[utoipa::path(
    post,
    path = "/dispatch/auth/refresh",
    responses(
        (status = 200, description = "New JWT token", body = LoginResponse),
        (status = 401, description = "Invalid, expired, or too-old token"),
    ),
    tag = "dispatch-auth"
)]
pub async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let token = bearer_token(&headers).ok_or(AppError::Unauthorized)?;
    let claims = decode_dispatcher_jwt(token, &state.config.dispatcher_jwt_secret)?;

    // Check iat: refresh window is 7 days
    let now = jsonwebtoken::get_current_timestamp() as usize;
    let max_refresh_secs = 7 * 24 * 3600;
    if now.saturating_sub(claims.iat) > max_refresh_secs {
        return Err(AppError::Unauthorized);
    }

    let dispatcher_id: Uuid = claims.dispatcher_id.parse()
        .map_err(|_| AppError::Unauthorized)?;

    let creds = state.db.get_dispatcher_credentials(dispatcher_id).await?
        .ok_or(AppError::Unauthorized)?;

    if creds.token_version != claims.token_version {
        return Err(AppError::Unauthorized);
    }

    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Err(AppError::Unauthorized);
        }
    }

    let dispatcher = state.db.get_dispatcher_by_id(dispatcher_id).await
        .map_err(|_| AppError::Unauthorized)?;
    if dispatcher.status == DispatcherStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    let new_token = encode_dispatcher_jwt(dispatcher_id, creds.token_version, &state.config.dispatcher_jwt_secret)?;

    Ok(Json(LoginResponse { token: new_token }))
}
