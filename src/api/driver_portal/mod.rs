// src/api/driver_portal/mod.rs
pub mod auth;
pub mod jwt;
pub mod middleware;

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

pub fn portal_router(state: &AppState) -> Router<AppState> {
    // data sub-router: empty until #51 adds routes (GET /me, /trips, etc.).
    // The JWT middleware is wired here so #51 only needs to add .route() calls
    // to data_router(); the protection is already in place.
    //
    // NOTE: Axum panics if route_layer is applied to a router with no routes.
    // Once #51 adds the first route, uncomment the route_layer block below.
    //
    // let data = Router::new()
    //     /* #51 routes */
    //     .route_layer(axum::middleware::from_fn_with_state(
    //         state.clone(),
    //         middleware::require_driver_jwt,
    //     ));
    let _ = state; // state used once #51 wires the data sub-router

    Router::new()
        .nest("/driver/api/v1", auth_router())
}
