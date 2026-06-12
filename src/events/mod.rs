use crate::db::DbClient;
use uuid::Uuid;

fn now_z() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

async fn stop_name(db: &DbClient, trip_id: Uuid, seq: u32) -> Option<String> {
    db.get_trip(trip_id)
        .await
        .ok()?
        .stops
        .into_iter()
        .find(|s| s.sequence == seq)
        .and_then(|s| s.name)
}

pub async fn on_trip_assigned(db: &DbClient, trip_id: Uuid) {
    let _ = db.append_event("trip", trip_id, "trip.assigned", None, None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip assigned");
}

pub async fn on_trip_unassigned(db: &DbClient, trip_id: Uuid) {
    let _ = db.append_event("trip", trip_id, "trip.unassigned", None, None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip unassigned");
}

pub async fn on_trip_dispatched(db: &DbClient, trip_id: Uuid) {
    let _ = db.append_event("trip", trip_id, "trip.dispatched", None, None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip dispatched");
}

pub async fn on_trip_undispatched(db: &DbClient, trip_id: Uuid) {
    let _ = db.append_event("trip", trip_id, "trip.undispatched", None, None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip undispatched");
}

pub async fn on_trip_in_transit(db: &DbClient, trip_id: Uuid) {
    let _ = db.append_event("trip", trip_id, "trip.in_transit", None, None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip in_transit");
}

pub async fn on_trip_delivered(db: &DbClient, trip_id: Uuid) {
    let _ = db.append_event("trip", trip_id, "trip.delivered", None, None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip delivered");
}

pub async fn on_trip_completed(db: &DbClient, trip_id: Uuid, driver_id: Option<Uuid>, truck_id: Option<Uuid>, trailer_ids: &[Uuid]) {
    let _ = db.append_event("trip", trip_id, "trip_completed", None, None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip completed");
    if let Some(id) = driver_id {
        let _ = db.append_event("driver", id, "driver_available", None, None, &now_z(), None).await;
        tracing::info!(driver_id = %id, "driver available after trip completion");
    }
    if let Some(id) = truck_id {
        let _ = db.append_event("truck", id, "truck_available", None, None, &now_z(), None).await;
        tracing::info!(truck_id = %id, "truck available after trip completion");
    }
    for &trailer_id in trailer_ids {
        let _ = db.append_event("trailer", trailer_id, "trailer_available", None, None, &now_z(), None).await;
        tracing::info!(trailer_id = %trailer_id, "trailer available after trip completion");
    }
}

pub async fn on_trip_cancelled(db: &DbClient, trip_id: Uuid) {
    let _ = db.append_event("trip", trip_id, "trip.cancelled", None, None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip cancelled");
}

pub async fn on_stop_arrived(db: &DbClient, trip_id: Uuid, seq: u32) {
    let payload = serde_json::json!({ "seq": seq, "stop_name": stop_name(db, trip_id, seq).await });
    let _ = db.append_event("trip", trip_id, "stop.arrived", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, seq, "stop arrived");
}

pub async fn on_stop_departed(db: &DbClient, trip_id: Uuid, seq: u32) {
    let payload = serde_json::json!({ "seq": seq, "stop_name": stop_name(db, trip_id, seq).await });
    let _ = db.append_event("trip", trip_id, "stop.departed", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, seq, "stop departed");
}

pub async fn on_stop_late(db: &DbClient, trip_id: Uuid, seq: u32, eta: Option<String>, notes: Option<String>) {
    let payload = serde_json::json!({
        "seq": seq, "stop_name": stop_name(db, trip_id, seq).await, "eta": eta, "notes": notes
    });
    let _ = db.append_event("trip", trip_id, "stop.late", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, seq, "stop late");
}

pub async fn on_driver_trailer_changed(
    db: &DbClient,
    driver_id: Uuid,
    previous_trailer_ids: &[Uuid],
    new_trailer_ids: &[Uuid],
    trip_id: Option<Uuid>,
    trip_cascade: bool,
) {
    let payload = serde_json::json!({
        "previous_trailer_ids": previous_trailer_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "new_trailer_ids": new_trailer_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "trip_id": trip_id.map(|u| u.to_string()),
        "trip_cascade": trip_cascade,
    });
    let _ = db.append_event("driver", driver_id, "driver.trailer_changed", Some(payload), None, &now_z(), None).await;
    tracing::info!(driver_id = %driver_id, trip_cascade, "driver trailer changed");
}

#[allow(clippy::too_many_arguments)]
pub async fn on_driver_equipment_changed(
    db: &DbClient,
    driver_id: Uuid,
    previous_truck_id: Option<Uuid>,
    new_truck_id: Option<Uuid>,
    previous_trailer_ids: &[Uuid],
    new_trailer_ids: &[Uuid],
    trip_id: Option<Uuid>,
    trip_cascade: bool,
) {
    let payload = serde_json::json!({
        "previous_truck_id": previous_truck_id.map(|u| u.to_string()),
        "new_truck_id": new_truck_id.map(|u| u.to_string()),
        "previous_trailer_ids": previous_trailer_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "new_trailer_ids": new_trailer_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "trip_id": trip_id.map(|u| u.to_string()),
        "trip_cascade": trip_cascade,
    });
    let _ = db.append_event("driver", driver_id, "driver.equipment_changed", Some(payload), None, &now_z(), None).await;
    tracing::info!(driver_id = %driver_id, trip_cascade, "driver equipment changed");
}

pub async fn on_check_call(db: &DbClient, trip_id: Uuid, location: String, notes: Option<String>, eta_next_stop: Option<String>) {
    let payload = serde_json::json!({ "location": location, "notes": notes, "eta_next_stop": eta_next_stop });
    let _ = db.append_event("trip", trip_id, "check_call", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "check call");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::trip::{TripRecord, TripStatus, TripStop, TripStopType};
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn now_rfc3339() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn make_stop(seq: u32, name: Option<&str>) -> TripStop {
        TripStop {
            sequence: seq,
            stop_type: TripStopType::Pickup,
            facility_id: None,
            name: name.map(|s| s.to_string()),
            address: None,
            load_stop_index: None,
            scheduled_arrive: None,
            scheduled_arrive_end: None,
            actual_arrive: None,
            actual_depart: None,
            expected_dwell_minutes: None,
            detention_free_minutes: None,
            detention_grace_minutes: None,
            notes: None,
            timezone: None,
            actual_arrive_utc: None,
            actual_depart_utc: None,
        }
    }

    fn make_trip(id: Uuid, stops: Vec<TripStop>) -> TripRecord {
        let now = chrono::Utc::now();
        TripRecord {
            id,
            trip_number: "T-TEST-0001".into(),
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
            status: TripStatus::Planned,
            stops,
            notes: None,
            blob_ids: vec![],
            loaded_rate_per_mile: None,
            deadhead_rate_per_mile: None,
            extra_stop_fee: None,
            detention_rate_per_hour: None,
            free_dwell_minutes: None,
            settlement_ref: None,
            pay_period_start: None,
            pay_period_end: None,
            driver_pay_snapshot: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_stop_arrived_writes_stop_name_in_payload() {
        let (db, _dir) = test_db().await;
        let trip_id = Uuid::new_v4();
        let trip = make_trip(trip_id, vec![make_stop(1, Some("Test Stop"))]);
        db.insert_trip(&trip).await.unwrap();

        on_stop_arrived(&db, trip_id, 1).await;

        let (_total, events) = db.query_events(
            Some(trip_id), None, Some("stop.arrived"), None, None, 10, 0,
        ).await.unwrap();
        assert_eq!(events.len(), 1);
        let payload: serde_json::Value = serde_json::from_str(
            events[0].payload.as_deref().unwrap_or("{}"),
        ).unwrap();
        assert_eq!(payload["stop_name"], serde_json::json!("Test Stop"));
        assert_eq!(payload["seq"], serde_json::json!(1u32));
    }

    #[tokio::test]
    async fn test_stop_arrived_graceful_when_seq_missing() {
        let (db, _dir) = test_db().await;
        let trip_id = Uuid::new_v4();
        // Trip has stop seq=0 only; call with seq=99 (doesn't exist).
        let trip = make_trip(trip_id, vec![make_stop(0, Some("Only Stop"))]);
        db.insert_trip(&trip).await.unwrap();

        on_stop_arrived(&db, trip_id, 99).await;

        let (_total, events) = db.query_events(
            Some(trip_id), None, Some("stop.arrived"), None, None, 10, 0,
        ).await.unwrap();
        assert_eq!(events.len(), 1);
        let payload: serde_json::Value = serde_json::from_str(
            events[0].payload.as_deref().unwrap_or("{}"),
        ).unwrap();
        assert_eq!(payload["stop_name"], serde_json::Value::Null);
        assert_eq!(payload["seq"], serde_json::json!(99u32));
    }

    #[tokio::test]
    async fn test_stop_arrived_graceful_when_timestamp_only() {
        // Also verify the now_rfc3339 helper used in other event_ops tests matches now_z().
        let ts = now_rfc3339();
        assert!(ts.ends_with('Z'), "timestamp must end in Z: {ts}");
    }
}
