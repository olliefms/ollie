use crate::db::DbClient;
use uuid::Uuid;

fn now_z() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
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

pub async fn on_trip_cancelled(db: &DbClient, trip_id: Uuid) {
    let _ = db.append_event("trip", trip_id, "trip.cancelled", None, None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "trip cancelled");
}

pub async fn on_stop_arrived(db: &DbClient, trip_id: Uuid, seq: u32) {
    let payload = serde_json::json!({ "seq": seq });
    let _ = db.append_event("trip", trip_id, "stop.arrived", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, seq, "stop arrived");
}

pub async fn on_stop_departed(db: &DbClient, trip_id: Uuid, seq: u32) {
    let payload = serde_json::json!({ "seq": seq });
    let _ = db.append_event("trip", trip_id, "stop.departed", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, seq, "stop departed");
}

pub async fn on_stop_late(db: &DbClient, trip_id: Uuid, seq: u32, eta: Option<String>, notes: Option<String>) {
    let payload = serde_json::json!({ "seq": seq, "eta": eta, "notes": notes });
    let _ = db.append_event("trip", trip_id, "stop.late", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, seq, "stop late");
}

pub async fn on_check_call(db: &DbClient, trip_id: Uuid, location: String, notes: Option<String>, eta_next_stop: Option<String>) {
    let payload = serde_json::json!({ "location": location, "notes": notes, "eta_next_stop": eta_next_stop });
    let _ = db.append_event("trip", trip_id, "check_call", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, "check call");
}
