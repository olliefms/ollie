// src/api/fleet_portal/mod.rs
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
pub mod terminal_writes;
pub mod trip_writes;
pub mod truck_writes;
pub mod users;

use crate::AppState;
use axum::{
    extract::{DefaultBodyLimit, State},
    response::Response,
    Router,
    routing::{delete, get, post},
};

/// Injects `WWW-Authenticate` on 401 responses from the MCP endpoint only.
/// The header tells OAuth clients where to find the protected-resource metadata.
async fn mcp_www_authenticate(
    State(state): State<AppState>,
    response: Response,
) -> Response {
    if response.status() != axum::http::StatusCode::UNAUTHORIZED {
        return response;
    }
    let prm_url = format!(
        "{}/.well-known/oauth-protected-resource/fleet/mcp",
        state.config.public_base_url.trim_end_matches('/'),
    );
    let header_val = format!("Bearer resource_metadata=\"{prm_url}\"");
    let mut response = response;
    if let Ok(v) = header_val.parse() {
        response.headers_mut().insert(axum::http::header::WWW_AUTHENTICATE, v);
    }
    response
}

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/fleet/auth/login", post(auth::login))
        .route("/fleet/auth/refresh", post(auth::refresh))
        .route("/fleet/auth/logout", post(auth::logout))
        // First-run owner setup wizard — UNauthenticated (table is empty), guarded
        // by count_fleet_users() == 0. Deliberately outside require_fleet_user_auth.
        .route("/fleet/api/v1/setup/status", get(auth::setup_status))
        .route("/fleet/setup", post(auth::setup))
}

pub fn data_router(state: &AppState) -> Router<AppState> {
    Router::new()
        .route("/fleet/api/v1/loads", get(data::list_loads).post(data::create_load))
        .route(
            "/fleet/api/v1/loads/{id}",
            get(data::get_load).put(data::update_load).delete(data::delete_load_handler),
        )
        .route("/fleet/api/v1/loads/{id}/invoice", post(data::invoice_load_handler))
        .route("/fleet/api/v1/loads/{id}/cancel", post(data::cancel_load_handler))
        .route("/fleet/api/v1/loads/{id}/settle", post(data::settle_load_handler))
        .route("/fleet/api/v1/trips", get(data::list_trips).post(data::create_trip_handler))
        .route(
            "/fleet/api/v1/trips/{id}",
            get(data::get_trip)
                .patch(trip_writes::patch_trip_handler)
                .delete(trip_writes::delete_trip_handler),
        )
        .route(
            "/fleet/api/v1/trips/{id}/recalculate-miles",
            post(trip_writes::recalculate_miles_handler),
        )
        .route("/fleet/api/v1/trips/{id}/assign", post(data::assign_trip))
        .route("/fleet/api/v1/trips/{id}/unassign", post(data::unassign_trip))
        .route("/fleet/api/v1/trips/{id}/dispatch", post(data::dispatch_trip))
        .route("/fleet/api/v1/trips/{id}/undispatch", post(data::undispatch_trip))
        .route("/fleet/api/v1/trips/{id}/cancel", post(data::cancel_trip))
        .route("/fleet/api/v1/trips/{id}/complete", post(data::complete_trip))
        .route("/fleet/api/v1/trips/{id}/stops/{seq}/arrive", post(data::stop_arrive))
        .route("/fleet/api/v1/trips/{id}/stops/{seq}/depart", post(data::stop_depart))
        .route("/fleet/api/v1/trips/{id}/stops/{seq}/late", post(data::stop_late))
        .route("/fleet/api/v1/trips/{id}/check-call", post(data::check_call))
        .route("/fleet/api/v1/drivers", get(data::list_drivers).post(driver_writes::create_driver_handler))
        .route(
            "/fleet/api/v1/drivers/{id}",
            get(data::get_driver)
                .patch(driver_writes::patch_driver_handler)
                .delete(driver_writes::delete_driver_handler),
        )
        .route("/fleet/api/v1/drivers/{id}/pin", post(driver_writes::set_driver_pin_handler))
        .route(
            "/fleet/api/v1/drivers/{id}/attach-equipment",
            post(driver_writes::attach_equipment_handler),
        )
        .route(
            "/fleet/api/v1/drivers/{id}/detach-equipment",
            post(driver_writes::detach_equipment_handler),
        )
        .route(
            "/fleet/api/v1/trucks",
            get(data::list_trucks).post(truck_writes::create_truck_handler),
        )
        .route(
            "/fleet/api/v1/trucks/{id}",
            get(data::get_truck)
                .patch(truck_writes::update_truck_handler)
                .delete(truck_writes::delete_truck_handler),
        )
        .route(
            "/fleet/api/v1/trailers",
            get(data::list_trailers).post(trailer_writes::create_trailer_handler),
        )
        .route(
            "/fleet/api/v1/trailers/{id}",
            get(data::get_trailer)
                .patch(trailer_writes::update_trailer_handler)
                .delete(trailer_writes::delete_trailer_handler),
        )
        .route("/fleet/api/v1/events", get(data::list_events))
        // Facilities
        .route(
            "/fleet/api/v1/facilities",
            get(data::list_facilities).post(facility_writes::create_facility_handler),
        )
        .route(
            "/fleet/api/v1/facilities/{id}",
            get(data::get_facility).patch(facility_writes::update_facility_handler),
        )
        // Terminals
        .route(
            "/fleet/api/v1/terminals",
            get(terminal_writes::list_terminals).post(terminal_writes::create_terminal),
        )
        .route(
            "/fleet/api/v1/terminals/{id}", // axum 0.8 path syntax — NOT :id
            get(terminal_writes::get_terminal)
                .put(terminal_writes::update_terminal)
                .delete(terminal_writes::delete_terminal),
        )
        // KPI count endpoints
        .route("/fleet/api/v1/loads/count", get(data::count_open_loads))
        .route("/fleet/api/v1/drivers/count", get(data::count_active_drivers))
        .route("/fleet/api/v1/blobs/count", get(data::count_pending_documents))
        .route("/fleet/api/v1/events/count", get(data::count_events_today))
        // Blob endpoints
        .route(
            "/fleet/api/v1/blobs",
            get(blobs::list_blobs).post(blobs::upload_blob).layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route(
            "/fleet/api/v1/blob/{id}",
            get(blobs::get_blob)
                .put(blobs::update_blob)
                .delete(blobs::delete_blob),
        )
        .route("/fleet/api/v1/blobs/{id}/query", post(blobs::query_blob))
        // API key management (GET allowed for both JWT and API-key auth; POST/DELETE require JWT)
        .route("/fleet/api-keys", post(api_keys::create_api_key).get(api_keys::list_api_keys))
        .route("/fleet/api-keys/{id}", delete(api_keys::revoke_api_key))
        // Fleet users management (#331) — gated by users:* scopes (owner + fleet_manager).
        .merge(users::router())
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_fleet_user_auth,
        ))
        // MCP route: auth + body limit + WWW-Authenticate on 401.
        // Mounted separately (outside the shared route_layer) so that the
        // map_response_with_state layer wraps the entire auth+handler stack and
        // can inject WWW-Authenticate on the auth 401 too.
        .merge(mcp_router(state))
}

/// MCP endpoint with its own layering stack:
///   map_response_with_state (outer, sees auth 401s)
///     → require_fleet_user_auth (route_layer)
///       → rmcp StreamableHttpService (nested; owns GET/POST/DELETE + JSON-RPC)
///
/// The rmcp service is mounted with `nest_service` (not `post(...)`) because it
/// dispatches on the HTTP method internally. A `RequestBodyLimitLayer` replaces
/// the old `DefaultBodyLimit` — the rmcp service reads the body itself rather
/// than through an axum extractor, so the tower-level limit is what applies.
///
/// Keeping it separate from data_router ensures the map_response layer wraps the
/// whole auth+handler stack — a route-level layer on a route inside a router
/// with route_layer would NOT see the auth 401.
fn mcp_router(state: &AppState) -> Router<AppState> {
    Router::new()
        .nest_service("/fleet/mcp", mcp::mcp_service(state))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_fleet_user_auth,
        ))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(1024 * 1024))
        .layer(axum::middleware::map_response_with_state(state.clone(), mcp_www_authenticate))
}

/// Presigned blob byte-transfer routes. Token-authenticated via the `token` query
/// param (see `blob_links`), so these are deliberately mounted WITHOUT the
/// fleet user JWT middleware — an agent holding only a presigned token (no JWT)
/// must be able to reach them.
pub(crate) fn public_router() -> Router<AppState> {
    Router::new()
        .route(
            "/fleet/blobs/presigned",
            post(blobs::presigned_upload)
                .layer(DefaultBodyLimit::max(crate::api::blobs::PRESIGNED_UPLOAD_MAX_BYTES)),
        )
        .route(
            "/fleet/blobs/presigned/{id}",
            get(blobs::presigned_download),
        )
}

pub fn fleet_portal_router(state: &AppState) -> Router<AppState> {
    auth_router().merge(data_router(state))
}
