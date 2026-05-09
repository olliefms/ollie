// src/api/dispatcher_portal/mod.rs
pub mod auth;
pub mod data;
pub mod jwt;
pub mod middleware;

use crate::AppState;
use axum::{Router, routing::{get, post}};

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/dispatch/auth/login", post(auth::login))
        .route("/dispatch/auth/refresh", post(auth::refresh))
}

pub fn data_router(state: &AppState) -> Router<AppState> {
    Router::new()
        .route("/dispatch/api/v1/loads", get(data::list_loads).post(data::create_load))
        .route("/dispatch/api/v1/loads/:id", get(data::get_load).put(data::update_load))
        .route("/dispatch/api/v1/trips", get(data::list_trips))
        .route("/dispatch/api/v1/trips/:id", get(data::get_trip))
        .route("/dispatch/api/v1/trips/:id/assign", post(data::assign_trip))
        .route("/dispatch/api/v1/trips/:id/unassign", post(data::unassign_trip))
        .route("/dispatch/api/v1/drivers", get(data::list_drivers))
        .route("/dispatch/api/v1/drivers/:id", get(data::get_driver))
        .route("/dispatch/api/v1/trucks", get(data::list_trucks))
        .route("/dispatch/api/v1/trucks/:id", get(data::get_truck))
        .route("/dispatch/api/v1/trailers", get(data::list_trailers))
        .route("/dispatch/api/v1/trailers/:id", get(data::get_trailer))
        .route("/dispatch/api/v1/events", get(data::list_events))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_dispatcher_jwt,
        ))
}

pub fn dispatcher_portal_router(state: &AppState) -> Router<AppState> {
    auth_router().merge(data_router(state))
}
