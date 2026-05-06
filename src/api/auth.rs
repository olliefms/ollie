// src/api/auth.rs
use crate::error::AppError;
use axum::{extract::Request, http::header::AUTHORIZATION, middleware::Next, response::Response};

pub async fn require_bearer(
    expected_key: String,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) if t == expected_key => Ok(next.run(request).await),
        _ => Err(AppError::Unauthorized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, middleware::from_fn, routing::get, Router};
    use axum_test::TestServer;

    fn test_app(key: &'static str) -> Router {
        let key = key.to_string();
        Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(from_fn(move |req, next| {
                let k = key.clone();
                async move { require_bearer(k, req, next).await }
            }))
    }

    #[tokio::test]
    async fn test_valid_bearer_passes() {
        let server = TestServer::new(test_app("secret")).unwrap();
        let resp = server.get("/test")
            .add_header(axum::http::header::AUTHORIZATION, "Bearer secret")
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_missing_bearer_returns_401() {
        let server = TestServer::new(test_app("secret")).unwrap();
        let resp = server.get("/test").await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_wrong_key_returns_401() {
        let server = TestServer::new(test_app("secret")).unwrap();
        let resp = server.get("/test")
            .add_header(axum::http::header::AUTHORIZATION, "Bearer wrong")
            .await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }
}
