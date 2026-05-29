// src/api/driver_portal/auth.rs
use crate::{
    AppState,
    error::AppError,
    models::{DriverCredentials, DriverPasskeyCredential, DriverStatus},
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Instant;
use uuid::Uuid;
use webauthn_rs::prelude::Passkey;

use super::jwt::{decode_driver_jwt, encode_driver_jwt};
use crate::api::refresh_tokens;
use axum::http::header::SET_COOKIE;

// --- Phone normalization ---

pub fn normalize_phone(phone: &str) -> String {
    let stripped: String = phone.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '(' | ')'))
        .collect();
    if stripped.starts_with('+') {
        return stripped;
    }
    if stripped.len() == 10 && stripped.chars().all(|c| c.is_ascii_digit()) {
        return format!("+1{stripped}");
    }
    if stripped.chars().all(|c| c.is_ascii_digit()) {
        return format!("+{stripped}");
    }
    stripped
}

// --- Request/Response types ---

#[derive(Deserialize)]
pub struct ChallengeRequest {
    pub phone: String,
}

#[derive(Deserialize)]
pub struct VerifyRequest {
    pub driver_id: Uuid,
    pub response: Value,
}

#[derive(Deserialize)]
pub struct PinAuthRequest {
    pub phone: String,
    pub pin: String,
}

#[derive(Deserialize)]
pub struct RegisterPasskeyRequest {
    pub phase: String,
    pub response: Option<Value>,
}

// --- Helpers ---

// Static dummy bcrypt hash for timing equalization when no PIN is set.
// This is a valid bcrypt hash so bcrypt::verify runs to full completion,
// ensuring constant-time rejection regardless of whether PIN is set.
const DUMMY_HASH: &str = "$2b$12$LQv3c1yqBWVHxkd0LHAkCOYz6TtxMQJqhN8/lewPCVzPa.E6wuBK.";

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers.get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

fn default_credentials(driver_id: Uuid) -> DriverCredentials {
    DriverCredentials {
        driver_id,
        pin_hash: None,
        token_version: 0,
        failed_pin_attempts: 0,
        locked_until: None,
        updated_at: Utc::now(),
    }
}

// --- Handlers ---

pub async fn challenge(
    State(state): State<AppState>,
    Json(req): Json<ChallengeRequest>,
) -> Result<impl IntoResponse, AppError> {
    let phone = normalize_phone(&req.phone);
    let driver = state.db.get_driver_by_phone(&phone).await?
        .ok_or(AppError::NotFound)?;

    if driver.status == DriverStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    let passkey_records = state.db.get_passkey_credentials_for_driver(driver.id).await?;
    let passkeys: Vec<Passkey> = passkey_records.iter()
        .filter_map(|r| serde_json::from_str::<Passkey>(&r.public_key).ok())
        .collect();

    let (rcr, auth_state) = state.webauthn
        .start_passkey_authentication(&passkeys)
        .map_err(|e| AppError::Internal(format!("webauthn start auth error: {e}")))?;

    state.auth_challenge_store.insert(driver.id, (auth_state, Instant::now()));

    let challenge_value = serde_json::to_value(&rcr)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(json!({ "driver_id": driver.id, "challenge": challenge_value })))
}

pub async fn verify(
    State(state): State<AppState>,
    Json(req): Json<VerifyRequest>,
) -> Result<impl IntoResponse, AppError> {
    let driver = state.db.get_driver_by_id(req.driver_id).await?;
    if driver.status == DriverStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    // Fix 2: check locked_until before allowing passkey login
    let mut creds = match state.db.get_driver_credentials(req.driver_id).await? {
        Some(c) => c,
        None => default_credentials(req.driver_id),
    };
    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Err(AppError::Unauthorized);
        }
    }

    let (auth_state, _ts) = state.auth_challenge_store
        .remove(&req.driver_id)
        .map(|(_, v)| v)
        .ok_or(AppError::Unauthorized)?;

    let pub_key_cred = serde_json::from_value(req.response)
        .map_err(|e| AppError::BadRequest(format!("invalid credential response: {e}")))?;

    let auth_result = state.webauthn
        .finish_passkey_authentication(&pub_key_cred, &auth_state)
        .map_err(|_| AppError::Unauthorized)?;

    // Update counter — CredentialID is HumanBinaryData (Vec<u8>), encode as base64url for storage key
    let credential_id = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        auth_result.cred_id().as_slice(),
    );
    if let Some(mut passkey_cred) = state.db.get_passkey_credential(&credential_id).await? {
        passkey_cred.counter = auth_result.counter() as i64;
        state.db.upsert_passkey_credential(&passkey_cred).await?;
    }

    // Fix 2: reset lockout on successful passkey auth
    // Fix 4: always persist credentials row so the JWT middleware can find it
    creds.failed_pin_attempts = 0;
    creds.locked_until = None;
    creds.updated_at = Utc::now();
    state.db.upsert_driver_credentials(&creds).await?;

    let token = encode_driver_jwt(req.driver_id, creds.token_version, &state.config.driver_jwt_secret)?;

    tracing::info!(driver_id = %req.driver_id, "driver auth via passkey succeeded");

    let issued = refresh_tokens::issue(
        &state.db, "driver", req.driver_id, None, creds.token_version, Utc::now(),
    ).await?;
    let cookie = refresh_tokens::set_cookie_header(&issued.secret, state.config.cookie_secure);

    let mut response = Json(json!({ "token": token })).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        cookie.parse().map_err(|_| AppError::Internal("bad cookie".into()))?,
    );
    Ok(response)
}

pub async fn pin_auth(
    State(state): State<AppState>,
    Json(req): Json<PinAuthRequest>,
) -> Result<impl IntoResponse, AppError> {
    let phone = normalize_phone(&req.phone);
    let driver = state.db.get_driver_by_phone(&phone).await?
        .ok_or(AppError::Unauthorized)?;

    if driver.status == DriverStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    let mut creds = state.db.get_driver_credentials(driver.id).await?
        .ok_or(AppError::Unauthorized)?;

    // Check lockout
    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Ok((
                StatusCode::LOCKED,
                Json(json!({
                    "error": format!("locked until {}", locked_until.to_rfc3339()),
                    "locked_until": locked_until.to_rfc3339(),
                })),
            ).into_response());
        }
    }

    // Equalize timing: if no PIN is set, still run bcrypt against dummy hash
    let pin_hash = creds.pin_hash.clone().unwrap_or_else(|| DUMMY_HASH.to_string());
    let pin_valid = bcrypt::verify(&req.pin, &pin_hash)
        .map_err(|e| AppError::Internal(format!("bcrypt error: {e}")))?;

    // Ensure we fail if no PIN was actually set
    if creds.pin_hash.is_none() && pin_valid {
        // This shouldn't happen (dummy hash won't verify), but be explicit
        return Err(AppError::Unauthorized);
    }

    if !pin_valid {
        creds.failed_pin_attempts += 1;
        if creds.failed_pin_attempts >= 5 {
            let extra_failures = creds.failed_pin_attempts - 5;
            let backoff_mins = 15u64 * 2u64.pow(extra_failures as u32);
            let backoff_mins = backoff_mins.min(24 * 60);
            creds.locked_until = Some(Utc::now() + chrono::Duration::minutes(backoff_mins as i64));
            tracing::warn!(
                driver_id = %driver.id,
                failed_attempts = creds.failed_pin_attempts,
                locked_until = ?creds.locked_until,
                "driver PIN lockout"
            );
        }
        creds.updated_at = Utc::now();
        state.db.upsert_driver_credentials(&creds).await?;
        return Err(AppError::Unauthorized);
    }

    // Valid PIN
    creds.failed_pin_attempts = 0;
    creds.locked_until = None;
    creds.updated_at = Utc::now();
    state.db.upsert_driver_credentials(&creds).await?;

    let token = encode_driver_jwt(driver.id, creds.token_version, &state.config.driver_jwt_secret)?;

    tracing::info!(driver_id = %driver.id, "driver auth via PIN succeeded");

    let issued = refresh_tokens::issue(
        &state.db, "driver", driver.id, None, creds.token_version, Utc::now(),
    ).await?;
    let cookie = refresh_tokens::set_cookie_header(&issued.secret, state.config.cookie_secure);

    let mut response = Json(json!({ "token": token })).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        cookie.parse().map_err(|_| AppError::Internal("bad cookie".into()))?,
    );
    Ok(response)
}

pub async fn register_passkey(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterPasskeyRequest>,
) -> Result<impl IntoResponse, AppError> {
    let token = bearer_token(&headers).ok_or(AppError::Unauthorized)?;
    let claims = decode_driver_jwt(token, &state.config.driver_jwt_secret)?;
    let jwt_driver_id: Uuid = claims.driver_id.parse()
        .map_err(|_| AppError::Unauthorized)?;

    // Fix 1: validate token_version and driver status before either phase
    let reg_creds = state.db.get_driver_credentials(jwt_driver_id).await?
        .ok_or(AppError::Unauthorized)?;
    if reg_creds.token_version != claims.token_version {
        return Err(AppError::Unauthorized);
    }
    let reg_driver = state.db.get_driver_by_id(jwt_driver_id).await?;
    if reg_driver.status == DriverStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    match req.phase.as_str() {
        "start" => {
            // driver status and token_version already validated above

            let existing = state.db.get_passkey_credentials_for_driver(jwt_driver_id).await?;
            let existing_passkeys: Vec<Passkey> = existing.iter()
                .filter_map(|r| serde_json::from_str::<Passkey>(&r.public_key).ok())
                .collect();
            let exclude: Vec<webauthn_rs::prelude::CredentialID> = existing_passkeys.iter()
                .map(|p| p.cred_id().clone())
                .collect();
            let exclude_opt = if exclude.is_empty() { None } else { Some(exclude) };

            let phone_display = reg_driver.phone.as_deref().unwrap_or("unknown");
            let name_display = reg_driver.name.as_str();

            let (ccr, reg_state) = state.webauthn
                .start_passkey_registration(
                    jwt_driver_id,
                    phone_display,
                    name_display,
                    exclude_opt,
                )
                .map_err(|e| AppError::Internal(format!("webauthn start reg error: {e}")))?;

            state.reg_challenge_store.insert(jwt_driver_id, (reg_state, Instant::now()));

            let challenge_value = serde_json::to_value(&ccr)
                .map_err(|e| AppError::Internal(e.to_string()))?;

            Ok(Json(json!({ "challenge": challenge_value })).into_response())
        }
        "finish" => {
            let cred_value = req.response.ok_or_else(|| AppError::BadRequest("response is required for finish phase".into()))?;

            let (reg_state, _ts) = state.reg_challenge_store
                .remove(&jwt_driver_id)
                .map(|(_, v)| v)
                .ok_or(AppError::BadRequest("no pending registration for this driver".into()))?;

            let reg_pub_key = serde_json::from_value(cred_value)
                .map_err(|e| AppError::BadRequest(format!("invalid credential response: {e}")))?;

            let passkey = state.webauthn
                .finish_passkey_registration(&reg_pub_key, &reg_state)
                .map_err(|e| AppError::BadRequest(format!("webauthn finish reg error: {e}")))?;

            let credential_id = base64::Engine::encode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                passkey.cred_id().as_slice(),
            );
            let public_key = serde_json::to_string(&passkey)
                .map_err(|e| AppError::Internal(e.to_string()))?;

            let passkey_cred = DriverPasskeyCredential {
                credential_id,
                driver_id: jwt_driver_id,
                public_key,
                counter: 0,
                transports: "[]".into(),
                created_at: Utc::now(),
            };
            state.db.upsert_passkey_credential(&passkey_cred).await?;

            tracing::info!(driver_id = %jwt_driver_id, "driver passkey registered");

            Ok(Json(json!({ "ok": true })).into_response())
        }
        other => Err(AppError::BadRequest(format!("unknown phase: {other}"))),
    }
}

pub async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let secret = refresh_tokens::read_cookie(&headers).ok_or(AppError::Unauthorized)?;

    let hash = refresh_tokens::hash_token(&secret);
    let row = state.db.get_refresh_token_by_hash(&hash).await?
        .ok_or(AppError::Unauthorized)?;
    if row.subject_type != "driver" {
        return Err(AppError::Unauthorized);
    }
    let creds = state.db.get_driver_credentials(row.subject_id).await?
        .ok_or(AppError::Unauthorized)?;

    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Err(AppError::Unauthorized);
        }
    }

    match refresh_tokens::rotate(&state.db, &secret, creds.token_version, Utc::now()).await? {
        refresh_tokens::RotateResult::Rotated(next) => {
            let driver = state.db.get_driver_by_id(row.subject_id).await
                .map_err(|_| AppError::Unauthorized)?;
            if driver.status == DriverStatus::Inactive {
                return Err(AppError::Unauthorized);
            }
            let token = encode_driver_jwt(row.subject_id, creds.token_version, &state.config.driver_jwt_secret)?;
            let cookie = refresh_tokens::set_cookie_header(&next.secret, state.config.cookie_secure);
            let mut response = Json(json!({ "token": token })).into_response();
            response.headers_mut().insert(
                SET_COOKIE,
                cookie.parse().map_err(|_| AppError::Internal("bad cookie".into()))?,
            );
            Ok(response)
        }
        _ => Err(AppError::Unauthorized),
    }
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    if let Some(secret) = refresh_tokens::read_cookie(&headers) {
        let hash = refresh_tokens::hash_token(&secret);
        if let Some(row) = state.db.get_refresh_token_by_hash(&hash).await? {
            state.db.revoke_refresh_token_family(row.family_id, Utc::now()).await?;
        }
    }
    let cookie = refresh_tokens::clear_cookie_header(state.config.cookie_secure);
    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        cookie.parse().map_err(|_| AppError::Internal("bad cookie".into()))?,
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_phone_e164_unchanged() {
        assert_eq!(normalize_phone("+12125551234"), "+12125551234");
    }

    #[test]
    fn test_normalize_phone_10_digits() {
        assert_eq!(normalize_phone("2125551234"), "+12125551234");
    }

    #[test]
    fn test_normalize_phone_strips_formatting() {
        assert_eq!(normalize_phone("(212) 555-1234"), "+12125551234");
    }

    #[test]
    fn test_normalize_phone_strips_dashes() {
        assert_eq!(normalize_phone("212-555-1234"), "+12125551234");
    }

    #[test]
    fn test_normalize_phone_international_with_plus() {
        assert_eq!(normalize_phone("+442071838750"), "+442071838750");
    }

    #[test]
    fn test_normalize_phone_11_digits_numeric() {
        assert_eq!(normalize_phone("12125551234"), "+12125551234");
    }
}
