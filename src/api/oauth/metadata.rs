// src/api/oauth/metadata.rs
use crate::AppState;
use axum::{extract::State, Json};
use serde_json::json;

/// RFC 9728 Protected Resource Metadata.
pub async fn protected_resource(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "resource": super::dispatch_resource(&state),
        "authorization_servers": [super::issuer(&state)],
    }))
}

/// RFC 8414 Authorization Server Metadata.
pub async fn authorization_server(State(state): State<AppState>) -> Json<serde_json::Value> {
    let iss = super::issuer(&state);
    Json(json!({
        "issuer": iss,
        "authorization_endpoint": format!("{iss}/oauth/authorize"),
        "token_endpoint": format!("{iss}/oauth/token"),
        "registration_endpoint": format!("{iss}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
    }))
}
