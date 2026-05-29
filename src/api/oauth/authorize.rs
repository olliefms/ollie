// src/api/oauth/authorize.rs
use crate::AppState;
use axum::{extract::State, response::Response, response::IntoResponse, http::StatusCode};

pub async fn authorize_page(State(_state): State<AppState>) -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

pub async fn authorize_decision(State(_state): State<AppState>) -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}
