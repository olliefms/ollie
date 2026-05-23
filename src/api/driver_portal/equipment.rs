// src/api/driver_portal/equipment.rs
use axum::{
    Extension,
    Json,
    extract::State,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AppState,
    api::driver_portal::jwt::DriverClaims,
    error::AppError,
    events,
    models::{TripStatus, TripStopType, TrailerStatus},
};

#[derive(Serialize, utoipa::ToSchema)]
pub struct EquipmentTrailerSummary {
    pub id: Uuid,
    pub unit_number: String,
    pub owner: String,
    pub owner_name: Option<String>,
    pub status: String,
    pub trailer_type: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct EquipmentTruckSummary {
    pub id: Uuid,
    pub unit_number: String,
    pub plate: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct DriverEquipmentResponse {
    pub truck: Option<EquipmentTruckSummary>,
    pub trailers: Vec<EquipmentTrailerSummary>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateTrailerRequest {
    #[serde(default)]
    pub trailer_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    pub trailer_unit_numbers: Option<Vec<String>>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct UpdateTrailerResponse {
    pub trailers: Vec<EquipmentTrailerSummary>,
    pub trip_cascade: bool,
    pub trip_id: Option<Uuid>,
}

#[utoipa::path(
    get,
    path = "/driver/api/v1/equipment",
    responses(
        (status = 200, description = "Current driver equipment", body = DriverEquipmentResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn get_equipment(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims.driver_id.parse::<Uuid>().map_err(|_| AppError::Unauthorized)?;
    let driver = state.db.get_driver_by_id(driver_id).await?;

    let truck = if let Some(tid) = driver.current_truck_id {
        state.db.get_truck_by_id(tid).await.ok().map(|t| EquipmentTruckSummary {
            id: t.id,
            unit_number: t.unit_number,
            plate: t.plate,
        })
    } else {
        None
    };

    let mut trailers = Vec::with_capacity(driver.current_trailer_ids.len());
    for tid in &driver.current_trailer_ids {
        if let Ok(t) = state.db.get_trailer_by_id(*tid).await {
            trailers.push(trailer_summary(&t));
        }
    }

    Ok(Json(DriverEquipmentResponse { truck, trailers }))
}

#[utoipa::path(
    put,
    path = "/driver/api/v1/equipment/trailer",
    request_body = UpdateTrailerRequest,
    responses(
        (status = 200, description = "Trailer assignment updated", body = UpdateTrailerResponse),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trailer not found"),
        (status = 409, description = "Conflict — trailer already on another active trip"),
        (status = 422, description = "Validation failed"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn update_trailer(
    State(state): State<AppState>,
    Extension(claims): Extension<DriverClaims>,
    Json(req): Json<UpdateTrailerRequest>,
) -> Result<impl IntoResponse, AppError> {
    let driver_id = claims.driver_id.parse::<Uuid>().map_err(|_| AppError::Unauthorized)?;

    if req.trailer_ids.is_some() && req.trailer_unit_numbers.is_some() {
        return Err(AppError::BadRequest(
            "specify either trailer_ids or trailer_unit_numbers, not both".into(),
        ));
    }

    let resolved_ids: Vec<Uuid> = if let Some(ids) = req.trailer_ids {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let t = state.db.get_trailer_by_id(id).await?;
            if matches!(t.status, TrailerStatus::Inactive | TrailerStatus::OutOfService) {
                return Err(AppError::UnprocessableEntity(format!(
                    "trailer {} is {}",
                    t.unit_number,
                    t.status.as_str()
                )));
            }
            out.push(t.id);
        }
        out
    } else if let Some(units) = req.trailer_unit_numbers {
        let mut out = Vec::with_capacity(units.len());
        for unit in units {
            let t = state.db.get_trailer_by_unit_number(&unit).await?
                .ok_or(AppError::NotFound)?;
            if matches!(t.status, TrailerStatus::Inactive | TrailerStatus::OutOfService) {
                return Err(AppError::UnprocessableEntity(format!(
                    "trailer {} is {}",
                    t.unit_number,
                    t.status.as_str()
                )));
            }
            out.push(t.id);
        }
        out
    } else {
        return Err(AppError::BadRequest(
            "must supply trailer_ids or trailer_unit_numbers".into(),
        ));
    };

    // Reject duplicates within the request.
    let mut seen = std::collections::HashSet::new();
    for id in &resolved_ids {
        if !seen.insert(*id) {
            return Err(AppError::UnprocessableEntity("duplicate trailer in request".into()));
        }
    }

    // Reject trailers already on a different driver's active trip.
    if !resolved_ids.is_empty() {
        let trips = state.db.list_trips(None, None, None).await?;
        for t in &trips {
            if !matches!(t.status, TripStatus::Dispatched | TripStatus::InTransit) { continue; }
            if t.driver_id == Some(driver_id) { continue; }
            for tid in &resolved_ids {
                if t.trailer_ids.contains(tid) {
                    return Err(AppError::Conflict(format!(
                        "trailer {tid} is on another driver's active trip"
                    )));
                }
            }
        }
    }

    let driver = state.db.get_driver_by_id(driver_id).await?;
    let previous_trailer_ids = driver.current_trailer_ids.clone();

    // Determine cascade target: a Dispatched/InTransit trip for this driver
    // where the driver is NOT at the final delivery stop.
    let driver_trips = state.db.list_trips(None, Some(driver_id), None).await?;
    let active_trip = driver_trips.into_iter()
        .find(|t| matches!(t.status, TripStatus::Dispatched | TripStatus::InTransit));

    let mut trip_cascade = false;
    let mut trip_id: Option<Uuid> = None;
    if let Some(trip) = &active_trip {
        trip_id = Some(trip.id);
        if !driver_at_final_delivery(trip) {
            let previous_trip_trailers: std::collections::HashSet<Uuid> =
                trip.trailer_ids.iter().copied().collect();
            let new_set: std::collections::HashSet<Uuid> = resolved_ids.iter().copied().collect();

            state.db.update_trip_resources(
                trip.id,
                trip.driver_id,
                trip.truck_id,
                resolved_ids.clone(),
            ).await?;
            trip_cascade = true;

            // Sync trailer statuses to match the new trip composition. Removed
            // trailers fall back to Available; newly attached trailers track the
            // trip's active phase (InTransit → Dispatched, otherwise Assigned).
            let added_target = match trip.status {
                TripStatus::Dispatched | TripStatus::InTransit => TrailerStatus::Dispatched,
                _ => TrailerStatus::Assigned,
            };
            for old_tid in &previous_trip_trailers {
                if !new_set.contains(old_tid) {
                    let _ = state.db.update_trailer_status(*old_tid, TrailerStatus::Available).await;
                }
            }
            for new_tid in &resolved_ids {
                if !previous_trip_trailers.contains(new_tid) {
                    let _ = state.db.update_trailer_status(*new_tid, added_target.clone()).await;
                }
            }
        }
    }

    let updated = state.db.update_driver_equipment(
        driver_id,
        None,
        Some(resolved_ids.clone()),
    ).await?;

    events::on_driver_trailer_changed(
        &state.db,
        driver_id,
        &previous_trailer_ids,
        &updated.current_trailer_ids,
        trip_id,
        trip_cascade,
    ).await;

    let mut summaries = Vec::with_capacity(updated.current_trailer_ids.len());
    for tid in &updated.current_trailer_ids {
        if let Ok(t) = state.db.get_trailer_by_id(*tid).await {
            summaries.push(trailer_summary(&t));
        }
    }

    Ok(Json(UpdateTrailerResponse {
        trailers: summaries,
        trip_cascade,
        trip_id,
    }))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AvailableTrailerItem {
    pub id: Uuid,
    pub unit_number: String,
    pub owner_name: Option<String>,
    pub trailer_type: Option<String>,
    pub status: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AvailableTrailersResponse {
    pub items: Vec<AvailableTrailerItem>,
}

#[utoipa::path(
    get,
    path = "/driver/api/v1/trailers",
    responses(
        (status = 200, description = "Trailers available for assignment", body = AvailableTrailersResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "driver"
)]
pub async fn list_available_trailers(
    State(state): State<AppState>,
    Extension(_claims): Extension<DriverClaims>,
) -> Result<impl IntoResponse, AppError> {
    // List all trailers (cap at 500 — drivers picking from a fleet typically
    // know the unit number; this is the search corpus).
    let (_total, items) = state.db.list_trailers(None, None, 500, 0).await?;
    let filtered: Vec<AvailableTrailerItem> = items.into_iter()
        .filter(|t| !matches!(t.status.as_str(), "inactive" | "out_of_service"))
        .map(|t| AvailableTrailerItem {
            id: t.id,
            unit_number: t.unit_number,
            owner_name: t.owner_name,
            trailer_type: t.trailer_type,
            status: t.status.as_str().to_string(),
        })
        .collect();
    Ok(Json(AvailableTrailersResponse { items: filtered }))
}

fn trailer_summary(t: &crate::models::TrailerRecord) -> EquipmentTrailerSummary {
    EquipmentTrailerSummary {
        id: t.id,
        unit_number: t.unit_number.clone(),
        owner: t.owner.as_str().to_string(),
        owner_name: t.owner_name.clone(),
        status: t.status.as_str().to_string(),
        trailer_type: t.trailer_type.clone(),
    }
}

/// Driver is "at the final delivery" if the last stop is a Delivery and the
/// driver has recorded arrival there (regardless of whether they've departed).
/// Per the work-issue scope: changing trailer while at final delivery should
/// NOT cascade into the trip's trailer_ids (the dropped trailer stays on the
/// completed trip record; the driver's new trailer is recorded for the next
/// dispatch).
fn driver_at_final_delivery(trip: &crate::models::TripListItem) -> bool {
    let Some(last) = trip.stops.last() else { return false; };
    matches!(last.stop_type, TripStopType::Delivery) && last.actual_arrive.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TripListItem, TripStatus, TripStop, TripStopType};
    use chrono::Utc;

    fn make_trip_with_last_stop(stop_type: TripStopType, actual_arrive: Option<String>) -> TripListItem {
        TripListItem {
            id: Uuid::new_v4(),
            trip_number: "T-001".into(),
            load_id: None,
            load_number: None,
            previous_trip_id: None,
            deadhead_miles: None,
            loaded_miles: None,
            total_miles: None,
            segment_miles: vec![],
            sequence: 0,
            driver_id: None,
            truck_id: None,
            trailer_ids: vec![],
            status: TripStatus::InTransit,
            stops: vec![TripStop {
                sequence: 1,
                stop_type,
                facility_id: None,
                name: Some("X".into()),
                address: None,
                load_stop_index: None,
                scheduled_arrive: None,
                scheduled_arrive_end: None,
                actual_arrive,
                actual_depart: None,
                expected_dwell_minutes: None,
                detention_free_minutes: None,
                detention_grace_minutes: None,
                notes: None,
                timezone: None,
                actual_arrive_utc: None,
                actual_depart_utc: None,
            }],
            notes: None,
            owner_id: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            score: None,
        }
    }

    #[test]
    fn test_at_final_delivery_when_delivery_and_arrived() {
        let trip = make_trip_with_last_stop(TripStopType::Delivery, Some("2026-05-23T10:00:00".into()));
        assert!(driver_at_final_delivery(&trip));
    }

    #[test]
    fn test_not_at_final_delivery_when_not_yet_arrived() {
        let trip = make_trip_with_last_stop(TripStopType::Delivery, None);
        assert!(!driver_at_final_delivery(&trip));
    }

    #[test]
    fn test_not_at_final_delivery_when_last_is_pickup() {
        let trip = make_trip_with_last_stop(TripStopType::Pickup, Some("2026-05-23T10:00:00".into()));
        assert!(!driver_at_final_delivery(&trip));
    }
}
