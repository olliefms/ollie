// src/pipeline/routing.rs
use crate::{db::DbClient, error::AppError, routing::RoutingClient};
use uuid::Uuid;

pub async fn process_load_routing(
    id: Uuid,
    db: &DbClient,
    ors: &RoutingClient,
) -> Result<(), AppError> {
    let load = db.get_load_by_id(id).await?;

    if load.stops.is_empty() { return Ok(()); }
    if load.miles.is_some() { return Ok(()); }

    let facility_ids: Vec<Uuid> = load.stops.iter().map(|s| s.facility_id).collect();
    let facilities = db.batch_get_facilities(&facility_ids).await?;

    let waypoints: Vec<(f64, f64)> = load.stops.iter()
        .filter_map(|stop| {
            let f = facilities.get(&stop.facility_id)?;
            Some((f.lat?, f.lng?))
        })
        .collect();

    if waypoints.len() != load.stops.len() {
        tracing::debug!("load {id}: not all stops geocoded yet, skipping routing");
        return Ok(());
    }

    match ors.calculate_route_miles(&waypoints).await {
        Some(miles) => {
            db.update_load_miles(id, miles).await?;
            tracing::info!("load {id}: routed {miles:.1} miles");
        }
        None => {
            tracing::warn!("load {id}: ORS routing returned no result");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::DbClient, routing::RoutingClient};
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_routing_skips_load_with_missing_coordinates() {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap());
        let ors = Arc::new(RoutingClient::new("fake-key"));

        let fac_id = uuid::Uuid::new_v4();
        let now = chrono::Utc::now();
        let facility = crate::models::FacilityRecord {
            id: fac_id, owner_id: 0, name: "Test".into(), address: "Memphis, TN".into(),
            normalized_address: None, lat: None, lng: None,
            geocode_status: crate::models::GeocodeStatus::Pending,
            contacts: vec![], notes: None, tags: vec![], blob_ids: vec![],
            avg_dwell_minutes: None, dwell_sample_count: 0, embedding: None,
            created_at: now, updated_at: now,
        };
        db.insert_facility(&facility).await.unwrap();

        let load_id = uuid::Uuid::new_v4();
        let load = crate::models::LoadRecord {
            id: load_id, load_number: "LD-2026-0001".into(), owner_id: 0,
            status: crate::models::LoadStatus::Planned, customer_name: "ACME".into(),
            customer_ref: None,
            stops: vec![crate::models::Stop {
                sequence: 1, stop_type: crate::models::StopType::Pickup,
                service_type: crate::models::ServiceType::LiveLoad,
                facility_id: fac_id, scheduled_arrive: "2026-05-10".into(),
                scheduled_arrive_end: None, actual_arrive: None, actual_depart: None,
                expected_dwell_minutes: None, detention_free_minutes: None,
                detention_grace_minutes: None,
                notes: None, blob_ids: vec![],
            }],
            rate_items: vec![], commodity: None, weight_lbs: None, miles: None,
            notes: None, tags: vec![], blob_ids: vec![],
            invoice_number: None, invoice_date: None, cancellation_reason: None,
            embedding: None, created_at: now, updated_at: now,
        };
        db.insert_load(&load).await.unwrap();

        // Should complete without error and leave miles as None
        process_load_routing(load_id, &db, &ors).await.unwrap();
        let fetched = db.get_load_by_id(load_id).await.unwrap();
        assert!(fetched.miles.is_none());
    }
}
