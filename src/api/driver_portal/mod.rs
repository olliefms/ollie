// src/api/driver_portal/mod.rs
pub mod auth;
pub mod data;
pub mod jwt;
pub mod middleware;

use crate::AppState;
use axum::{Router, routing::{get, post}};

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/auth/challenge", post(auth::challenge))
        .route("/auth/verify", post(auth::verify))
        .route("/auth/pin", post(auth::pin_auth))
        .route("/auth/register-passkey", post(auth::register_passkey))
        .route("/auth/refresh", post(auth::refresh))
}

pub fn portal_router(state: &AppState) -> Router<AppState> {
    let data = Router::new()
        .route("/me", get(data::me))
        .route("/trips", get(data::list_trips))
        .route("/trips/:id", get(data::trip_detail))
        .route("/trips/:id/stops/:seq", get(data::stop_detail))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_driver_jwt,
        ));

    Router::new()
        .nest("/driver/api/v1", auth_router())
        .nest("/driver/api/v1", data)
}
