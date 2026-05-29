// src/api/oauth/metadata.rs
use crate::AppState;
use axum::{extract::State, Json};

pub async fn protected_resource(State(_state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({}))
}

pub async fn authorization_server(State(_state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({}))
}
