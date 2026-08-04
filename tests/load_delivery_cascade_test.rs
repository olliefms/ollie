// tests/load_delivery_cascade_test.rs
//
// Regression tests for #395 — a load stranded in `in_transit`.
//
// `cascade_final_stop_delivered` used to require every trip on a load to be
// exactly `Delivered`, so any sibling trip in a different status permanently
// stranded the load:
//   * a cancelled (superseded) pre-assignment counted against the check;
//   * a completed earlier leg on a relay load did too — and `Delivered ->
//     Completed` is the *normal* end state, so every multi-leg load was exposed.
//
// A stranded load has no supported way out: `invoice` runs only from
// `delivered` and `settle` only from `invoiced`, so the billing chain is out of
// reach, and `cancel` is semantically wrong for freight that physically
// delivered. `load_doctor`'s `load.status_matches_trips` check is the repair
// path for loads stranded before this fix shipped.

use ollie::models::trip::TripStopType;
use ollie::models::{
    LoadKind, LoadRecord, LoadStatus, ServiceType, Stop, StopType, TripRecord, TripStatus,
    TripStop,
};
use ollie::services::doctors;
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
        WebauthnBuilder::new("localhost", &rp_origin).unwrap().build().unwrap(),
    );
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
        auth_challenge_store: Arc::new(dashmap::DashMap::new()),
        reg_challenge_store: Arc::new(dashmap::DashMap::new()),
    };
    (state, blob_dir, db_dir)
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

/// Fixed facilities so a load stop and the trip stop that serves it can be
/// matched — that pairing is the signal `load_doctor` corroborates against.
fn pickup_facility() -> Uuid {
    Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap()
}

fn delivery_facility() -> Uuid {
    Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap()
}

fn load_stop(seq: u32, stop_type: StopType, facility_id: Uuid) -> Stop {
    Stop {
        sequence: seq,
        stop_type,
        service_type: ServiceType::LiveLoad,
        facility_id,
        scheduled_arrive: "2026-05-22T08:00:00".into(),
        scheduled_arrive_end: None,
        actual_arrive: None,
        actual_depart: None,
        expected_dwell_minutes: None,
        detention_free_minutes: None,
        detention_grace_minutes: None,
        notes: None,
        blob_ids: vec![],
        timezone: Some("America/New_York".into()),
        actual_arrive_utc: None,
        actual_depart_utc: None,
    }
}

/// Pickup at one facility, delivery at another — the ordinary single-leg load.
fn freight_stops() -> Vec<Stop> {
    vec![
        load_stop(1, StopType::Pickup, pickup_facility()),
        load_stop(2, StopType::Delivery, delivery_facility()),
    ]
}

fn load(id: Uuid, status: LoadStatus) -> LoadRecord {
    load_with_stops(id, status, freight_stops())
}

fn load_with_stops(id: Uuid, status: LoadStatus, stops: Vec<Stop>) -> LoadRecord {
    LoadRecord {
        id,
        load_number: "4819063".into(),
        owner_id: 0,
        status,
        kind: LoadKind::Freight,
        customer_name: "Acme Freight".into(),
        customer_ref: None,
        stops,
        rate_items: vec![],
        commodity: None,
        weight_lbs: None,
        miles: None,
        notes: None,
        tags: vec![],
        blob_ids: vec![],
        invoice_number: None,
        invoice_date: None,
        cancellation_reason: None,
        embedding: None,
        created_at: now(),
        updated_at: now(),
    }
}

fn stop(seq: u32, stop_type: TripStopType, facility_id: Option<Uuid>) -> TripStop {
    TripStop {
        sequence: seq,
        stop_type,
        facility_id,
        name: Some("Yard".into()),
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

fn trip(id: Uuid, number: &str, load_id: Uuid, status: TripStatus, stops: Vec<TripStop>) -> TripRecord {
    let n = now();
    TripRecord {
        id,
        trip_number: number.into(),
        load_id: Some(load_id),
        load_number: Some("4819063".into()),
        previous_trip_id: None,
        deadhead_miles: None,
        loaded_miles: None,
        total_miles: None,
        segment_miles: vec![],
        sequence: 0,
        driver_id: None,
        truck_id: None,
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

/// Pickup at sequence 2, delivery at 3 — non-1-based per AGENTS.md, so a 0-based
/// vs 1-based index mixup in the cascade stays visible.
fn loaded_stops() -> Vec<TripStop> {
    vec![
        stop(2, TripStopType::Pickup, Some(pickup_facility())),
        stop(3, TripStopType::Delivery, Some(delivery_facility())),
    ]
}

/// A relay leg that hands off rather than delivering — it covers none of the
/// load's delivery stops.
fn relay_leg_stops() -> Vec<TripStop> {
    vec![
        stop(2, TripStopType::Pickup, Some(pickup_facility())),
        stop(3, TripStopType::Relay, Some(Uuid::new_v4())),
    ]
}

/// Drive the delivering trip's final stop and return the load's resulting status.
async fn deliver_final_stop(state: &AppState, trip_id: Uuid, load_id: Uuid) -> LoadStatus {
    trip_stops::record_stop_arrive(state, trip_id, 3, "2026-05-22T10:00:00".into())
        .await
        .unwrap();
    let after = trip_stops::record_stop_depart(state, trip_id, 3, "2026-05-22T10:30:00".into())
        .await
        .unwrap();
    assert_eq!(after.status, TripStatus::Delivered, "final depart must deliver the trip");
    state.db.get_load_by_id(load_id).await.unwrap().status
}

// --- cascade --------------------------------------------------------------

#[tokio::test]
async fn cancelled_sibling_trip_does_not_strand_the_load() {
    let (state, _b, _d) = test_state().await;
    let lid = Uuid::new_v4();
    state.db.insert_load(&load(lid, LoadStatus::InTransit)).await.unwrap();

    // The superseded pre-assignment that stranded load 4819063 in production.
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0033", lid, TripStatus::Cancelled, loaded_stops()))
        .await
        .unwrap();
    let tid = Uuid::new_v4();
    state.db
        .insert_trip(&trip(tid, "T-2026-0034", lid, TripStatus::InTransit, loaded_stops()))
        .await
        .unwrap();

    assert_eq!(
        deliver_final_stop(&state, tid, lid).await,
        LoadStatus::Delivered,
        "#395: a cancelled sibling is a dead record and must not gate the load"
    );
}

#[tokio::test]
async fn completed_sibling_trip_does_not_strand_the_load() {
    let (state, _b, _d) = test_state().await;
    let lid = Uuid::new_v4();
    state.db.insert_load(&load(lid, LoadStatus::InTransit)).await.unwrap();

    // Relay: leg 1 already delivered *and* completed before leg 2 delivers.
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0040", lid, TripStatus::Completed, loaded_stops()))
        .await
        .unwrap();
    let tid = Uuid::new_v4();
    state.db
        .insert_trip(&trip(tid, "T-2026-0041", lid, TripStatus::InTransit, loaded_stops()))
        .await
        .unwrap();

    assert_eq!(
        deliver_final_stop(&state, tid, lid).await,
        LoadStatus::Delivered,
        "#395: Delivered -> Completed is the normal end state and must still cascade"
    );
}

#[tokio::test]
async fn unfinished_sibling_trip_still_holds_the_load() {
    let (state, _b, _d) = test_state().await;
    let lid = Uuid::new_v4();
    state.db.insert_load(&load(lid, LoadStatus::InTransit)).await.unwrap();

    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0050", lid, TripStatus::InTransit, loaded_stops()))
        .await
        .unwrap();
    let tid = Uuid::new_v4();
    state.db
        .insert_trip(&trip(tid, "T-2026-0051", lid, TripStatus::InTransit, loaded_stops()))
        .await
        .unwrap();

    assert_eq!(
        deliver_final_stop(&state, tid, lid).await,
        LoadStatus::InTransit,
        "#395: the fix must not cascade while a sibling leg is still running"
    );
}

// --- load_doctor ----------------------------------------------------------

/// A load already stranded before the cascade fix shipped: `in_transit`, every
/// trip terminal. Nothing drives the cascade for it, so `load_doctor` is the
/// only supported way forward.
async fn stranded_load(state: &AppState) -> Uuid {
    let lid = Uuid::new_v4();
    state.db.insert_load(&load(lid, LoadStatus::InTransit)).await.unwrap();
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0033", lid, TripStatus::Cancelled, loaded_stops()))
        .await
        .unwrap();
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0034", lid, TripStatus::Delivered, loaded_stops()))
        .await
        .unwrap();
    lid
}

fn finding<'a>(report: &'a doctors::DoctorReport, check: &str) -> Option<&'a doctors::Finding> {
    report.findings.iter().find(|f| f.check == check)
}

#[tokio::test]
async fn load_doctor_reports_a_stranded_load_without_mutating_it() {
    let (state, _b, _d) = test_state().await;
    let lid = stranded_load(&state).await;

    let report = doctors::load::run(&state, lid, false).await.unwrap();
    let f = finding(&report, "load.status_matches_trips")
        .expect("#395: load_doctor must detect a load stranded in in_transit");
    assert!(matches!(f.severity, doctors::Severity::Error));
    let fix = f.fix.as_ref().expect("the finding must carry a fix");
    assert_eq!(fix.kind, "advance_load_to_delivered");
    assert!(fix.safe_to_auto_apply);
    assert!(report.dry_run);
    assert!(report.applied.is_empty(), "a dry run must not apply anything");

    assert_eq!(
        state.db.get_load_by_id(lid).await.unwrap().status,
        LoadStatus::InTransit,
        "#395: a dry run must leave the load untouched",
    );
}

#[tokio::test]
async fn load_doctor_apply_advances_a_stranded_load_to_delivered() {
    let (state, _b, _d) = test_state().await;
    let lid = stranded_load(&state).await;

    let report = doctors::load::run(&state, lid, true).await.unwrap();
    assert!(
        report.applied.iter().any(|c| c == "load.status_matches_trips"),
        "#395: apply=true must record the fix as applied, got {:?}",
        report.applied,
    );
    assert_eq!(
        state.db.get_load_by_id(lid).await.unwrap().status,
        LoadStatus::Delivered,
        "#395: apply=true must walk the load forward so it can be invoiced",
    );

    // Idempotent: a second run has nothing left to find.
    let again = doctors::load::run(&state, lid, true).await.unwrap();
    assert!(finding(&again, "load.status_matches_trips").is_none());
    assert!(again.applied.is_empty());
}

/// The deliver-then-cancel ordering: leg 1 handed off at the relay point, leg 2
/// was still `Planned` when it got cancelled, so the load's delivery stop was
/// never reached. Every *live* trip has delivered, so the finding fires — but
/// advancing the load would claim freight arrived somewhere nobody went, and
/// `Delivered` has no reverse edge. The fix must be reported-and-held.
#[tokio::test]
async fn load_doctor_will_not_auto_advance_a_load_with_an_uncovered_delivery_stop() {
    let (state, _b, _d) = test_state().await;
    let lid = Uuid::new_v4();
    state.db.insert_load(&load(lid, LoadStatus::InTransit)).await.unwrap();
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0090", lid, TripStatus::Delivered, relay_leg_stops()))
        .await
        .unwrap();
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0091", lid, TripStatus::Cancelled, loaded_stops()))
        .await
        .unwrap();

    let report = doctors::load::run(&state, lid, true).await.unwrap();
    let f = finding(&report, "load.status_matches_trips")
        .expect("the strand is still worth surfacing");
    let fix = f.fix.as_ref().unwrap();
    assert!(!fix.safe_to_auto_apply, "an uncovered delivery stop must hold the fix");
    assert_eq!(fix.conflicts.len(), 1, "conflicts: {:?}", fix.conflicts);
    assert!(fix.conflicts[0].contains("stop[2]"), "conflicts: {:?}", fix.conflicts);
    assert!(report.applied.is_empty(), "applied: {:?}", report.applied);
    assert!(report.skipped_due_to_conflict.iter().any(|c| c == "load.status_matches_trips"));

    assert_eq!(
        state.db.get_load_by_id(lid).await.unwrap().status,
        LoadStatus::InTransit,
        "#395: apply must not advance a load whose delivery stop was never covered",
    );
}

/// A trip stop with no `facility_id` can't be matched to a load stop, and might
/// be the very stop that covers it. That is absence of signal, not evidence of
/// an unserved stop — treating it as a conflict would make the repair path
/// inert for every trip whose stops were entered without facilities.
#[tokio::test]
async fn load_doctor_applies_when_trip_stops_name_no_facility() {
    let (state, _b, _d) = test_state().await;
    let lid = Uuid::new_v4();
    state.db.insert_load(&load(lid, LoadStatus::InTransit)).await.unwrap();
    let unmatchable = vec![
        stop(2, TripStopType::Pickup, None),
        stop(3, TripStopType::Delivery, None),
    ];
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0100", lid, TripStatus::Delivered, unmatchable))
        .await
        .unwrap();

    let report = doctors::load::run(&state, lid, true).await.unwrap();
    let fix = finding(&report, "load.status_matches_trips").unwrap().fix.as_ref().unwrap();
    assert!(fix.conflicts.is_empty(), "conflicts: {:?}", fix.conflicts);
    assert!(fix.safe_to_auto_apply);
    assert_eq!(state.db.get_load_by_id(lid).await.unwrap().status, LoadStatus::Delivered);
}

#[tokio::test]
async fn load_doctor_is_quiet_while_a_trip_is_still_running() {
    let (state, _b, _d) = test_state().await;
    let lid = Uuid::new_v4();
    state.db.insert_load(&load(lid, LoadStatus::InTransit)).await.unwrap();
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0060", lid, TripStatus::Delivered, loaded_stops()))
        .await
        .unwrap();
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0061", lid, TripStatus::InTransit, loaded_stops()))
        .await
        .unwrap();

    let report = doctors::load::run(&state, lid, true).await.unwrap();
    assert!(finding(&report, "load.status_matches_trips").is_none());
    assert_eq!(state.db.get_load_by_id(lid).await.unwrap().status, LoadStatus::InTransit);
}

#[tokio::test]
async fn load_doctor_is_quiet_when_every_trip_was_cancelled() {
    let (state, _b, _d) = test_state().await;
    let lid = Uuid::new_v4();
    state.db.insert_load(&load(lid, LoadStatus::InTransit)).await.unwrap();
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0070", lid, TripStatus::Cancelled, loaded_stops()))
        .await
        .unwrap();

    let report = doctors::load::run(&state, lid, true).await.unwrap();
    assert!(
        finding(&report, "load.status_matches_trips").is_none(),
        "#395: an all-cancelled load has nothing delivered — it must not be walked to delivered",
    );
    assert_eq!(state.db.get_load_by_id(lid).await.unwrap().status, LoadStatus::InTransit);
}

#[tokio::test]
async fn load_doctor_leaves_a_delivered_load_alone() {
    let (state, _b, _d) = test_state().await;
    let lid = Uuid::new_v4();
    state.db.insert_load(&load(lid, LoadStatus::Delivered)).await.unwrap();
    state.db
        .insert_trip(&trip(Uuid::new_v4(), "T-2026-0080", lid, TripStatus::Completed, loaded_stops()))
        .await
        .unwrap();

    let report = doctors::load::run(&state, lid, true).await.unwrap();
    assert!(finding(&report, "load.status_matches_trips").is_none());
    assert_eq!(state.db.get_load_by_id(lid).await.unwrap().status, LoadStatus::Delivered);
}
