// src/api/oauth/mod.rs
//
// Portal-agnostic OAuth 2.1 Authorization Server for MCP connectors.
// One resource wired today: fleet_user (/fleet/mcp). Driver is future work.
pub mod authorize;
pub mod metadata;
pub mod register;
pub mod token;

use crate::AppState;
use axum::{routing::{get, post}, Router};

pub const FLEET_MCP_PATH: &str = "/fleet/mcp";

/// Absolute issuer/base URL from config (e.g. https://ollie.your-ollie-instance.example.com).
pub fn issuer(state: &AppState) -> String {
    state.config.public_base_url.trim_end_matches('/').to_string()
}

/// The protected-resource URL for the Fleet MCP endpoint.
pub fn dispatch_resource(state: &AppState) -> String {
    format!("{}{}", issuer(state), FLEET_MCP_PATH)
}

/// All OAuth routes — mounted PUBLIC (no fleet user middleware) by T10.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/oauth-authorization-server", get(metadata::authorization_server))
        .route("/.well-known/oauth-protected-resource", get(metadata::protected_resource))
        .route("/.well-known/oauth-protected-resource/fleet/mcp", get(metadata::protected_resource))
        .route("/oauth/register", post(register::register))
        .route("/oauth/authorize", get(authorize::authorize_page).post(authorize::authorize_decision))
        .route("/oauth/token", post(token::token))
}

/// OAuth error rendered per-spec. Token/DCR → JSON; authorize handles its own
/// (redirect vs error page) in `authorize.rs`.
pub enum OauthError {
    InvalidRequest(String),
    #[allow(dead_code)] // no endpoint uses client-credentials auth yet
    InvalidClient(String),
    InvalidGrant(String),
    UnsupportedGrantType,
    InvalidClientMetadata(String),
    ServerError(String),
}

impl axum::response::IntoResponse for OauthError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        let (status, code, desc) = match self {
            OauthError::InvalidRequest(d) => (StatusCode::BAD_REQUEST, "invalid_request", d),
            OauthError::InvalidClient(d) => (StatusCode::UNAUTHORIZED, "invalid_client", d),
            OauthError::InvalidGrant(d) => (StatusCode::BAD_REQUEST, "invalid_grant", d),
            OauthError::UnsupportedGrantType => (StatusCode::BAD_REQUEST, "unsupported_grant_type", String::new()),
            OauthError::InvalidClientMetadata(d) => (StatusCode::BAD_REQUEST, "invalid_client_metadata", d),
            OauthError::ServerError(d) => (StatusCode::INTERNAL_SERVER_ERROR, "server_error", d),
        };
        let body = axum::Json(serde_json::json!({ "error": code, "error_description": desc }));
        (status, body).into_response()
    }
}
