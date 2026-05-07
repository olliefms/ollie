// src/api/driver_portal/mod.rs
pub mod auth;
pub mod jwt;

use crate::AppState;
use axum::{Router, routing::post};

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/auth/challenge", post(auth::challenge))
        .route("/auth/verify", post(auth::verify))
        .route("/auth/pin", post(auth::pin_auth))
        .route("/auth/register-passkey", post(auth::register_passkey))
        .route("/auth/refresh", post(auth::refresh))
}
