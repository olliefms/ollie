// tests/tonu_test.rs
//
// TONU (Truck Ordered Not Used): a load is dispatched, the truck rolls to the
// shipper, and the driver is released before loading. The truck was ordered and
// it moved, so the deadhead and any dock wait are payable and the broker owes a
// fee — none of which survives a plain cancel.
//
// Integration tests run with RoutingClient::new(""), so ORS is always
// unavailable. Mileage must degrade to a warning, never an error; these tests
// assert on stop structure and status, not mile counts. Driver pay is likewise
// out of scope here — with no ORS a TONU'd trip ends with `deadhead_miles: None`
// and earns nothing; `tests/terminals_pay_settlement_test.rs` sets mileage on the
// record directly and is where TONU pay is proven.

use axum_test::TestServer;
use ollie::{
    ai::OllamaClient, api, config::Config, db::DbClient, models::LoadStatus, storage::BlobStore,
    AppState,
};
use std::sync::Arc;
use tempfile::TempDir;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

async fn setup() -> (TestServer, AppState, TempDir, TempDir, async_channel::Receiver<ollie::pipeline::PipelineJob>) {
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
        // Deliberately unreachable: integration tests must not depend on a live
        // Ollama (a real one on :11434 feeds wrong-dim embeddings into the test schema).
        "http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream",
    ));
    let geocoding = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors = Arc::new(ollie::routing::RoutingClient::new(""));
    // Keep capacity generous and the receiver alive: dropping it closes the
    // channel and blob uploads (which await pipeline_tx.send) start failing.
    let (pipeline_tx, rx) = async_channel::bounded(100);
    let (geocoding_tx, _grx) = async_channel::bounded(100);
    let (routing_tx, _rrx) = async_channel::bounded(100);
    let rp_origin = Url::parse("http://localhost:3000").unwrap();
    let webauthn = Arc::new(
        WebauthnBuilder::new("localhost", &rp_origin).unwrap().build().unwrap(),
    );
    let auth_challenge_store = Arc::new(dashmap::DashMap::new());
    let reg_challenge_store = Arc::new(dashmap::DashMap::new());

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
        webauthn, auth_challenge_store, reg_challenge_store,
    };
    let server = TestServer::new(api::router(state.clone())).unwrap();
    (server, state, blob_dir, db_dir, rx)
}

const OWNER_EMAIL: &str = "owner@example.com";
const OWNER_PASSWORD: &str = "owner-password-123";

async fn setup_owner(server: &TestServer) -> String {
    let resp = server.post("/fleet/setup")
        .json(&serde_json::json!({
            "email": OWNER_EMAIL, "name": "Owner", "password": OWNER_PASSWORD,
        }))
        .await;
    if resp.status_code() == 200 {
        return resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string();
    }
    let login = server.post("/fleet/auth/login")
        .json(&serde_json::json!({ "email": OWNER_EMAIL, "password": OWNER_PASSWORD }))
        .await;
    assert_eq!(login.status_code(), 200, "owner login failed");
    login.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

async fn create_test_facility(server: &TestServer, token: &str, name: &str, address: &str) -> String {
    let resp = server.post("/fleet/api/v1/facilities")
        .authorization_bearer(token)
        .json(&serde_json::json!({ "name": name, "address": address }))
        .await;
    assert_eq!(resp.status_code(), 201, "create facility failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn create_driver(server: &TestServer, token: &str, name: &str) -> String {
    let resp = server.post("/fleet/api/v1/drivers")
        .authorization_bearer(token)
        .json(&serde_json::json!({ "name": name }))
        .await;
    assert_eq!(resp.status_code(), 201, "create driver failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn create_truck(server: &TestServer, token: &str, unit_number: &str) -> String {
    let resp = server.post("/fleet/api/v1/trucks")
        .authorization_bearer(token)
        .json(&serde_json::json!({ "unit_number": unit_number }))
        .await;
    assert_eq!(resp.status_code(), 201, "create truck failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

fn stop_json(fac_id: &str, scheduled_arrive: &str) -> serde_json::Value {
    serde_json::json!({
        "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
        "facility_id": fac_id, "scheduled_arrive": scheduled_arrive,
        "timezone": "America/Chicago"
    })
}

async fn dispatched_trip(
    server: &TestServer, token: &str, load_number: &str,
) -> (String, String, String) {
    let fac_id = create_test_facility(server, token, &format!("{load_number} Dock"), "Chicago, IL").await;
    let driver_id = create_driver(server, token, &format!("{load_number} Driver")).await;
    let truck_id = create_truck(server, token, &format!("T-{load_number}")).await;

    let resp = server.post("/fleet/api/v1/loads")
        .authorization_bearer(token)
        .json(&serde_json::json!({
            "load_number": load_number,
            "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")],
            "rate_items": [{ "description": "Line Haul", "amount_usd": 1800.0 }],
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let load_id = resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_resp = server.post("/fleet/api/v1/trips")
        .authorization_bearer(token)
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [{
                "sequence": 0, "stop_type": "pickup", "facility_id": fac_id,
                "name": "Dock", "scheduled_arrive": "2026-06-01T08:00:00",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(trip_resp.status_code(), 201, "trip create failed: {}", trip_resp.text());
    let trip_id = trip_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let assign = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .authorization_bearer(token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200, "assign failed: {}", assign.text());

    let dispatch = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .authorization_bearer(token).await;
    assert_eq!(dispatch.status_code(), 200, "dispatch failed: {}", dispatch.text());

    (load_id, trip_id, driver_id)
}

#[tokio::test]
async fn test_tonu_after_arrival_truncates_and_clears_rate_items() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, driver_id) = dispatched_trip(&server, &token, "4581480").await;

    // Driver reaches the dock and waits. Arrival does not move the status: only
    // departure from the pickup starts transit, so this is still `dispatched`.
    let arrive = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" }))
        .await;
    assert_eq!(arrive.status_code(), 200, "arrive failed: {}", arrive.text());

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "reason": "shipper cancelled while waiting to load" }))
        .await;
    assert_eq!(resp.status_code(), 200, "tonu failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["status"], "tonu");
    assert_eq!(trip["stops"].as_array().unwrap().len(), 1, "truncated to the reached stop");
    // The release time is what makes detention payable.
    assert!(trip["stops"][0]["actual_depart"].is_string(),
            "release time must be stamped so the dock wait is billable");
    assert!(trip["loaded_miles"].is_null(), "a TONU trip has zero loaded miles");

    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(load["status"], "tonu");
    assert_eq!(load["rate_items"].as_array().unwrap().len(), 0,
               "line haul will never be earned");
    assert_eq!(load["quoted_rate_items"].as_array().unwrap().len(), 1,
               "but the booked rate must not be lost");
    assert_eq!(load["cancellation_reason"], "shipper cancelled while waiting to load");

    // Equipment is released.
    let driver: serde_json::Value = server.get(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(driver["status"], "available");
}

/// The regression test for the entire reason TONU truncates.
///
/// A follow-on trip's deadhead origin is `previous_trip.stops.last().facility_id`
/// (`src/api/trips.rs`). If a TONU'd trip kept its unreached delivery stop,
/// every downstream deadhead would be measured from a city the truck never
/// visited — and the resulting number is plausible, so nobody would catch it.
#[tokio::test]
async fn test_follow_on_trip_deadheads_from_where_the_truck_actually_is() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    // A two-stop trip: shipper in Chicago, consignee in Denver.
    let shipper = create_test_facility(&server, &token, "Chicago Shipper", "Chicago, IL").await;
    let consignee = create_test_facility(&server, &token, "Denver Consignee", "Denver, CO").await;
    let driver_id = create_driver(&server, &token, "Chain Driver").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN").await;

    let load = server.post("/fleet/api/v1/loads").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581489", "customer_name": "Landstar",
            "stops": [stop_json(&shipper, "2026-06-01T08:00:00")]
        })).await;
    let load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "facility_id": shipper,
                  "name": "Chicago Shipper", "scheduled_arrive": "2026-06-01T08:00:00",
                  "timezone": "America/Chicago" },
                { "sequence": 1, "stop_type": "delivery", "facility_id": consignee,
                  "name": "Denver Consignee", "scheduled_arrive": "2026-06-02T08:00:00",
                  "timezone": "America/Denver" }
            ]
        })).await;
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id })).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .authorization_bearer(&token).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" })).await;

    let tonu = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "reason": "shipper cancelled at the dock" })).await;
    assert_eq!(tonu.status_code(), 200, "tonu failed: {}", tonu.text());

    // The dispatcher chains the next trip off the TONU'd one.
    let next_fac = create_test_facility(&server, &token, "Next Shipper", "Peoria, IL").await;
    let next = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "previous_trip_id": trip_id,
            "stops": [{ "sequence": 0, "stop_type": "pickup", "facility_id": next_fac,
                        "name": "Next Shipper", "scheduled_arrive": "2026-06-02T08:00:00",
                        "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(next.status_code(), 201, "next trip create failed: {}", next.text());
    let next_id: uuid::Uuid = next.json::<serde_json::Value>()["id"]
        .as_str().unwrap().parse().unwrap();

    // Assert the resolved origin directly rather than the mileage, since these
    // tests run without ORS.
    let next_trip = state.db.get_trip(next_id).await.unwrap();
    let summary = ollie::api::mileage_summary::build_mileage_summary(&state, &next_trip).await;
    let origin = summary.origin.expect("a chained trip must resolve a deadhead origin");
    assert_eq!(
        origin.facility_name.as_deref(), Some("Chicago Shipper"),
        "the follow-on must deadhead from the dock the truck is sitting at, \
         not from the Denver consignee it never reached",
    );
}

/// `last_reached` is derived by `sequence`, but the kept prefix is then stamped
/// and renumbered by vector *position* — and nothing in the codebase sorts trip
/// stops (`src/api/trips.rs` stores whatever order the caller supplied). Without
/// a sort, a trip whose stops arrived out of order has its history reversed AND
/// the release time stamped on the wrong stop, which bills detention against a
/// dock the truck left hours earlier.
#[tokio::test]
async fn test_tonu_stamps_and_renumbers_by_sequence_not_by_stored_order() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    let a = create_test_facility(&server, &token, "Shipper A", "Chicago, IL").await;
    let b = create_test_facility(&server, &token, "Shipper B", "Joliet, IL").await;
    let c = create_test_facility(&server, &token, "Consignee C", "Denver, CO").await;
    let driver_id = create_driver(&server, &token, "Unsorted TONU Driver").await;
    let truck_id = create_truck(&server, &token, "T-UNSORTED-TONU").await;

    let load = server.post("/fleet/api/v1/loads").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581496", "customer_name": "Landstar",
            "stops": [stop_json(&a, "2026-06-01T08:00:00")]
        })).await;
    assert_eq!(load.status_code(), 201, "load create failed: {}", load.text());
    let load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Supplied middle-first: vector order and sequence order disagree.
    let trip = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "facility_id": b,
                  "name": "Shipper B", "scheduled_arrive": "2026-06-01T12:00:00",
                  "timezone": "America/Chicago" },
                { "sequence": 0, "stop_type": "pickup", "facility_id": a,
                  "name": "Shipper A", "scheduled_arrive": "2026-06-01T08:00:00",
                  "timezone": "America/Chicago" },
                { "sequence": 2, "stop_type": "delivery", "facility_id": c,
                  "name": "Consignee C", "scheduled_arrive": "2026-06-02T08:00:00",
                  "timezone": "America/Denver" }
            ]
        })).await;
    assert_eq!(trip.status_code(), 201, "trip create failed: {}", trip.text());
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id })).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .authorization_bearer(&token).await;
    // Arrive at both pickups without departing either: departing a pickup would
    // start transit and put this in divert territory instead.
    for (seq, at) in [(0, "2026-06-01T08:00:00"), (1, "2026-06-01T12:00:00")] {
        let r = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/{seq}/arrive"))
            .authorization_bearer(&token)
            .json(&serde_json::json!({ "actual_arrive": at })).await;
        assert_eq!(r.status_code(), 200, "arrive {seq} failed: {}", r.text());
    }

    let tonu = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "occurred_at": "2026-06-01T15:00:00" })).await;
    assert_eq!(tonu.status_code(), 200, "tonu failed: {}", tonu.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 2, "truncated to the two reached pickups");
    assert_eq!(stops[0]["name"], "Shipper A",
        "the truck visited A then B; renumbering by vector position swaps them and \
         makes every downstream deadhead start from the wrong city");
    assert_eq!(stops[1]["name"], "Shipper B");
    assert_eq!(stops[1]["actual_depart"], "2026-06-01T15:00:00",
        "the release time belongs on the stop the truck was actually sitting at");
    assert!(stops[0]["actual_depart"].is_null(),
        "stamping A would bill three hours of detention against a dock the driver \
         left at noon");
}

#[tokio::test]
async fn test_tonu_before_arrival_requires_a_waypoint() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581481").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(resp.status_code(), 422, "no stop was reached, so the truck's \
        position is unknown and the deadhead cannot be measured: {}", resp.text());
}

#[tokio::test]
async fn test_tonu_before_arrival_replaces_stop_zero_with_the_waypoint() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581482").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": {
                "facility_name": "Pilot Effingham", "address": "Effingham, IL",
                "timezone": "America/Chicago",
                "actual_arrive": "2026-06-01T06:30:00"
            },
            "reason": "cancelled en route"
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "tonu failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0]["stop_type"], "waypoint");
    assert_eq!(stops[0]["name"], "Pilot Effingham",
               "the dock the truck never saw must not remain the trip's endpoint");
}

#[tokio::test]
async fn test_tonu_rejects_wrong_statuses_and_names_the_right_verb() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    // Planned: never dispatched, so this is an ordinary cancellation.
    let fac_id = create_test_facility(&server, &token, "Planned Dock", "Chicago, IL").await;
    let load = server.post("/fleet/api/v1/loads").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581483", "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")]
        })).await;
    let load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let trip = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({ "load_id": load_id })).await;
    let planned_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post(&format!("/fleet/api/v1/trips/{planned_id}/tonu"))
        .authorization_bearer(&token).json(&serde_json::json!({})).await;
    assert_eq!(resp.status_code(), 409);
    assert!(resp.text().contains("cancel_trip"),
            "the error must point at the right verb: {}", resp.text());

    // In transit: freight is aboard, so this is a diversion. The trip needs two
    // stops — on a single-stop trip the one depart is also the FINAL depart, so
    // the cascade runs it straight through in_transit to delivered.
    let shipper = create_test_facility(&server, &token, "Rolling Shipper", "Chicago, IL").await;
    let consignee = create_test_facility(&server, &token, "Rolling Consignee", "Denver, CO").await;
    let driver_id = create_driver(&server, &token, "Rolling Driver").await;
    let truck_id = create_truck(&server, &token, "T-ROLL").await;
    let load = server.post("/fleet/api/v1/loads").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581484", "customer_name": "Landstar",
            "stops": [stop_json(&shipper, "2026-06-01T08:00:00")]
        })).await;
    let rolling_load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let trip = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_id": rolling_load_id,
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "facility_id": shipper,
                  "name": "Rolling Shipper", "scheduled_arrive": "2026-06-01T08:00:00",
                  "timezone": "America/Chicago" },
                { "sequence": 1, "stop_type": "delivery", "facility_id": consignee,
                  "name": "Rolling Consignee", "scheduled_arrive": "2026-06-02T08:00:00",
                  "timezone": "America/Denver" }
            ]
        })).await;
    let rolling_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/fleet/api/v1/trips/{rolling_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id })).await;
    server.post(&format!("/fleet/api/v1/trips/{rolling_id}/dispatch"))
        .authorization_bearer(&token).await;
    server.post(&format!("/fleet/api/v1/trips/{rolling_id}/stops/0/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" })).await;
    server.post(&format!("/fleet/api/v1/trips/{rolling_id}/stops/0/depart"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_depart": "2026-06-01T09:00:00" })).await;

    let rolling: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{rolling_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(rolling["status"], "in_transit", "fixture must be carrying freight");

    let resp = server.post(&format!("/fleet/api/v1/trips/{rolling_id}/tonu"))
        .authorization_bearer(&token).json(&serde_json::json!({})).await;
    assert_eq!(resp.status_code(), 409);
    assert!(resp.text().contains("divert_trip"),
            "the error must point at the right verb: {}", resp.text());
}

#[tokio::test]
async fn test_tonu_does_not_auto_dispatch_the_next_trip() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, driver_id) = dispatched_trip(&server, &token, "4581485").await;

    // A follow-on already staged for the same driver.
    let fac_b = create_test_facility(&server, &token, "Next Dock", "Peoria, IL").await;
    let truck_b = create_truck(&server, &token, "T-NEXT").await;
    let trip_b = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "stops": [{
                "sequence": 0, "stop_type": "pickup", "facility_id": fac_b,
                "name": "Next Dock", "scheduled_arrive": "2026-06-02T08:00:00",
                "timezone": "America/Chicago"
            }]
        })).await;
    let trip_b_id = trip_b.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/fleet/api/v1/trips/{trip_b_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_b })).await;

    let tonu = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;
    assert_eq!(tonu.status_code(), 200, "tonu failed: {}", tonu.text());

    let b: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_b_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(b["status"], "assigned",
        "after a TONU the truck is not where the plan assumed, and auto-dispatch \
         does not recompute mileage — the dispatcher must decide");
}

#[tokio::test]
async fn test_tonu_load_invoices_and_settles() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581486").await;

    let tonu = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;
    assert_eq!(tonu.status_code(), 200, "tonu failed: {}", tonu.text());

    // The agreed fee arrives days later.
    let update = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "rate_items": [{ "description": "TONU", "amount_usd": 250.0 }]
        })).await;
    assert_eq!(update.status_code(), 200, "rate update failed: {}", update.text());

    let inv = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "invoice_number": "JQL-4581486" })).await;
    assert_eq!(inv.status_code(), 200, "invoice failed: {}", inv.text());

    let settle = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .authorization_bearer(&token).json(&serde_json::json!({})).await;
    assert_eq!(settle.status_code(), 200, "settle failed: {}", settle.text());

    let uuid: uuid::Uuid = load_id.parse().unwrap();
    assert_eq!(state.db.get_load_by_id(uuid).await.unwrap().status, LoadStatus::Settled);
}

#[tokio::test]
async fn test_tonu_leg_does_not_strand_a_load_with_a_live_sibling() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581487").await;

    // A second leg still holds the load.
    let fac_b = create_test_facility(&server, &token, "Relay Dock", "Gary, IN").await;
    let driver_b = create_driver(&server, &token, "Relay Driver").await;
    let truck_b = create_truck(&server, &token, "T-RELAY").await;
    let trip_b = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [{
                "sequence": 0, "stop_type": "delivery", "facility_id": fac_b,
                "name": "Relay Dock", "scheduled_arrive": "2026-06-02T08:00:00",
                "timezone": "America/Chicago"
            }]
        })).await;
    let trip_b_id = trip_b.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/fleet/api/v1/trips/{trip_b_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_b, "truck_id": truck_b })).await;

    let tonu = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;
    assert_eq!(tonu.status_code(), 200, "tonu failed: {}", tonu.text());

    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    // Not `assert_ne!(.., "tonu")` — that passes for a wrong value too, so a
    // silent demotion back to `planned` would slip through.
    assert_eq!(load["status"], "dispatched",
        "a live sibling still holds the load; only the last leg out takes it terminal");
}

/// The other half of `resolve_position`'s facility branch: a dispatcher who
/// already has the yard on file names it by id, and no dedup runs at all.
#[tokio::test]
async fn test_tonu_waypoint_accepts_an_existing_facility_id() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581492").await;
    let yard = create_test_facility(&server, &token, "Joliet Yard", "Joliet, IL").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_id": yard, "timezone": "America/Chicago" }
        })).await;
    assert_eq!(resp.status_code(), 200, "tonu failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0]["facility_id"], yard);
    assert_eq!(stops[0]["name"], "Joliet Yard",
               "the name must come from the named facility, not be left null");
    assert_eq!(stops[0]["stop_type"], "waypoint");
}

/// `resolve_position` validates the zone before anything is written, because a
/// zone it cannot parse means every stop time on the stop is unreadable.
#[tokio::test]
async fn test_tonu_waypoint_rejects_an_invalid_timezone() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581493").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "US/Notazone" }
        })).await;
    assert_eq!(resp.status_code(), 422, "{}", resp.text());
    assert!(resp.text().contains("US/Notazone"), "the error must name the bad value: {}", resp.text());

    // Rejected before the first write, so the trip is untouched and retryable.
    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["status"], "dispatched", "a rejected TONU must not half-apply");
}

/// The release time is what makes the dock wait billable, so an explicitly
/// supplied one must land verbatim on the truncation stop.
#[tokio::test]
async fn test_tonu_occurred_at_stamps_the_reached_stop() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581494").await;

    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" })).await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "occurred_at": "2026-06-01T11:45:00" })).await;
    assert_eq!(resp.status_code(), 200, "tonu failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["stops"][0]["actual_depart"], "2026-06-01T11:45:00",
               "three and three-quarter hours of detention, not 'now'");
}

/// `occurred_at` is defined in the truncation stop's timezone. With no stop
/// reached there is no zone to read it in, so it is rejected rather than dropped
/// — a 200 that ignored it would read as "applied".
#[tokio::test]
async fn test_tonu_rejects_occurred_at_when_no_stop_was_reached() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581495").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "occurred_at": "2026-06-01T11:45:00",
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;
    assert_eq!(resp.status_code(), 422, "{}", resp.text());
    assert!(resp.text().contains("waypoint.actual_depart"),
            "the refusal must name the field that does work: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["status"], "dispatched", "a rejected TONU must not half-apply");
}

/// A TONU is an operational fact that must be recordable with ORS down.
/// `compute_and_persist_mileage` returns 409 whenever a route was expected and
/// none came back, so propagating it would make the verb unusable exactly when a
/// dispatcher needs it. A chained trip expects a deadhead leg, so this fixture
/// reaches that branch under the suite's empty `RoutingClient`.
#[tokio::test]
async fn test_tonu_records_the_outcome_when_routing_is_unavailable() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    let prev_fac = create_test_facility(&server, &token, "Prior Dock", "Gary, IN").await;
    let prev = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "stops": [{ "sequence": 0, "stop_type": "delivery", "facility_id": prev_fac,
                        "name": "Prior Dock", "scheduled_arrive": "2026-05-31T08:00:00",
                        "timezone": "America/Chicago" }]
        })).await;
    let prev_id = prev.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let fac_id = create_test_facility(&server, &token, "Chained Dock", "Chicago, IL").await;
    let driver_id = create_driver(&server, &token, "Chained Driver").await;
    let truck_id = create_truck(&server, &token, "T-CHAINED").await;
    let load = server.post("/fleet/api/v1/loads").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581491", "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")]
        })).await;
    let load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let trip = server.post("/fleet/api/v1/trips").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_id": load_id, "previous_trip_id": prev_id,
            "stops": [{ "sequence": 0, "stop_type": "pickup", "facility_id": fac_id,
                        "name": "Chained Dock", "scheduled_arrive": "2026-06-01T08:00:00",
                        "timezone": "America/Chicago" }]
        })).await;
    let trip_id = trip.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id })).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .authorization_bearer(&token).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" })).await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "reason": "released at the dock" })).await;
    assert_eq!(resp.status_code(), 200,
               "a routing failure must not block the outcome: {}", resp.text());

    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "tonu", "the outcome is recorded regardless");
    assert!(body["mileage_recompute_warning"].is_string(),
            "the caller must be told the miles are unknown, not left to assume zero: {body}");
    assert!(body["deadhead_miles"].is_null(),
            "an honest unknown beats the never-driven planned figure");
}

/// `delete` soft-cancels anything non-terminal, and `Tonu` has no edge to
/// `Cancelled`, so without an explicit arm a TONU'd trip fell through to
/// `cancel()` and failed with a transition error that names neither verb.
#[tokio::test]
async fn test_delete_refuses_a_tonu_trip() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581490").await;

    let tonu = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;
    assert_eq!(tonu.status_code(), 200, "tonu failed: {}", tonu.text());

    let resp = server.delete(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await;
    assert_eq!(resp.status_code(), 409);
    assert!(resp.text().contains("cannot delete trip with status 'tonu'"),
            "the refusal must name the status, not a bogus transition: {}", resp.text());
}

#[tokio::test]
async fn test_doctors_are_clean_on_a_tonu_load_and_trip() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver) = dispatched_trip(&server, &token, "4581488").await;

    let tonu = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Hold", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;
    // Both doctor checks below are also silent on a `dispatched` trip/load, so
    // without these two assertions a `tonu` that regressed to a 4xx would leave
    // the test green while proving nothing.
    assert_eq!(tonu.status_code(), 200, "tonu failed: {}", tonu.text());
    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["status"], "tonu", "the doctors must be run against a TONU'd trip");

    // The doctors are MCP-only tools with no REST route, so call the services
    // directly rather than going through the server.
    let load_uuid: uuid::Uuid = load_id.parse().unwrap();
    let report = ollie::services::doctors::load::run(&state, load_uuid, false).await.unwrap();
    assert!(
        !report.findings.iter().any(|f| f.check == "load.status_matches_trips"),
        "a tonu load is not stranded — it invoices from tonu: {:?}", report.findings,
    );

    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    let report = ollie::services::doctors::trip::run(&state, trip_uuid, false).await.unwrap();
    assert!(
        !report.findings.iter().any(|f| f.check == "trip.status.actuals_consistent"),
        "a TONU'd trip legitimately has stops with no actuals: {:?}", report.findings,
    );
}

/// The branch's headline invariant, with the positive coverage it was missing.
///
/// `assert!(trip["loaded_miles"].is_null())` after a `tonu` call is true under
/// this suite's `RoutingClient::new("")` whether or not the reassignment ran at
/// all — with no ORS, nothing ever sets `loaded_miles` to `Some`, so a swapped
/// pair of arguments at the `update_trip_mileage` call would ship green. Seeding
/// a routed figure and driving the reassignment step directly is what makes that
/// regression fail. The step is exercised on its own rather than through `tonu`
/// because `tonu` deliberately clears mileage before recomputing (a failed
/// recompute must leave an honest null), so a figure seeded ahead of the call is
/// gone before the reassignment is reached.
#[tokio::test]
async fn test_tonu_mileage_reassignment_puts_the_whole_figure_in_deadhead() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581510").await;
    let uuid: uuid::Uuid = trip_id.parse().unwrap();

    // Stand in for a successful routing pass over the truncated stop list, split
    // the way `compute_trip_mileage` would split it with no `previous_trip_id`:
    // almost everything called "loaded".
    state.db.update_trip_mileage(uuid, Some(12.0), Some(407.0), Some(419.0), vec![12.0, 407.0])
        .await.unwrap();

    let warning = ollie::services::trip_lifecycle::reassign_all_to_deadhead(&state, uuid).await;
    assert!(warning.is_none(), "reassignment reported a problem: {warning:?}");

    let trip = state.db.get_trip(uuid).await.unwrap();
    assert_eq!(trip.deadhead_miles, Some(419.0),
        "a TONU trip has zero loaded miles by construction: the whole routed \
         figure belongs in deadhead, got {:?}", trip.deadhead_miles);
    assert!(trip.loaded_miles.is_none(),
        "407 miles left in loaded_miles would be paid at the loaded rate: {:?}",
        trip.loaded_miles);
    assert_eq!(trip.total_miles, Some(419.0), "the total is unchanged by the split");
}

/// `recalculate_trip_miles` is the documented repair when ORS is down at TONU
/// time, and it used to reintroduce the exact split TONU exists to prevent: it
/// calls `compute_and_persist_mileage` directly, which applies the normal
/// previous-trip split, and `already_set` short-circuits only when BOTH fields
/// are set — which for a TONU trip is true of the *broken* state and false of
/// the correct one.
///
/// Seeded split miles stand in for that state (also reachable for real when
/// `tonu`'s own reassignment write fails after a successful route). The
/// recompute still 409s because the suite has no ORS — that is the endpoint's
/// existing contract — but the split must be corrected regardless.
#[tokio::test]
async fn test_recalculate_never_leaves_a_tonu_trip_with_split_miles() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581511").await;

    let arrive = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" }))
        .await;
    assert_eq!(arrive.status_code(), 200, "arrive failed: {}", arrive.text());

    let tonu = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Overflow Lot", "address": "Joliet, IL",
                          "timezone": "America/Chicago" }
        })).await;
    assert_eq!(tonu.status_code(), 200, "tonu failed: {}", tonu.text());

    let uuid: uuid::Uuid = trip_id.parse().unwrap();
    let trip = state.db.get_trip(uuid).await.unwrap();
    assert_eq!(trip.status, ollie::models::TripStatus::Tonu);
    assert_eq!(trip.stops.len(), 2, "reached dock + waypoint");

    // The state ORS-down TONU leaves behind, once someone has routed it: a split
    // that bills the empty run at the loaded rate.
    state.db.update_trip_mileage(uuid, Some(12.0), Some(407.0), Some(419.0), vec![12.0, 407.0])
        .await.unwrap();

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/recalculate-miles"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({})).await;
    assert_eq!(resp.status_code(), 409,
        "no ORS in this suite, so the recompute itself still fails: {}", resp.text());

    let trip = state.db.get_trip(uuid).await.unwrap();
    assert!(trip.loaded_miles.is_none(),
        "the documented repair path must not leave the empty run in loaded_miles: {:?}",
        trip.loaded_miles);
    assert_eq!(trip.deadhead_miles, Some(419.0),
        "the whole figure belongs in deadhead: {:?}", trip.deadhead_miles);
}

/// `stop_type` is a legitimate override on a diversion destination and has no
/// valid use on a TONU waypoint, where honouring it makes the parking spot a
/// service stop and earns the driver an extra-stop fee for parking.
#[tokio::test]
async fn test_tonu_ignores_a_stop_type_override_on_the_waypoint() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver_id) = dispatched_trip(&server, &token, "4581513").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/tonu"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "waypoint": { "facility_name": "Overflow Lot", "address": "Joliet, IL",
                          "timezone": "America/Chicago", "stop_type": "delivery" }
        })).await;
    assert_eq!(resp.status_code(), 200, "tonu failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 1, "no stop was reached, so the waypoint is the list");
    assert_eq!(stops[0]["stop_type"], "waypoint",
        "a waypoint is a waypoint whatever the caller asked for: {}", trip["stops"]);
}
