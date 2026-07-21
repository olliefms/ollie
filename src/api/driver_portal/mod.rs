// src/api/driver_portal/mod.rs
pub mod auth;
pub mod data;
pub mod documents;
pub mod equipment;
pub mod expenses;
pub mod jwt;
pub mod middleware;

use crate::AppState;
use axum::{Router, routing::{delete as axum_delete, get, post, put}};

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/auth/challenge", post(auth::challenge))
        .route("/auth/verify", post(auth::verify))
        .route("/auth/pin", post(auth::pin_auth))
        .route("/auth/register-passkey", post(auth::register_passkey))
        .route("/auth/refresh", post(auth::refresh))
        .route("/auth/logout", post(auth::logout))
}

pub fn portal_router(state: &AppState) -> Router<AppState> {
    let data = Router::new()
        .route("/me", get(data::me))
        .route("/trips", get(data::list_trips))
        .route("/trips/{id}", get(data::trip_detail))
        .route("/trips/{id}/stops/{seq}", get(data::stop_detail).patch(data::update_stop_times))
        .route(
            "/trips/{id}/documents",
            post(documents::upload_document)
                .layer(axum::extract::DefaultBodyLimit::max(50 * 1024 * 1024))
                .get(documents::list_documents),
        )
        .route(
            "/trips/{id}/documents/{blob_id}",
            axum_delete(documents::delete_document),
        )
        .route(
            "/trips/{id}/documents/{blob_id}/content",
            get(documents::get_document_content),
        )
        .route("/equipment", get(equipment::get_equipment))
        .route("/equipment/trailer", put(equipment::update_trailer))
        .route("/trailers", get(equipment::list_available_trailers))
        .route("/expenses", get(expenses::list_expenses))
        .route("/expenses/{id}", axum_delete(expenses::delete_expense))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_driver_jwt,
        ));

    Router::new()
        .nest("/driver/api/v1", auth_router())
        .nest("/driver/api/v1", data)
}
