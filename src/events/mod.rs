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
