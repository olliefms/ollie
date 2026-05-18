use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct VersionResponse {
    pub version: String,
}

/// Server version (matches CARGO_PKG_VERSION). Unauthenticated.
#[utoipa::path(
    get,
    path = "/version",
    tag = "meta",
    responses(
        (status = 200, description = "Server version", body = VersionResponse)
    )
)]
pub async fn get_version() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}
