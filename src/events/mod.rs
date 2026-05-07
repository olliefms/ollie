use uuid::Uuid;

pub async fn on_trip_assigned(trip_id: Uuid) {
    tracing::info!(trip_id = %trip_id, "trip assigned");
}

pub async fn on_trip_unassigned(trip_id: Uuid) {
    tracing::info!(trip_id = %trip_id, "trip unassigned");
}

pub async fn on_trip_dispatched(trip_id: Uuid) {
    tracing::info!(trip_id = %trip_id, "trip dispatched");
}

pub async fn on_trip_undispatched(trip_id: Uuid) {
    tracing::info!(trip_id = %trip_id, "trip undispatched");
}

pub async fn on_trip_in_transit(trip_id: Uuid) {
    tracing::info!(trip_id = %trip_id, "trip in_transit");
}

pub async fn on_trip_delivered(trip_id: Uuid) {
    tracing::info!(trip_id = %trip_id, "trip delivered");
}

pub async fn on_trip_cancelled(trip_id: Uuid) {
    tracing::info!(trip_id = %trip_id, "trip cancelled");
}

pub async fn on_stop_arrived(trip_id: Uuid, seq: u32) {
    tracing::info!(trip_id = %trip_id, seq, "stop arrived");
}

pub async fn on_stop_departed(trip_id: Uuid, seq: u32) {
    tracing::info!(trip_id = %trip_id, seq, "stop departed");
}

pub async fn on_stop_late(trip_id: Uuid, seq: u32) {
    tracing::info!(trip_id = %trip_id, seq, "stop late");
}

pub async fn on_check_call(trip_id: Uuid) {
    tracing::info!(trip_id = %trip_id, "check call");
}
