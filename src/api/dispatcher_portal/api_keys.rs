// src/api/dispatcher_portal/api_keys.rs
use axum::{
    Extension,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use base64::Engine;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    AppState,
    api::dispatcher_portal::jwt::DispatcherClaims,
    error::AppError,
    models::DispatcherApiKey,
};

pub fn generate_api_key() -> String {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(a.as_bytes());
    bytes[16..].copy_from_slice(b.as_bytes());
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!("olld_{encoded}")
}

pub fn hash_api_key(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateApiKeyRequest {
    pub label: String,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateApiKeyResponse {
    pub id: Uuid,
    pub label: String,
    pub key: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiKeyListItem {
    pub id: Uuid,
    pub label: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
    pub last_used_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiKeyListResponse {
    pub keys: Vec<ApiKeyListItem>,
}

#[utoipa::path(
    post,
    path = "/dispatch/api-keys",
    request_body(content = CreateApiKeyRequest, description = "API key creation request"),
    responses(
        (status = 201, description = "API key created (plaintext returned once)", body = CreateApiKeyResponse),
        (status = 400, description = "Invalid label or expires_in_days"),
        (status = 401, description = "Unauthorized (JWT required, API-key auth not allowed)"),
        (status = 429, description = "Dispatcher already has 20 active keys"),
    ),
    tag = "dispatch-api-keys"
)]
pub async fn create_api_key(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Json(req): Json<CreateApiKeyRequest>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("api_keys:write")?;
    if claims.api_key_id.is_some() {
        return Err(AppError::Unauthorized);
    }

    let label = req.label.trim().to_string();
    if label.is_empty() || label.len() > 64 {
        return Err(AppError::BadRequest("label must be 1-64 characters".into()));
    }
    let expires_in_days = req.expires_in_days.unwrap_or(365);
    if !(1..=365).contains(&expires_in_days) {
        return Err(AppError::BadRequest("expires_in_days must be between 1 and 365".into()));
    }

    let dispatcher_id: Uuid = claims.dispatcher_id.parse().map_err(|_| AppError::Unauthorized)?;

    let active = state.db.count_active_dispatcher_api_keys(dispatcher_id).await?;
    if active >= 20 {
        return Err(AppError::TooManyRequests);
    }

    let now = Utc::now();
    let plaintext = generate_api_key();
    let key_hash = hash_api_key(&plaintext);
    let key_prefix = plaintext[..12].to_string();

    let record = DispatcherApiKey {
        id: Uuid::new_v4(),
        dispatcher_id,
        label: label.clone(),
        key_hash,
        key_prefix: key_prefix.clone(),
        created_at: now,
        expires_at: now + chrono::Duration::days(expires_in_days as i64),
        revoked_at: None,
        last_used_at: None,
    };
    state.db.insert_dispatcher_api_key(&record).await?;

    Ok((StatusCode::CREATED, Json(CreateApiKeyResponse {
        id: record.id,
        label,
        key: plaintext,
        key_prefix,
        created_at: record.created_at,
        expires_at: record.expires_at,
    })))
}

#[utoipa::path(
    get,
    path = "/dispatch/api-keys",
    responses(
        (status = 200, description = "List active API keys for the calling dispatcher", body = ApiKeyListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    tag = "dispatch-api-keys"
)]
pub async fn list_api_keys(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("api_keys:read")?;
    let dispatcher_id: Uuid = claims.dispatcher_id.parse().map_err(|_| AppError::Unauthorized)?;
    let keys = state.db.list_active_dispatcher_api_keys(dispatcher_id).await?;
    let items = keys.into_iter().map(|k| ApiKeyListItem {
        id: k.id,
        label: k.label,
        key_prefix: k.key_prefix,
        created_at: k.created_at,
        expires_at: k.expires_at,
        last_used_at: k.last_used_at,
    }).collect();
    Ok(Json(ApiKeyListResponse { keys: items }))
}

#[utoipa::path(
    delete,
    path = "/dispatch/api-keys/{id}",
    params(("id" = Uuid, Path, description = "API key id")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, description = "Unauthorized (JWT required)"),
        (status = 404, description = "Not found or owned by another dispatcher"),
    ),
    tag = "dispatch-api-keys"
)]
pub async fn revoke_api_key(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(key_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    claims.require_scope("api_keys:delete")?;
    if claims.api_key_id.is_some() {
        return Err(AppError::Unauthorized);
    }

    let dispatcher_id: Uuid = claims.dispatcher_id.parse().map_err(|_| AppError::Unauthorized)?;

    let key = state.db.get_dispatcher_api_key_by_id(key_id, dispatcher_id).await?
        .ok_or(AppError::NotFound)?;

    if key.revoked_at.is_some() {
        return Err(AppError::NotFound);
    }

    let mut updated = key;
    updated.revoked_at = Some(Utc::now());
    state.db.upsert_dispatcher_api_key(&updated).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_generate_api_key_format() {
        let key = generate_api_key();
        assert!(key.starts_with("olld_"), "key must start with olld_: {key}");
        assert_eq!(key.len(), 48, "key must be 48 chars: {key}");
    }

    #[test]
    fn test_generate_api_key_prefix_is_first_12_chars() {
        let key = generate_api_key();
        let prefix = &key[..12];
        assert!(prefix.starts_with("olld_"));
        assert_eq!(prefix.len(), 12);
    }

    #[test]
    fn test_generate_api_key_unique() {
        let keys: HashSet<String> = (0..20).map(|_| generate_api_key()).collect();
        assert_eq!(keys.len(), 20, "all 20 generated keys must be unique");
    }

    #[test]
    fn test_hash_api_key_is_hex_sha256() {
        let hash = hash_api_key("olld_testkey");
        assert_eq!(hash.len(), 64, "SHA-256 hex must be 64 chars");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_api_key_stable() {
        let h1 = hash_api_key("olld_testkey");
        let h2 = hash_api_key("olld_testkey");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_api_key_different_inputs() {
        assert_ne!(hash_api_key("olld_aaa"), hash_api_key("olld_bbb"));
    }
}
