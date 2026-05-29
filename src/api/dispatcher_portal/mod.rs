// src/api/dispatcher_portal/mod.rs
pub mod api_keys;
pub mod auth;
pub mod blob_links;
pub mod blobs;
pub mod data;
pub mod driver_writes;
pub mod facility_writes;
pub mod jwt;
pub mod mcp;
pub mod middleware;
pub mod trailer_writes;
pub mod trip_writes;
pub mod truck_writes;

use crate::AppState;
use axum::{
    extract::DefaultBodyLimit,
    Router,
    routing::{delete, get, post},
};

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/dispatch/auth/login", post(auth::login))
        .route("/dispatch/auth/refresh", post(auth::refresh))
        .route("/dispatch/auth/logout", post(auth::logout))
}

pub fn data_router(state: &AppState) -> Router<AppState> {
    Router::new()
        .route("/dispatch/api/v1/loads", get(data::list_loads).post(data::create_load))
        .route("/dispatch/api/v1/loads/:id", get(data::get_load).put(data::update_load))
        .route("/dispatch/api/v1/trips", get(data::list_trips))
        .route(
            "/dispatch/api/v1/trips/:id",
            get(data::get_trip).patch(trip_writes::patch_trip_handler),
        )
        .route(
            "/dispatch/api/v1/trips/:id/recalculate-miles",
            post(trip_writes::recalculate_miles_handler),
        )
        .route("/dispatch/api/v1/trips/:id/assign", post(data::assign_trip))
        .route("/dispatch/api/v1/trips/:id/unassign", post(data::unassign_trip))
        .route("/dispatch/api/v1/trips/:id/dispatch", post(data::dispatch_trip))
        .route("/dispatch/api/v1/trips/:id/undispatch", post(data::undispatch_trip))
        .route("/dispatch/api/v1/trips/:id/cancel", post(data::cancel_trip))
        .route("/dispatch/api/v1/trips/:id/complete", post(data::complete_trip))
        .route("/dispatch/api/v1/trips/:id/stops/:seq/arrive", post(data::stop_arrive))
        .route("/dispatch/api/v1/trips/:id/stops/:seq/depart", post(data::stop_depart))
        .route("/dispatch/api/v1/trips/:id/stops/:seq/late", post(data::stop_late))
        .route("/dispatch/api/v1/trips/:id/check-call", post(data::check_call))
        .route("/dispatch/api/v1/drivers", get(data::list_drivers))
        .route("/dispatch/api/v1/drivers/:id", get(data::get_driver))
        .route(
            "/dispatch/api/v1/drivers/:id/attach-equipment",
            post(driver_writes::attach_equipment_handler),
        )
        .route(
            "/dispatch/api/v1/drivers/:id/detach-equipment",
            post(driver_writes::detach_equipment_handler),
        )
        .route(
            "/dispatch/api/v1/trucks",
            get(data::list_trucks).post(truck_writes::create_truck_handler),
        )
        .route(
            "/dispatch/api/v1/trucks/:id",
            get(data::get_truck).patch(truck_writes::update_truck_handler),
        )
        .route(
            "/dispatch/api/v1/trailers",
            get(data::list_trailers).post(trailer_writes::create_trailer_handler),
        )
        .route(
            "/dispatch/api/v1/trailers/:id",
            get(data::get_trailer).patch(trailer_writes::update_trailer_handler),
        )
        .route("/dispatch/api/v1/events", get(data::list_events))
        // Facilities
        .route(
            "/dispatch/api/v1/facilities",
            get(data::list_facilities).post(facility_writes::create_facility_handler),
        )
        .route(
            "/dispatch/api/v1/facilities/:id",
            get(data::get_facility).patch(facility_writes::update_facility_handler),
        )
        // KPI count endpoints
        .route("/dispatch/api/v1/loads/count", get(data::count_open_loads))
        .route("/dispatch/api/v1/drivers/count", get(data::count_active_drivers))
        .route("/dispatch/api/v1/blobs/count", get(data::count_pending_documents))
        .route("/dispatch/api/v1/events/count", get(data::count_events_today))
        // Blob endpoints
        .route(
            "/dispatch/api/v1/blobs",
            get(blobs::list_blobs).post(blobs::upload_blob).layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route(
            "/dispatch/api/v1/blob/:id",
            get(blobs::get_blob)
                .put(blobs::update_blob)
                .delete(blobs::delete_blob),
        )
        .route("/dispatch/api/v1/blobs/:id/query", post(blobs::query_blob))
        // MCP JSON-RPC 2.0 endpoint for AI agent tool calls. File bytes never
        // travel over MCP (uploads go through presigned URLs), so tool-call
        // payloads are small JSON envelopes; 1 MiB is generous.
        .route(
            "/dispatch/mcp",
            post(mcp::handle).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        // API key management (GET allowed for both JWT and API-key auth; POST/DELETE require JWT)
        .route("/dispatch/api-keys", post(api_keys::create_api_key).get(api_keys::list_api_keys))
        .route("/dispatch/api-keys/:id", delete(api_keys::revoke_api_key))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_dispatcher_auth,
        ))
}

/// Presigned blob byte-transfer routes. Token-authenticated via the `token` query
/// param (see `blob_links`), so these are deliberately mounted WITHOUT the
/// dispatcher JWT middleware — an agent holding only a presigned token (no JWT)
/// must be able to reach them.
pub(crate) fn public_router() -> Router<AppState> {
    Router::new()
        .route(
            "/dispatch/blobs/presigned",
            post(blobs::presigned_upload)
                .layer(DefaultBodyLimit::max(crate::api::blobs::PRESIGNED_UPLOAD_MAX_BYTES)),
        )
        .route(
            "/dispatch/blobs/presigned/:id",
            get(blobs::presigned_download),
        )
}

pub fn dispatcher_portal_router(state: &AppState) -> Router<AppState> {
    auth_router().merge(data_router(state))
}
