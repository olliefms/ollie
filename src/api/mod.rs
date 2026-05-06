// src/api/mod.rs
pub mod auth;
pub mod blob;
pub mod blobs;
pub mod facilities;
pub mod loads;

use crate::{api::auth::require_bearer, AppState};
use axum::{
    middleware::from_fn,
    routing::{delete, get, patch, post, put},
    Router,
};

pub fn router(state: AppState) -> Router {
    let key = state.config.admin_api_key.clone();
    Router::new()
        // Blobs
        .route("/api/v1/blobs", post(blobs::upload_blob))
        .route("/api/v1/blobs", get(blobs::list_blobs))
        .route("/api/v1/blob/:id", get(blob::get_blob))
        .route("/api/v1/blob/:id", put(blob::update_blob))
        .route("/api/v1/blob/:id", delete(blob::delete_blob))
        // Facilities
        .route("/api/v1/facilities", post(facilities::create_facility))
        .route("/api/v1/facilities", get(facilities::list_facilities))
        .route("/api/v1/facilities/:id", get(facilities::get_facility))
        .route("/api/v1/facilities/:id", patch(facilities::update_facility))
        .route("/api/v1/facilities/:id", delete(facilities::delete_facility))
        // Loads — CRUD
        .route("/api/v1/loads", post(loads::create_load))
        .route("/api/v1/loads", get(loads::list_loads))
        .route("/api/v1/loads/:id", get(loads::get_load))
        .route("/api/v1/loads/:id", patch(loads::update_load))
        .route("/api/v1/loads/:id", delete(loads::delete_load))
        // Loads — actions
        .route("/api/v1/loads/:id/assign", post(loads::assign_load))
        .route("/api/v1/loads/:id/dispatch", post(loads::dispatch_load))
        .route("/api/v1/loads/:id/in_transit", post(loads::in_transit_load))
        .route("/api/v1/loads/:id/deliver", post(loads::deliver_load))
        .route("/api/v1/loads/:id/invoice", post(loads::invoice_load))
        .route("/api/v1/loads/:id/cancel", post(loads::cancel_load))
        .route("/api/v1/loads/:id/settle", post(loads::settle_load))
        .layer(from_fn(move |req, next| {
            let k = key.clone();
            async move { require_bearer(k, req, next).await }
        }))
        .with_state(state)
}
