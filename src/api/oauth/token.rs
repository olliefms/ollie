// src/api/oauth/token.rs
use crate::{
    api::dispatcher_portal::jwt::encode_dispatcher_jwt,
    api::refresh_tokens,
    AppState,
};
use axum::{extract::State, Json, Form};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use super::OauthError;

#[derive(Deserialize)]
pub struct TokenForm {
    pub grant_type: String,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
    pub code_verifier: Option<String>,
    pub refresh_token: Option<String>,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub refresh_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

const ACCESS_TTL_SECS: i64 = 8 * 3600;

/// PKCE S256: base64url(SHA256(verifier)) == challenge.
fn pkce_ok(verifier: &str, challenge: &str) -> bool {
    use base64::Engine;
    let digest = Sha256::digest(verifier.as_bytes());
    let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    computed == challenge
}

pub async fn token(
    State(state): State<AppState>,
    Form(f): Form<TokenForm>,
) -> Result<Json<TokenResponse>, OauthError> {
    match f.grant_type.as_str() {
        "authorization_code" => auth_code_grant(&state, f).await,
        "refresh_token" => refresh_grant(&state, f).await,
        _ => Err(OauthError::UnsupportedGrantType),
    }
}

async fn auth_code_grant(state: &AppState, f: TokenForm) -> Result<Json<TokenResponse>, OauthError> {
    let code = f.code.ok_or_else(|| OauthError::InvalidRequest("code required".into()))?;
    let redirect_uri = f.redirect_uri.ok_or_else(|| OauthError::InvalidRequest("redirect_uri required".into()))?;
    let client_id = f.client_id.ok_or_else(|| OauthError::InvalidRequest("client_id required".into()))?;
    let verifier = f.code_verifier.ok_or_else(|| OauthError::InvalidRequest("code_verifier required".into()))?;

    let code_hash = hex::encode(Sha256::digest(code.as_bytes()));
    let record = state.db.consume_authorization_code(&code_hash, Utc::now()).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
        .ok_or_else(|| OauthError::InvalidGrant("code invalid, expired, or used".into()))?;

    if record.client_id.to_string() != client_id {
        return Err(OauthError::InvalidGrant("client_id mismatch".into()));
    }
    if record.redirect_uri != redirect_uri {
        return Err(OauthError::InvalidGrant("redirect_uri mismatch".into()));
    }
    if !pkce_ok(&verifier, &record.code_challenge) {
        return Err(OauthError::InvalidGrant("PKCE verification failed".into()));
    }

    let creds = state.db.get_dispatcher_credentials(record.subject_id).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
        .ok_or_else(|| OauthError::InvalidGrant("unknown subject".into()))?;

    let access = encode_dispatcher_jwt(record.subject_id, creds.token_version, &state.config.dispatcher_jwt_secret)
        .map_err(|e| OauthError::ServerError(e.to_string()))?;
    let issued = refresh_tokens::issue(
        &state.db, "dispatcher", record.subject_id, Some(record.client_id), creds.token_version, Utc::now(),
    ).await.map_err(|e| OauthError::ServerError(e.to_string()))?;

    Ok(Json(TokenResponse {
        access_token: access,
        token_type: "Bearer",
        expires_in: ACCESS_TTL_SECS,
        refresh_token: issued.secret,
        scope: record.scope,
    }))
}

async fn refresh_grant(state: &AppState, f: TokenForm) -> Result<Json<TokenResponse>, OauthError> {
    let secret = f.refresh_token.ok_or_else(|| OauthError::InvalidRequest("refresh_token required".into()))?;
    let hash = refresh_tokens::hash_token(&secret);
    let row = state.db.get_refresh_token_by_hash(&hash).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
        .ok_or_else(|| OauthError::InvalidGrant("unknown refresh_token".into()))?;

    // This endpoint serves OAuth clients only: the token must be bound to a
    // registered client, and the request's client_id must match it (OAuth 2.1
    // §4.1.3 / RFC 9700 — enforced even for public clients). PWA session tokens
    // (client_id = None) are rotated via /fleet/auth/refresh, not here.
    let row_client_id = row.client_id
        .ok_or_else(|| OauthError::InvalidGrant("refresh_token not issued to an OAuth client".into()))?;
    let req_client_id = f.client_id.as_deref()
        .ok_or_else(|| OauthError::InvalidRequest("client_id required".into()))?;
    if req_client_id != row_client_id.to_string() {
        return Err(OauthError::InvalidGrant("client_id mismatch".into()));
    }

    if row.subject_type != "dispatcher" {
        return Err(OauthError::InvalidGrant("unsupported subject".into()));
    }
    let creds = state.db.get_dispatcher_credentials(row.subject_id).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
        .ok_or_else(|| OauthError::InvalidGrant("unknown subject".into()))?;

    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Err(OauthError::InvalidGrant("account locked".into()));
        }
    }
    let dispatcher = state.db.get_dispatcher_by_id(row.subject_id).await
        .map_err(|_| OauthError::InvalidGrant("unknown subject".into()))?;
    if dispatcher.status == crate::models::DispatcherStatus::Inactive {
        return Err(OauthError::InvalidGrant("account inactive".into()));
    }

    match refresh_tokens::rotate(&state.db, &secret, creds.token_version, Utc::now()).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
    {
        refresh_tokens::RotateResult::Rotated(next) => {
            let access = encode_dispatcher_jwt(row.subject_id, creds.token_version, &state.config.dispatcher_jwt_secret)
                .map_err(|e| OauthError::ServerError(e.to_string()))?;
            Ok(Json(TokenResponse {
                access_token: access,
                token_type: "Bearer",
                expires_in: ACCESS_TTL_SECS,
                refresh_token: next.secret,
                scope: None,
            }))
        }
        refresh_tokens::RotateResult::ReusedFamilyRevoked => {
            tracing::warn!(subject_id = %row.subject_id, "oauth refresh-token reuse detected; family revoked");
            Err(OauthError::InvalidGrant("refresh_token invalid or reused".into()))
        }
        refresh_tokens::RotateResult::Invalid => {
            Err(OauthError::InvalidGrant("refresh_token invalid or reused".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_pkce_known_vector() {
        // RFC 7636 Appendix B vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(pkce_ok(verifier, challenge));
        assert!(!pkce_ok("wrong", challenge));
    }
}
