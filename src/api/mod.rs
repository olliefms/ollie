// src/api/mod.rs
pub mod auth;
pub mod blob;
pub mod blobs;

use crate::{api::auth::require_bearer, AppState};
use axum::{
    middleware::from_fn,
    routing::{delete, get, post, put},
    Router,
};

pub fn router(state: AppState) -> Router {
    let key = state.config.admin_api_key.clone();
    Router::new()
        .route("/api/v1/blobs", post(blobs::upload_blob))
        .route("/api/v1/blobs", get(blobs::list_blobs))
        .route("/api/v1/blob/:id", get(blob::get_blob))
        .route("/api/v1/blob/:id", put(blob::update_blob))
        .route("/api/v1/blob/:id", delete(blob::delete_blob))
        .layer(from_fn(move |req, next| {
            let k = key.clone();
            async move { require_bearer(k, req, next).await }
        }))
        .with_state(state)
}
