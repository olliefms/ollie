// tests/trip_lifecycle_fixes_test.rs
//
// Regression tests for dispatcher-reported trip-lifecycle issues (2026-07-06):
//   #2  a non-freight "empty move" trip (terminal/empty_move stops, no pickup)
//       has a real completion path: dispatched -> in_transit -> delivered.
//   #4  assign is a planning action: it succeeds even when the driver/truck is
//       still dispatched on another (chained) trip, leaving the follow-on in
//       assigned. The single-active-dispatch rule is enforced at dispatch, which
//       still refuses to dispatch a driver already dispatched elsewhere.
//   #5  delete distinguishes the soft-cancel (first call) from the hard-delete
//       (second call) via its return value.
//   #6  delete is blocked while another trip still references it as
//       previous_trip_id, and the error names the referencing trip.

use ollie::models::trip::TripStopType;
use ollie::models::trailer::{TrailerOwner, TrailerRecord, TrailerStatus};
use ollie::models::{
    DriverRecord, DriverStatus, TripRecord, TripStatus, TripStop, TruckRecord, TruckStatus,
};
use ollie::services::trip_lifecycle::{self, AssignTripRequest, DeleteOutcome};
use ollie::services::trip_stops;
use ollie::{ai::OllamaClient, config::Config, db::DbClient, storage::BlobStore, AppState};
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

async fn test_state() -> (AppState, TempDir, TempDir) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    std::env::set_var("DRIVER_JWT_SECRET", "test-driver-jwt-secret-that-is-long-enough");
    std::env::set_var("DRIVER_RP_ID", "localhost");
    std::env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
    std::env::set_var("FLEET_JWT_SECRET", "test-fleet_user-secret-must-be-32b");

    let config = Arc::new(Config::from_env().unwrap());
    let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai = Arc::new(OllamaClient::new(
        "http://127.0.0.1:1",
        "nomic-embed-text",
        "llama3.2",
        "moondream",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));
    let (pipeline_tx, _rx) = async_channel::bounded(100);
    let (geocoding_tx, _grx) = async_channel::bounded(100);
    let (routing_tx, _rrx) = async_channel::bounded(100);
    let rp_origin = Url::parse("http://localhost:3000").unwrap();
    let webauthn = Arc::new(
        WebauthnBuilder::new("localhost", &rp_origin)
            .unwrap()
            .build()
            .unwrap(),
    );
    let auth_challenge_store = Arc::new(dashmap::DashMap::new());
    let reg_challenge_store = Arc::new(dashmap::DashMap::new());
    let state = AppState {
        db,
        store,
        ai,
        geocoding,
        ors,
        pipeline_tx,
        geocoding_tx,
        routing_tx,
        config,
        webauthn,
        auth_challenge_store,
        reg_challenge_store,
    };
    (state, blob_dir, db_dir)
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn driver(id: Uuid, status: DriverStatus) -> DriverRecord {
    DriverRecord {
        id,
        name: "Test Driver".into(),
        phone: None,
        email: None,
        license_number: None,
        license_state: None,
        license_expiry: None,
        status,
        notes: None,
        current_truck_id: None,
        current_trailer_ids: vec![],
        blob_ids: vec![],
        embedding: None,
        owner_id: 0,
        created_at: now(),
        updated_at: now(),
        terminal_id: None,
        loaded_rate_per_mile: None,
        deadhead_rate_per_mile: None,
        extra_stop_fee: None,
        detention_rate_per_hour: None,
        free_dwell_minutes: None,
    }
}

fn truck(id: Uuid, status: TruckStatus) -> TruckRecord {
    TruckRecord {
        id,
        unit_number: "T1".into(),
        year: None,
        make: None,
        model: None,
        vin: None,
        plate: None,
        plate_state: None,
        status,
        notes: None,
        blob_ids: vec![],
        embedding: None,
        owner_id: 0,
        created_at: now(),
        updated_at: now(),
    }
}

fn trailer(id: Uuid, status: TrailerStatus) -> TrailerRecord {
    TrailerRecord {
        id,
        unit_number: "TR1".into(),
        owner: TrailerOwner::Fleet,
        owner_name: None,
        year: None,
        make: None,
        trailer_type: None,
        length_ft: None,
        vin: None,
        plate: None,
        plate_state: None,
        status,
        notes: None,
        blob_ids: vec![],
        embedding: None,
        owner_id: 0,
        created_at: now(),
        updated_at: now(),
    }
}

fn stop(seq: u32, stop_type: TripStopType) -> TripStop {
    TripStop {
        sequence: seq,
        stop_type,
        facility_id: None,
        name: Some("Home Yard".into()),
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
        timezone: Some("America/New_York".into()),
        actual_arrive_utc: None,
        actual_depart_utc: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn trip(
    id: Uuid,
    number: &str,
    status: TripStatus,
    driver_id: Option<Uuid>,
    truck_id: Option<Uuid>,
    previous_trip_id: Option<Uuid>,
    stops: Vec<TripStop>,
) -> TripRecord {
    let n = now();
    TripRecord {
        id,
        trip_number: number.into(),
        load_id: None,
        load_number: None,
        previous_trip_id,
        deadhead_miles: None,
        loaded_miles: None,
        total_miles: None,
        segment_miles: vec![],
        sequence: 0,
        driver_id,
        truck_id,
        trailer_ids: vec![],
        status,
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
        created_at: n,
        updated_at: n,
    }
}

// --- #2 -------------------------------------------------------------------

#[tokio::test]
async fn single_terminal_stop_trip_reaches_delivered_and_completes() {
    let (state, _b, _d) = test_state().await;
    let did = Uuid::new_v4();
    state.db.insert_driver(&driver(did, DriverStatus::Dispatched)).await.unwrap();
    let tid = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(
            tid,
            "T-EMPTY-0001",
            TripStatus::Dispatched,
            Some(did),
            None,
            None,
            // Single stop at sequence 2 (not 0/1) per AGENTS.md: keeps a 1-based
            // vs 0-based mixup visible in cascade tests.
            vec![stop(2, TripStopType::Terminal)],
        ))
        .await
        .unwrap();

    trip_stops::record_stop_arrive(&state, tid, 2, "2026-05-22T10:00:00".into())
        .await
        .unwrap();
    let after = trip_stops::record_stop_depart(&state, tid, 2, "2026-05-22T10:30:00".into())
        .await
        .unwrap();
    assert_eq!(
        after.status,
        TripStatus::Delivered,
        "single-stop empty move should cascade to delivered on the lone depart"
    );

    trip_lifecycle::complete(&state, tid).await.unwrap();
    assert_eq!(state.db.get_trip(tid).await.unwrap().status, TripStatus::Completed);
}

#[tokio::test]
async fn two_stop_empty_move_starts_transit_then_delivers() {
    let (state, _b, _d) = test_state().await;
    let tid = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(
            tid,
            "T-EMPTY-0002",
            TripStatus::Dispatched,
            None,
            None,
            None,
            vec![
                stop(1, TripStopType::EmptyMove),
                stop(2, TripStopType::Terminal),
            ],
        ))
        .await
        .unwrap();

    let after0 = trip_stops::record_stop_depart(&state, tid, 1, "2026-05-22T08:00:00".into())
        .await
        .unwrap();
    assert_eq!(
        after0.status,
        TripStatus::InTransit,
        "first stop depart of a no-pickup trip should start transit"
    );

    let after1 = trip_stops::record_stop_depart(&state, tid, 2, "2026-05-22T12:00:00".into())
        .await
        .unwrap();
    assert_eq!(after1.status, TripStatus::Delivered, "final stop depart should deliver");
}

#[tokio::test]
async fn loaded_trip_still_gated_on_pickup_depart() {
    // Regression guard: a freight trip must NOT start transit when a non-pickup
    // stop (origin) departs — only the pickup depart does.
    let (state, _b, _d) = test_state().await;
    let tid = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(
            tid,
            "T-LOAD-0001",
            TripStatus::Dispatched,
            None,
            None,
            None,
            vec![
                stop(1, TripStopType::Origin),
                stop(2, TripStopType::Pickup),
                stop(3, TripStopType::Delivery),
            ],
        ))
        .await
        .unwrap();

    let after_origin = trip_stops::record_stop_depart(&state, tid, 1, "2026-05-22T06:00:00".into())
        .await
        .unwrap();
    assert_eq!(
        after_origin.status,
        TripStatus::Dispatched,
        "departing origin (not the pickup) must not start transit on a loaded trip"
    );

    let after_pickup = trip_stops::record_stop_depart(&state, tid, 2, "2026-05-22T07:00:00".into())
        .await
        .unwrap();
    assert_eq!(after_pickup.status, TripStatus::InTransit);

    let after_delivery = trip_stops::record_stop_depart(&state, tid, 3, "2026-05-22T15:00:00".into())
        .await
        .unwrap();
    assert_eq!(after_delivery.status, TripStatus::Delivered);
}

// --- #4 -------------------------------------------------------------------

#[tokio::test]
async fn assign_allows_dispatched_driver_but_dispatch_rejects() {
    // A driver dispatched on their current (chained) trip can still be *assigned*
    // to the follow-on leg — that's planning, not dispatching. The single-active-
    // dispatch rule fires only when we try to `dispatch` the follow-on.
    let (state, _b, _d) = test_state().await;
    let did = Uuid::new_v4();
    state.db.insert_driver(&driver(did, DriverStatus::Dispatched)).await.unwrap();
    let tkid = Uuid::new_v4();
    state.db.insert_truck(&truck(tkid, TruckStatus::Available)).await.unwrap();
    let tid = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(tid, "T-ASSIGN-0001", TripStatus::Planned, None, None, None, vec![stop(1, TripStopType::Terminal)]))
        .await
        .unwrap();

    let assigned = trip_lifecycle::assign(
        &state,
        tid,
        AssignTripRequest { driver_id: did, truck_id: tkid, trailer_ids: vec![] },
    )
    .await
    .expect("assign should succeed for a dispatched driver (planning the next leg)");
    assert_eq!(assigned.status, TripStatus::Assigned);
    assert_eq!(assigned.driver_id, Some(did));
    // The driver stays Dispatched on their live trip — assign never demotes.
    assert_eq!(
        state.db.get_driver_by_id(did).await.unwrap().status,
        DriverStatus::Dispatched
    );

    let err = trip_lifecycle::dispatch(&state, tid)
        .await
        .expect_err("dispatch must still refuse a driver already dispatched elsewhere");
    assert!(format!("{err:?}").contains("already dispatched"), "got: {err:?}");
}

#[tokio::test]
async fn assign_allows_dispatched_truck_but_dispatch_rejects() {
    let (state, _b, _d) = test_state().await;
    let did = Uuid::new_v4();
    state.db.insert_driver(&driver(did, DriverStatus::Available)).await.unwrap();
    let tkid = Uuid::new_v4();
    state.db.insert_truck(&truck(tkid, TruckStatus::Dispatched)).await.unwrap();
    let tid = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(tid, "T-ASSIGN-0002", TripStatus::Planned, None, None, None, vec![stop(1, TripStopType::Terminal)]))
        .await
        .unwrap();

    let assigned = trip_lifecycle::assign(
        &state,
        tid,
        AssignTripRequest { driver_id: did, truck_id: tkid, trailer_ids: vec![] },
    )
    .await
    .expect("assign should succeed for a dispatched truck (planning the next leg)");
    assert_eq!(assigned.status, TripStatus::Assigned);
    assert_eq!(assigned.truck_id, Some(tkid));
    assert_eq!(
        state.db.get_truck_by_id(tkid).await.unwrap().status,
        TruckStatus::Dispatched
    );

    let err = trip_lifecycle::dispatch(&state, tid)
        .await
        .expect_err("dispatch must still refuse a truck already dispatched elsewhere");
    assert!(format!("{err:?}").contains("already dispatched"), "got: {err:?}");
}

#[tokio::test]
async fn assign_allows_dispatched_trailer_but_rejects_out_of_service() {
    let (state, _b, _d) = test_state().await;
    let did = Uuid::new_v4();
    state.db.insert_driver(&driver(did, DriverStatus::Available)).await.unwrap();
    let tkid = Uuid::new_v4();
    state.db.insert_truck(&truck(tkid, TruckStatus::Available)).await.unwrap();

    // A dispatched trailer (still on the live leg) can be pre-assigned to the follow-on.
    let trid = Uuid::new_v4();
    state.db.insert_trailer(&trailer(trid, TrailerStatus::Dispatched)).await.unwrap();
    let tid = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(tid, "T-ASSIGN-0003", TripStatus::Planned, None, None, None, vec![stop(1, TripStopType::Terminal)]))
        .await
        .unwrap();

    let assigned = trip_lifecycle::assign(
        &state,
        tid,
        AssignTripRequest { driver_id: did, truck_id: tkid, trailer_ids: vec![trid] },
    )
    .await
    .expect("assign should succeed for a dispatched trailer");
    assert_eq!(assigned.status, TripStatus::Assigned);
    assert_eq!(assigned.trailer_ids, vec![trid]);
    assert_eq!(
        state.db.get_trailer_by_id(trid).await.unwrap().status,
        TrailerStatus::Dispatched
    );

    // An out-of-service trailer is genuinely unavailable and is still rejected.
    let oos = Uuid::new_v4();
    state.db.insert_trailer(&trailer(oos, TrailerStatus::OutOfService)).await.unwrap();
    let tid2 = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(tid2, "T-ASSIGN-0004", TripStatus::Planned, None, None, None, vec![stop(1, TripStopType::Terminal)]))
        .await
        .unwrap();
    let err = trip_lifecycle::assign(
        &state,
        tid2,
        AssignTripRequest { driver_id: did, truck_id: tkid, trailer_ids: vec![oos] },
    )
    .await
    .expect_err("assign should reject an out-of-service trailer");
    assert!(format!("{err:?}").contains("not available"), "got: {err:?}");
}

#[tokio::test]
async fn unassign_does_not_demote_a_still_dispatched_resource() {
    // Regression for the double-dispatch corruption: driver D is dispatched on
    // live trip A, then pre-assigned to follow-on trip B. Unassigning B must NOT
    // knock D back to Available (D is still on A) — otherwise the dispatch guard
    // would let D be dispatched a second time.
    let (state, _b, _d) = test_state().await;
    let did = Uuid::new_v4();
    state.db.insert_driver(&driver(did, DriverStatus::Available)).await.unwrap();
    let tkid = Uuid::new_v4();
    state.db.insert_truck(&truck(tkid, TruckStatus::Available)).await.unwrap();
    let trid = Uuid::new_v4();
    state.db.insert_trailer(&trailer(trid, TrailerStatus::Available)).await.unwrap();

    // Trip A: assign then dispatch → driver/truck/trailer all Dispatched.
    let ta = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(ta, "T-UNA-A", TripStatus::Planned, None, None, None, vec![stop(1, TripStopType::Terminal)]))
        .await
        .unwrap();
    trip_lifecycle::assign(&state, ta, AssignTripRequest { driver_id: did, truck_id: tkid, trailer_ids: vec![trid] })
        .await
        .unwrap();
    trip_lifecycle::dispatch(&state, ta).await.unwrap();
    assert_eq!(state.db.get_driver_by_id(did).await.unwrap().status, DriverStatus::Dispatched);

    // Trip B: pre-assign the same still-dispatched resources.
    let tb = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(tb, "T-UNA-B", TripStatus::Planned, None, None, None, vec![stop(1, TripStopType::Terminal)]))
        .await
        .unwrap();
    trip_lifecycle::assign(&state, tb, AssignTripRequest { driver_id: did, truck_id: tkid, trailer_ids: vec![trid] })
        .await
        .unwrap();

    // Unassign B — resources must stay Dispatched (they're live on A).
    trip_lifecycle::unassign(&state, tb).await.unwrap();
    assert_eq!(state.db.get_driver_by_id(did).await.unwrap().status, DriverStatus::Dispatched, "driver must stay dispatched on trip A");
    assert_eq!(state.db.get_truck_by_id(tkid).await.unwrap().status, TruckStatus::Dispatched, "truck must stay dispatched on trip A");
    assert_eq!(state.db.get_trailer_by_id(trid).await.unwrap().status, TrailerStatus::Dispatched, "trailer must stay dispatched on trip A");
}

// --- #5 -------------------------------------------------------------------

#[tokio::test]
async fn delete_soft_cancels_then_hard_deletes() {
    let (state, _b, _d) = test_state().await;
    let tid = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(tid, "T-DEL-0001", TripStatus::Assigned, None, None, None, vec![stop(1, TripStopType::Terminal)]))
        .await
        .unwrap();

    let first = trip_lifecycle::delete(&state, tid).await.unwrap();
    assert_eq!(first, DeleteOutcome::Cancelled);
    assert_eq!(
        state.db.get_trip(tid).await.unwrap().status,
        TripStatus::Cancelled,
        "first delete should only soft-cancel"
    );

    let second = trip_lifecycle::delete(&state, tid).await.unwrap();
    assert_eq!(second, DeleteOutcome::Deleted);
    assert!(state.db.get_trip(tid).await.is_err(), "second delete should hard-delete");
}

// --- #6 -------------------------------------------------------------------

#[tokio::test]
async fn delete_blocked_while_referenced_as_previous_trip() {
    let (state, _b, _d) = test_state().await;
    // The referential guard sits on the hard-delete path, so the target must
    // already be cancelled to reach it.
    let prev = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(prev, "T-PREV-0001", TripStatus::Cancelled, None, None, None, vec![stop(1, TripStopType::Terminal)]))
        .await
        .unwrap();
    let succ = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(succ, "T-SUCC-0002", TripStatus::Planned, None, None, Some(prev), vec![stop(1, TripStopType::Terminal)]))
        .await
        .unwrap();

    let err = trip_lifecycle::delete(&state, prev)
        .await
        .expect_err("delete should be blocked by a dependent previous_trip_id reference");
    let msg = format!("{err:?}");
    assert!(msg.contains("previous_trip_id"), "error should mention previous_trip_id: {msg}");
    assert!(msg.contains("T-SUCC-0002"), "error should name the referencing trip: {msg}");
    assert!(state.db.get_trip(prev).await.is_ok(), "refused delete must leave the trip intact");
}

// --- driver stop-time clear (2026-07-10) ----------------------------------

#[tokio::test]
async fn clear_arrive_also_clears_depart() {
    let (state, _b, _d) = test_state().await;
    let tid = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(
            tid,
            "T-CLEAR-0001",
            TripStatus::Dispatched,
            None,
            None,
            None,
            vec![
                stop(1, TripStopType::EmptyMove),
                stop(2, TripStopType::Terminal),
            ],
        ))
        .await
        .unwrap();

    trip_stops::record_stop_arrive(&state, tid, 1, "2026-05-22T08:00:00".into())
        .await
        .unwrap();
    trip_stops::record_stop_depart(&state, tid, 1, "2026-05-22T08:30:00".into())
        .await
        .unwrap();

    // Clearing the arrival cascades to the departure.
    trip_stops::clear_stop_times(&state, tid, 1, true, false)
        .await
        .unwrap();

    let after = state.db.get_trip(tid).await.unwrap();
    let s1 = after.stops.iter().find(|s| s.sequence == 1).unwrap();
    assert!(s1.actual_arrive.is_none(), "arrival should be cleared");
    assert!(s1.actual_depart.is_none(), "departure should be cleared with the arrival");
}

#[tokio::test]
async fn clear_depart_leaves_arrive() {
    let (state, _b, _d) = test_state().await;
    let tid = Uuid::new_v4();
    state
        .db
        .insert_trip(&trip(
            tid,
            "T-CLEAR-0002",
            TripStatus::Dispatched,
            None,
            None,
            None,
            vec![stop(1, TripStopType::Terminal)],
        ))
        .await
        .unwrap();

    trip_stops::record_stop_arrive(&state, tid, 1, "2026-05-22T08:00:00".into())
        .await
        .unwrap();
    trip_stops::record_stop_depart(&state, tid, 1, "2026-05-22T08:30:00".into())
        .await
        .unwrap();

    trip_stops::clear_stop_times(&state, tid, 1, false, true)
        .await
        .unwrap();

    let after = state.db.get_trip(tid).await.unwrap();
    let s1 = after.stops.iter().find(|s| s.sequence == 1).unwrap();
    assert!(s1.actual_arrive.is_some(), "arrival should remain");
    assert!(s1.actual_depart.is_none(), "only departure should be cleared");
}
