// src/api/oauth/register.rs
use crate::AppState;
use axum::{extract::State, http::StatusCode};

pub async fn register(State(_state): State<AppState>) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}
