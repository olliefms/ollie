// src/api/dispatcher_portal/mod.rs
pub mod auth;
pub mod jwt;
pub mod middleware;

use crate::AppState;
use axum::{Router, routing::post};

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/dispatch/auth/login", post(auth::login))
        .route("/dispatch/auth/refresh", post(auth::refresh))
}

pub fn dispatcher_portal_router(_state: &AppState) -> Router<AppState> {
    // The dispatch API and MCP routes are added in #91 and #93.
    // For now, just export the auth router.
    auth_router()
}
