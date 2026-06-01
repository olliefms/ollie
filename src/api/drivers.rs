// src/api/drivers.rs
use serde::Deserialize;
use utoipa::IntoParams;

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListDriversQuery {
    /// Semantic search query — triggers vector search when present
    pub s: Option<String>,
    /// Filter by status (available, assigned, dispatched, inactive)
    pub status: Option<String>,
    /// Maximum results (default 20, max 100)
    pub limit: Option<usize>,
    /// Pagination offset (default 0)
    pub offset: Option<usize>,
}
