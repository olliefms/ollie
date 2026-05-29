// src/api/oauth/token.rs
use crate::AppState;
use axum::{extract::State, http::StatusCode};

pub async fn token(State(_state): State<AppState>) -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}
