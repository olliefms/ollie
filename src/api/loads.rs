// src/api/loads.rs
use crate::{
    api::facilities::resolve_or_create_facility,
    error::AppError,
    models::{FacilityResolutionResponse, Stop, StopInput},
    AppState,
};
use serde::Deserialize;
use utoipa::IntoParams;

#[derive(Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListLoadsQuery {
    /// Semantic search query — triggers vector search when present
    pub s: Option<String>,
    /// Filter by status (planned, dispatched, in_transit, delivered, invoiced, settled, cancelled)
    pub status: Option<String>,
    /// Filter by customer name (substring match)
    pub customer: Option<String>,
    /// Filter by created_at >= this date (ISO 8601, e.g. 2024-01-01)
    pub from: Option<String>,
    /// Filter by created_at <= this date (ISO 8601, e.g. 2024-12-31)
    pub to: Option<String>,
    /// Filter by tag (repeat for multiple: ?tag=a&tag=b)
    #[serde(default)]
    pub tag: Vec<String>,
    /// Maximum results (default 20, max 100)
    pub limit: Option<usize>,
    /// Pagination offset (default 0)
    pub offset: Option<usize>,
}

/// Resolve `StopInput`s into validated `Stop`s, creating/deduplicating facilities
/// as needed. Shared by the dispatcher load create/update handlers and the MCP
/// load tools.
pub async fn resolve_stops_pub(state: &AppState, inputs: Vec<StopInput>) -> Result<Vec<Stop>, AppError> {
    resolve_stops(state, inputs).await
}

async fn resolve_stops(state: &AppState, inputs: Vec<StopInput>) -> Result<Vec<Stop>, AppError> {
    let mut stops = Vec::new();
    let mut resolutions: Vec<FacilityResolutionResponse> = Vec::new();

    for (idx, input) in inputs.into_iter().enumerate() {
        if !input.service_type.is_valid_for(&input.stop_type) {
            return Err(AppError::BadRequest(format!(
                "service_type '{}' is not valid for stop_type '{}'",
                input.service_type.as_str(), input.stop_type.as_str()
            )));
        }

        if input.timezone.len() > 64 {
            return Err(AppError::UnprocessableEntity(
                "timezone exceeds maximum length of 64 characters".to_string()
            ));
        }
        let _: chrono_tz::Tz = input.timezone.parse().map_err(|_| {
            AppError::UnprocessableEntity(format!(
                "stop {}: '{}' is not a valid IANA timezone",
                input.sequence, input.timezone
            ))
        })?;

        crate::models::load::validate_stop_time_str(
            &input.scheduled_arrive, &input.timezone, "scheduled_arrive",
        )?;
        if let Some(ref end) = input.scheduled_arrive_end {
            crate::models::load::validate_stop_time_str(end, &input.timezone, "scheduled_arrive_end")?;
        }

        let facility_id = if let Some(id) = input.facility_id {
            state.db.get_facility_by_id(id).await?;
            id
        } else {
            let name = input.facility_name.ok_or_else(|| AppError::BadRequest(
                "stop must provide either facility_id or facility_name + address".into()
            ))?;
            let address = input.address.ok_or_else(|| AppError::BadRequest(
                "stop must provide address when facility_id is not given".into()
            ))?;
            match resolve_or_create_facility(state, &name, &address, input.force_new_facility).await {
                Ok(id) => id,
                Err(AppError::FacilityResolution(res)) => {
                    let mut inner = *res;
                    for r in &mut inner { r.stop_index = idx; }
                    resolutions.extend(inner);
                    continue;
                }
                Err(e) => return Err(e),
            }
        };

        stops.push(Stop {
            sequence: input.sequence,
            stop_type: input.stop_type,
            service_type: input.service_type,
            facility_id,
            scheduled_arrive: input.scheduled_arrive,
            scheduled_arrive_end: input.scheduled_arrive_end,
            actual_arrive: input.actual_arrive,
            actual_depart: input.actual_depart,
            expected_dwell_minutes: input.expected_dwell_minutes,
            detention_free_minutes: input.detention_free_minutes,
            detention_grace_minutes: input.detention_grace_minutes,
            notes: input.notes,
            blob_ids: input.blob_ids,
            timezone: Some(input.timezone),
            actual_arrive_utc: None,
            actual_depart_utc: None,
        });
    }

    if !resolutions.is_empty() {
        return Err(AppError::FacilityResolution(Box::new(resolutions)));
    }

    Ok(stops)
}
