// tests/it/divert_test.rs
//
// Diversion: a load is cancelled, reconsigned, or corrected after the driver
// departs the shipper with freight aboard. The trip keeps running to a new
// destination; only the plan changes.
//
// The `waypoint` is mandatory because ORS routes waypoint to waypoint. A trip
// recomputed as stop0 -> new_destination draws the path you would have taken
// had you known at the dock, and every mile of backtracking disappears —
// silently, because the result still looks plausible.
//
// Integration tests run with RoutingClient::new(""), so ORS is always
// unavailable and mileage degrades to a warning. The backtrack guarantee is
// therefore asserted through stop *ordering* — which is what determines the
// route — never through a mile count.

use axum_test::TestServer;
use ollie::{ai::OllamaClient, api, config::Config, db::DbClient, storage::BlobStore, AppState};
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

/// A dispatched, single-stop trip — TONU territory, before any freight is aboard.
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

/// A trip that has departed its pickup — freight aboard, status `in_transit`.
/// Departure from the first `Pickup` is exactly what promotes the status, so
/// this is the boundary between TONU territory and diversion territory.
///
/// The trip needs TWO stops. `cascade_final_stop_delivered` fires when the
/// departed stop is the max-sequence stop, so on a single-stop trip the one
/// depart is also the final depart and the trip runs straight through
/// `in_transit` to `delivered` — which would leave every test below asserting
/// against the wrong entry state. The final assertion pins that shut.
async fn in_transit_trip(
    server: &TestServer, token: &str, load_number: &str,
) -> (String, String, String) {
    let shipper = create_test_facility(server, token, &format!("{load_number} Shipper"), "Chicago, IL").await;
    let consignee = create_test_facility(server, token, &format!("{load_number} Consignee"), "Denver, CO").await;
    let driver_id = create_driver(server, token, &format!("{load_number} Driver")).await;
    let truck_id = create_truck(server, token, &format!("T-{load_number}")).await;

    let resp = server.post("/fleet/api/v1/loads")
        .authorization_bearer(token)
        .json(&serde_json::json!({
            "load_number": load_number,
            "customer_name": "Landstar",
            "stops": [stop_json(&shipper, "2026-06-01T08:00:00")],
            "rate_items": [{ "description": "Line Haul", "amount_usd": 1800.0 }],
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let load_id = resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_resp = server.post("/fleet/api/v1/trips")
        .authorization_bearer(token)
        .json(&serde_json::json!({
            "load_id": load_id,
            "stops": [
                { "sequence": 0, "stop_type": "pickup", "facility_id": shipper,
                  "name": "Shipper", "scheduled_arrive": "2026-06-01T08:00:00",
                  "timezone": "America/Chicago" },
                { "sequence": 1, "stop_type": "delivery", "facility_id": consignee,
                  "name": "Consignee", "scheduled_arrive": "2026-06-02T08:00:00",
                  "timezone": "America/Denver" }
            ]
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

    let arrive = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/arrive"))
        .authorization_bearer(token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T08:00:00" }))
        .await;
    assert_eq!(arrive.status_code(), 200, "arrive failed: {}", arrive.text());

    let depart = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/0/depart"))
        .authorization_bearer(token)
        .json(&serde_json::json!({ "actual_depart": "2026-06-01T09:00:00" }))
        .await;
    assert_eq!(depart.status_code(), 200, "depart failed: {}", depart.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(token).await.json();
    assert_eq!(trip["status"], "in_transit",
        "departing the pickup starts transit — a fixture that lands anywhere else \
         is not testing diversion at all");

    (load_id, trip_id, driver_id)
}

#[tokio::test]
async fn test_divert_inserts_the_waypoint_between_history_and_the_new_destination() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581490").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "notes": "consignee refused; returning to shipper",
            "waypoint": {
                "facility_name": "Salina Truck Stop", "address": "Salina, KS",
                "timezone": "America/Chicago",
                "actual_arrive": "2026-06-01T14:00:00",
                "actual_depart": "2026-06-01T17:00:00"
            },
            "stops": [{
                "stop_type": "delivery",
                "facility_name": "Return Dock", "address": "Kansas City, MO",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 3, "kept pickup, waypoint, new delivery");
    assert_eq!(stops[0]["stop_type"], "pickup", "departed history is immutable");
    assert_eq!(stops[0]["name"], "Shipper");
    assert_eq!(stops[1]["stop_type"], "waypoint");
    assert_eq!(stops[1]["name"], "Salina Truck Stop");
    assert_eq!(stops[2]["stop_type"], "delivery");
    assert_eq!(stops[2]["name"], "Return Dock");
    // The unreached original consignee is gone: it never happened and would
    // otherwise anchor every downstream deadhead to a city the truck never saw.
    assert!(!stops.iter().any(|s| s["name"] == "Consignee"),
            "the unreached consignee must not survive the re-target: {stops:?}");
    // The ordering IS the backtrack fix: routing walks pickup -> Salina ->
    // Kansas City, so the 200 miles out and 200 back are both counted. Routing
    // itself cannot be asserted here (tests run without ORS).
    assert_eq!(trip["status"], "in_transit", "the trip keeps running");

    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert!(load["diverted_at"].is_string());
    assert_eq!(load["diversion_reason"], "diverted");
    assert_eq!(load["diversion_notes"], "consignee refused; returning to shipper");
    assert_eq!(load["rate_items"].as_array().unwrap().len(), 1,
               "unlike TONU, the line haul is at least partly earned");
}

#[tokio::test]
async fn test_bol_correction_does_not_flag_the_load_as_diverted() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581491").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "bol_correction",
            "waypoint": {
                "facility_name": "Divergence Point", "address": "Springfield, IL",
                "timezone": "America/Chicago"
            },
            "stops": [{
                "stop_type": "delivery",
                "facility_name": "BOL Consignee", "address": "St Louis, MO",
                "timezone": "America/Chicago"
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    // The plan still changed — only the commercial flag is withheld.
    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["stops"].as_array().unwrap().len(), 3);
    assert_eq!(trip["stops"][2]["name"], "BOL Consignee");

    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert!(load["diverted_at"].is_null(),
        "nothing was diverted — the plan was wrong from the start, and flagging it \
         would poison the query the field exists to answer");
    assert!(load["diversion_reason"].is_null());
}

#[tokio::test]
async fn test_divert_requires_a_waypoint() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581492").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "reason": "reconsigned", "stops": [] }))
        .await;
    assert_eq!(resp.status_code(), 422,
        "without the divergence point the recomputed route erases the backtrack: {}",
        resp.text());
    // `waypoint` is only conditionally required, so serde can no longer enforce it
    // and `divert` does. Pin the explanation, not just the status.
    assert!(resp.text().contains("waypoint"), "{}", resp.text());
    assert!(resp.text().contains("waypoint to waypoint"),
            "the refusal must say why, or it reads as bureaucracy: {}", resp.text());

    // Rejected before the first write, so the plan is untouched and retryable.
    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["stops"].as_array().unwrap().len(), 2);
    assert_eq!(trip["status"], "in_transit");
}

#[tokio::test]
async fn test_departing_a_waypoint_does_not_deliver_the_trip() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581493").await;

    // Pulled over, disposition unknown: no destination yet.
    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": {
                "facility_name": "Holding Yard", "address": "Topeka, KS",
                "timezone": "America/Chicago",
                "actual_arrive": "2026-06-01T14:00:00"
            },
            "stops": []
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["stops"].as_array().unwrap().len(), 2, "kept pickup + waypoint");
    assert_eq!(trip["stops"][1]["stop_type"], "waypoint");

    // The waypoint is now the highest-sequence stop. Departing it must not fire
    // the delivered cascade — no freight came off the truck.
    let depart = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_depart": "2026-06-01T18:00:00" }))
        .await;
    assert_eq!(depart.status_code(), 200, "depart failed: {}", depart.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["status"], "in_transit", "a waypoint cannot deliver a load");
    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert_ne!(load["status"], "delivered");
}

/// `PositionInput` is shared between the `waypoint` field and the `stops` array,
/// where `stop_type` is a legitimate override — a cross-dock hand-off is a
/// `relay`, not a `delivery`. On a waypoint it has no valid use and one very bad
/// one: `cascade_final_stop_delivered` keys its guard on `stop_type == Waypoint`,
/// so a waypoint typed `delivery` on a `stops: []` divert becomes the
/// max-sequence stop, and departing it marks the trip Delivered and cascades the
/// load to Delivered — for freight that never arrived anywhere. The override is
/// dropped at the waypoint call site rather than honoured.
#[tokio::test]
async fn test_divert_ignores_a_stop_type_override_on_the_waypoint() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581512").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": {
                "facility_name": "Holding Yard", "address": "Topeka, KS",
                "timezone": "America/Chicago",
                "actual_arrive": "2026-06-01T14:00:00",
                "stop_type": "delivery"
            },
            "stops": []
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["stops"][1]["stop_type"], "waypoint",
        "a waypoint is a waypoint whatever the caller asked for: {}", trip["stops"]);

    // And the consequence the type carries: departing the hold delivers nothing.
    let depart = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/1/depart"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_depart": "2026-06-01T18:00:00" }))
        .await;
    assert_eq!(depart.status_code(), 200, "depart failed: {}", depart.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["status"], "in_transit",
        "the freight is still on the truck; nothing was delivered");
    let load: serde_json::Value = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(load["status"], "in_transit",
        "billing must not open on freight that never arrived anywhere");
}

/// The hold-then-append flow. A `stops: []` divert leaves the trip ending at a
/// waypoint the driver reached — which already satisfies the rule the mandatory
/// waypoint exists to enforce (the list ends where the truck actually is), so
/// naming the destination later needs no second divergence point. Without this,
/// "pulled over, disposition unknown" is a dead end: every stop is history and
/// the follow-up 422s.
#[tokio::test]
async fn test_a_hold_can_be_given_a_destination_without_a_second_waypoint() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581499").await;

    let hold = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": { "facility_name": "Holding Yard", "address": "Topeka, KS",
                          "timezone": "America/Chicago",
                          "actual_arrive": "2026-06-01T14:00:00" },
            "stops": []
        })).await;
    assert_eq!(hold.status_code(), 200, "hold divert failed: {}", hold.text());

    // Hours later the broker names a consignee. The truck has not moved.
    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "stops": [{ "facility_name": "New Consignee", "address": "Wichita, KS",
                        "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(resp.status_code(), 200,
        "the trip already ends where the truck is, so no new divergence point \
         exists to name: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 3, "pickup, the hold, the new destination");
    assert_eq!(stops[1]["name"], "Holding Yard", "the hold is history, not a stop to replace");
    assert_eq!(stops[1]["stop_type"], "waypoint");
    assert_eq!(stops[2]["name"], "New Consignee");
    assert_eq!(stops[2]["stop_type"], "delivery");
    assert_eq!(trip["status"], "in_transit");
}

/// The other half: the truck moved again while it was held, so there IS a fresh
/// divergence point and supplying it must append rather than replace the hold.
#[tokio::test]
async fn test_a_hold_that_moved_takes_a_second_waypoint_and_keeps_the_first() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581500").await;

    let hold = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": { "facility_name": "First Hold", "address": "Topeka, KS",
                          "timezone": "America/Chicago",
                          "actual_arrive": "2026-06-01T14:00:00",
                          "actual_depart": "2026-06-01T18:00:00" },
            "stops": []
        })).await;
    assert_eq!(hold.status_code(), 200, "hold divert failed: {}", hold.text());

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "waypoint": { "facility_name": "Second Hold", "address": "Salina, KS",
                          "timezone": "America/Chicago",
                          "actual_arrive": "2026-06-01T21:00:00" },
            "stops": [{ "facility_name": "New Consignee", "address": "Wichita, KS",
                        "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(resp.status_code(), 200, "second divert failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 4, "pickup, both holds, the new destination");
    assert_eq!(stops[1]["name"], "First Hold",
        "the first hold is where the truck sat for four hours — dropping it would \
         erase the miles from there to the second one");
    assert_eq!(stops[2]["name"], "Second Hold");
    assert_eq!(stops[2]["stop_type"], "waypoint");
    assert_eq!(stops[3]["name"], "New Consignee");
}

#[tokio::test]
async fn test_divert_refuses_to_rewrite_an_arrived_at_stop() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581494").await;

    // First divert gives the trip a delivery stop, then the driver arrives at it.
    let first = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "waypoint": { "facility_name": "Point A", "address": "Salina, KS",
                          "timezone": "America/Chicago" },
            "stops": [{ "stop_type": "delivery", "facility_name": "Consignee B",
                        "address": "Wichita, KS", "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(first.status_code(), 200, "first divert failed: {}", first.text());
    let arrive = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/2/arrive"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "actual_arrive": "2026-06-01T20:00:00" })).await;
    assert_eq!(arrive.status_code(), 200, "arrive failed: {}", arrive.text());

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "waypoint": { "facility_name": "Point C", "address": "Emporia, KS",
                          "timezone": "America/Chicago" },
            "stops": []
        })).await;
    assert_eq!(resp.status_code(), 422,
        "a stop the driver has already reached is history, not plan: {}", resp.text());

    // Refused before the first write: Point C never landed on the trip.
    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["stops"].as_array().unwrap().len(), 3);
    assert!(!trip["stops"].as_array().unwrap().iter().any(|s| s["name"] == "Point C"));
}

#[tokio::test]
async fn test_divert_rejects_a_dispatched_trip_and_names_tonu() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = dispatched_trip(&server, &token, "4581495").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": { "facility_name": "X", "address": "Joliet, IL",
                          "timezone": "America/Chicago" },
            "stops": []
        })).await;
    assert_eq!(resp.status_code(), 409);
    assert!(resp.text().contains("tonu_trip"),
            "no freight is aboard yet: {}", resp.text());
}

/// `divert` feeds several positions through `resolve_position`, so a rejection
/// has to name the one the caller actually got wrong. Every message used to say
/// "waypoint", which sent a dispatcher to a field that was already correct.
#[tokio::test]
async fn test_an_unresolvable_destination_names_the_field_it_came_from() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581504").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "waypoint": { "facility_name": "Divergence", "address": "Salina, KS",
                          "timezone": "America/Chicago" },
            "stops": [
                { "facility_name": "Fine Dock", "address": "Topeka, KS",
                  "timezone": "America/Chicago" },
                // Second destination: named, but with no address to geocode.
                { "facility_name": "Nameless Dock", "timezone": "America/Chicago" }
            ]
        })).await;
    assert_eq!(resp.status_code(), 422, "{}", resp.text());
    assert!(resp.text().contains("stops[1]"),
        "the waypoint was fine — pointing at it would send the dispatcher to the \
         wrong field: {}", resp.text());

    // Resolution runs before the first write, so nothing half-applied.
    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["stops"].as_array().unwrap().len(), 2);
    assert!(!trip["stops"].as_array().unwrap().iter().any(|s| s["name"] == "Fine Dock"),
            "the destination that did resolve must not survive the rejection");
}

/// A cross-dock hand-off is a `relay`, not a `delivery`. The per-position
/// `stop_type` override is the only way to say so, and it is advertised on the
/// `divert_trip` MCP schema for exactly this case.
#[tokio::test]
async fn test_divert_destination_stop_type_can_be_overridden() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581496").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "waypoint": { "facility_name": "Divergence", "address": "Salina, KS",
                          "timezone": "America/Chicago" },
            "stops": [{ "stop_type": "relay", "facility_name": "Cross Dock",
                        "address": "Kansas City, MO", "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["stops"][2]["stop_type"], "relay",
               "a cross-dock hand-off must not be typed as a delivery");
}

/// `last_reached` is derived by `sequence`, but the kept prefix is then
/// renumbered by vector position. Nothing in the codebase sorts trip stops —
/// `src/api/trips.rs` stores whatever order the caller supplied — so a trip
/// whose stops arrived out of order would have its history silently reversed,
/// and the reversed history is what routing then walks.
#[tokio::test]
async fn test_divert_keeps_history_in_sequence_order_when_stops_were_stored_out_of_order() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    let a = create_test_facility(&server, &token, "Shipper A", "Chicago, IL").await;
    let b = create_test_facility(&server, &token, "Shipper B", "Joliet, IL").await;
    let c = create_test_facility(&server, &token, "Consignee C", "Denver, CO").await;
    let driver_id = create_driver(&server, &token, "Unsorted Driver").await;
    let truck_id = create_truck(&server, &token, "T-UNSORTED").await;

    let load = server.post("/fleet/api/v1/loads").authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581498", "customer_name": "Landstar",
            "stops": [stop_json(&a, "2026-06-01T08:00:00")],
        })).await;
    assert_eq!(load.status_code(), 201, "load create failed: {}", load.text());
    let load_id = load.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Supplied middle-first: the vector order and the sequence order disagree.
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
    for (seq, arrive, depart) in [
        (0, "2026-06-01T08:00:00", "2026-06-01T09:00:00"),
        (1, "2026-06-01T12:00:00", "2026-06-01T13:00:00"),
    ] {
        let r = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/{seq}/arrive"))
            .authorization_bearer(&token)
            .json(&serde_json::json!({ "actual_arrive": arrive })).await;
        assert_eq!(r.status_code(), 200, "arrive {seq} failed: {}", r.text());
        let r = server.post(&format!("/fleet/api/v1/trips/{trip_id}/stops/{seq}/depart"))
            .authorization_bearer(&token)
            .json(&serde_json::json!({ "actual_depart": depart })).await;
        assert_eq!(r.status_code(), 200, "depart {seq} failed: {}", r.text());
    }

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    assert_eq!(trip["status"], "in_transit", "fixture must be carrying freight");

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "waypoint": { "facility_name": "Divergence", "address": "Salina, KS",
                          "timezone": "America/Chicago" },
            "stops": [{ "facility_name": "Return Dock", "address": "Kansas City, MO",
                        "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    let stops = trip["stops"].as_array().unwrap();
    assert_eq!(stops.len(), 4, "two reached pickups, waypoint, new delivery");
    assert_eq!(stops[0]["name"], "Shipper A",
        "the truck visited A then B; renumbering by vector position would swap them \
         and route the whole trip backwards through its own history");
    assert_eq!(stops[1]["name"], "Shipper B");
    assert_eq!(stops[2]["stop_type"], "waypoint");
    assert_eq!(stops[3]["name"], "Return Dock");
}

/// Mileage feeds `compute_driver_pay`. The pre-divert figure measures a route to
/// a consignee the trip is no longer going to, and it is not self-correcting:
/// `recalculate_trip_miles` short-circuits when deadhead and loaded are both
/// already set unless the caller passes `force`, and miles are not hand-settable
/// — so a stale figure that survives an ORS outage here survives every later
/// attempt to fix it, and gets paid out.
#[tokio::test]
async fn test_divert_clears_superseded_miles_when_routing_is_unavailable() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581501").await;
    let uuid: uuid::Uuid = trip_id.parse().unwrap();

    // Stand in for a successful pre-divert routing pass (the suite has no ORS).
    state.db.update_trip_mileage(uuid, Some(50.0), Some(1200.0), Some(1250.0), vec![50.0, 1200.0])
        .await.unwrap();

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": { "facility_name": "Turnaround", "address": "Salina, KS",
                          "timezone": "America/Chicago" },
            "stops": [{ "facility_name": "Return Dock", "address": "Kansas City, MO",
                        "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    let trip = state.db.get_trip(uuid).await.unwrap();
    assert!(trip.loaded_miles.is_none(),
        "1200 loaded miles to Denver must not survive a divert to Kansas City — \
         recalculate short-circuits on an already-set pair, so this figure would \
         be permanent and would be paid: {:?}", trip.loaded_miles);
    assert!(trip.deadhead_miles.is_none(), "{:?}", trip.deadhead_miles);
    assert!(trip.total_miles.is_none(), "{:?}", trip.total_miles);
    assert!(trip.segment_miles.is_empty(), "{:?}", trip.segment_miles);

    let body: serde_json::Value = resp.json();
    assert!(body["mileage_recompute_warning"].is_string(),
            "an honest null still has to be announced, not left to be read as zero: {body}");
}

/// A settled trip's miles and pay are frozen, and a divert would recompute both.
#[tokio::test]
async fn test_divert_refuses_a_settled_trip() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581502").await;

    let settle = server.patch(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "settlement_ref": "SETTLE-4581502" })).await;
    assert_eq!(settle.status_code(), 200, "settle failed: {}", settle.text());

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": { "facility_name": "Too Late", "address": "Salina, KS",
                          "timezone": "America/Chicago" },
            "stops": []
        })).await;
    assert_eq!(resp.status_code(), 409, "{}", resp.text());
    assert!(resp.text().contains("settled"), "the refusal must name why: {}", resp.text());

    let trip: serde_json::Value = server.get(&format!("/fleet/api/v1/trips/{trip_id}"))
        .authorization_bearer(&token).await.json();
    // Not a length check: the request carries `stops: []`, so a divert that
    // *succeeded* would leave [kept pickup, waypoint] — also two stops. The names
    // are what a successful write would actually have changed.
    let names: Vec<&str> = trip["stops"].as_array().unwrap().iter()
        .map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Shipper", "Consignee"], "nothing was written");
}

/// The event journal is how a diversion is reconstructed after the fact — the
/// load's `diverted_at` says a diversion happened, the event says what changed.
#[tokio::test]
async fn test_divert_appends_a_trip_diverted_event() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581503").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "reconsigned",
            "notes": "broker renominated to Wichita",
            "waypoint": { "facility_name": "Divergence", "address": "Salina, KS",
                          "timezone": "America/Chicago" },
            "stops": [{ "facility_name": "New Consignee", "address": "Wichita, KS",
                        "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(resp.status_code(), 200, "divert failed: {}", resp.text());

    let events: serde_json::Value = server
        .get(&format!("/fleet/api/v1/events?trip_id={trip_id}&limit=100"))
        .authorization_bearer(&token).await.json();
    let items = events["items"].as_array()
        .unwrap_or_else(|| panic!("unexpected events shape: {events}"));
    let diverted = items.iter().find(|e| e["event_type"] == "trip.diverted")
        .unwrap_or_else(|| panic!("no trip.diverted event: {events}"));
    assert_eq!(diverted["payload"]["reason"], "reconsigned");
    assert_eq!(diverted["payload"]["notes"], "broker renominated to Wichita");
    assert_eq!(diverted["payload"]["new_stop_count"], 3);
}

/// A diversion is an operational fact that must be recordable with ORS down.
/// The suite runs with an empty `RoutingClient`, so this is the only path
/// available — a propagated routing error would make the verb unusable exactly
/// when a dispatcher needs it.
#[tokio::test]
async fn test_divert_records_the_outcome_when_routing_is_unavailable() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let (_load_id, trip_id, _driver) = in_transit_trip(&server, &token, "4581497").await;

    let resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/divert"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "reason": "diverted",
            "waypoint": { "facility_name": "Hold", "address": "Salina, KS",
                          "timezone": "America/Chicago" },
            "stops": [{ "facility_name": "Return Dock", "address": "Kansas City, MO",
                        "timezone": "America/Chicago" }]
        })).await;
    assert_eq!(resp.status_code(), 200,
               "a routing failure must not block the outcome: {}", resp.text());

    let body: serde_json::Value = resp.json();
    assert!(body["mileage_recompute_warning"].is_string(),
            "the caller must be told the miles are stale, not left to assume: {body}");
    // The default for a diversion destination is `delivery`.
    assert_eq!(body["stops"][2]["stop_type"], "delivery");
}
