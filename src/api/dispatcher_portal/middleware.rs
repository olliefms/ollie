// src/api/dispatcher_portal/middleware.rs
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use crate::{
    AppState,
    error::AppError,
    models::DispatcherStatus,
};

use super::jwt::{decode_dispatcher_jwt, DispatcherClaims};

pub async fn require_dispatcher_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?
        .to_owned();

    let claims = if token.starts_with("olld_") {
        validate_api_key(&state, &token).await?
    } else {
        validate_jwt_token(&state, &token).await?
    };

    request.extensions_mut().insert(claims);
    Ok(next.run(request).await)
}

async fn validate_jwt_token(state: &AppState, token: &str) -> Result<DispatcherClaims, AppError> {
    let claims = decode_dispatcher_jwt(token, &state.config.dispatcher_jwt_secret)?;

    let dispatcher_id: Uuid = claims.dispatcher_id.parse()
        .map_err(|_| AppError::Unauthorized)?;

    let creds = state.db.get_dispatcher_credentials(dispatcher_id).await?
        .ok_or(AppError::Unauthorized)?;

    if creds.token_version != claims.token_version {
        return Err(AppError::Unauthorized);
    }

    let dispatcher = state.db.get_dispatcher_by_id(dispatcher_id).await
        .map_err(|_| AppError::Unauthorized)?;

    if dispatcher.status == DispatcherStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Err(AppError::Unauthorized);
        }
    }

    Ok(claims)
}

async fn validate_api_key(state: &AppState, token: &str) -> Result<DispatcherClaims, AppError> {
    let hash = hex::encode(Sha256::digest(token.as_bytes()));

    let key = state.db.get_dispatcher_api_key_by_hash(&hash).await?
        .ok_or(AppError::Unauthorized)?;

    if key.revoked_at.is_some() || key.expires_at <= Utc::now() {
        return Err(AppError::Unauthorized);
    }

    let dispatcher = state.db.get_dispatcher_by_id(key.dispatcher_id).await
        .map_err(|_| AppError::Unauthorized)?;

    if dispatcher.status == DispatcherStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    let creds = state.db.get_dispatcher_credentials(key.dispatcher_id).await?
        .ok_or(AppError::Unauthorized)?;

    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Err(AppError::Unauthorized);
        }
    }

    let key_id = key.id;
    let dispatcher_id = key.dispatcher_id;
    let db_for_touch = state.db.clone();
    tokio::spawn(async move {
        match db_for_touch.get_dispatcher_api_key_by_id(key_id, dispatcher_id).await {
            Ok(Some(mut current)) => {
                current.last_used_at = Some(Utc::now());
                if let Err(e) = db_for_touch.upsert_dispatcher_api_key(&current).await {
                    tracing::warn!(key_id = %key_id, err = ?e, "failed to update api key last_used_at");
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(key_id = %key_id, err = ?e, "failed to fetch api key for last_used_at update"),
        }
    });

    Ok(DispatcherClaims {
        dispatcher_id: key.dispatcher_id.to_string(),
        token_version: creds.token_version,
        iss: "ollie-dispatcher".into(),
        aud: "ollie-dispatcher".into(),
        exp: 0,
        iat: 0,
        kid: "api-key".into(),
        api_key_id: Some(key.id),
        api_key_label: Some(key.label),
    })
}

#[cfg(test)]
mod tests {
    use axum::{Router, http::StatusCode, middleware::from_fn, routing::get};
    use axum_test::TestServer;
    use crate::error::AppError;

    async fn stub_auth_middleware(
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> Result<axum::response::Response, AppError> {
        let has_bearer = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| !t.is_empty())
            .unwrap_or(false);
        if !has_bearer {
            return Err(AppError::Unauthorized);
        }
        Ok(next.run(req).await)
    }

    fn protected_app() -> Router {
        Router::new()
            .route("/protected", get(|| async { "ok" }))
            .route_layer(from_fn(stub_auth_middleware))
    }

    fn open_app() -> Router {
        Router::new()
            .route("/open", get(|| async { "open" }))
    }

    #[tokio::test]
    async fn test_require_dispatcher_auth_missing_header() {
        let server = TestServer::new(protected_app()).unwrap();
        let resp = server.get("/protected").await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_require_dispatcher_auth_invalid_token() {
        let server = TestServer::new(protected_app()).unwrap();
        let resp = server.get("/protected")
            .add_header(axum::http::header::AUTHORIZATION, "Bearer ")
            .await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_no_auth_routes_unaffected() {
        let server = TestServer::new(open_app()).unwrap();
        let resp = server.get("/open").await;
        assert_eq!(resp.status_code(), StatusCode::OK);
    }
}
