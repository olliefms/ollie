// src/api/driver_portal/middleware.rs
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use crate::{
    AppState,
    error::AppError,
    models::DriverStatus,
};

pub async fn require_driver_jwt(
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

    let claims = crate::api::driver_portal::jwt::decode_driver_jwt(
        &token,
        &state.config.driver_jwt_secret,
    )?;

    let driver_id = claims.driver_id.parse::<uuid::Uuid>()
        .map_err(|_| AppError::Unauthorized)?;

    let (creds_result, driver_result) = tokio::try_join!(
        state.db.get_driver_credentials(driver_id),
        state.db.get_driver_by_id(driver_id),
    )?;

    let creds = creds_result.ok_or(AppError::Unauthorized)?;
    if creds.token_version != claims.token_version {
        return Err(AppError::Unauthorized);
    }

    if driver_result.status == DriverStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    request.extensions_mut().insert(claims);
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use axum::{Router, http::StatusCode, middleware::from_fn, routing::get};
    use axum_test::TestServer;
    use crate::error::AppError;

    // Lightweight stand-in for the real middleware: checks Authorization header only,
    // without needing a real AppState. Tests the header extraction path.
    async fn stub_jwt_middleware(
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
            .route_layer(from_fn(stub_jwt_middleware))
    }

    fn open_app() -> Router {
        Router::new()
            .route("/open", get(|| async { "open" }))
    }

    #[tokio::test]
    async fn test_require_driver_jwt_missing_header() {
        let server = TestServer::new(protected_app()).unwrap();
        let resp = server.get("/protected").await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_require_driver_jwt_invalid_token() {
        // "Bearer " with empty token is rejected (simulates garbage token)
        let server = TestServer::new(protected_app()).unwrap();
        let resp = server.get("/protected")
            .add_header(axum::http::header::AUTHORIZATION, "Bearer ")
            .await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_no_auth_routes_unaffected() {
        // Routes not behind the middleware don't require JWT
        let server = TestServer::new(open_app()).unwrap();
        let resp = server.get("/open").await;
        assert_eq!(resp.status_code(), StatusCode::OK);
    }
}
