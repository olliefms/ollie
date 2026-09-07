// tests/it/assign_chain_test.rs
//
// #437 — the auto-dispatch chain link at assign time.
//
// Since #433 auto-dispatch is chain-only: on completion the successor is the
// Assigned trip whose `previous_trip_id` names the trip that just delivered, and
// nothing else. But that field was only ever written at trip creation (and only
// when the create payload named a driver) or through `update_trip_metadata`.
// Plan-then-assign — create the trip when the rate con arrives, pick the driver
// later — therefore never produced one, so those trips silently stopped
// auto-dispatching with nothing in the UI to explain it or fix it.
//
// `assign()` now settles the link. Integration tests run with
// `RoutingClient::new("")`, so ORS is always unavailable and the mileage
// recompute that a link change triggers always fails; that is deliberate here —
// it proves the link is still committed when routing is down.

use axum_test::TestServer;
use ollie::{
    ai::OllamaClient, api, config::Config, db::DbClient, storage::BlobStore,
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


// ── helpers ──────────────────────────────────────────────────────────────────

fn two_stop_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "stops": [
            { "sequence": 1, "stop_type": "pickup", "name": format!("{name}-Origin"),
              "timezone": "America/Los_Angeles" },
            { "sequence": 2, "stop_type": "delivery", "name": format!("{name}-Dest"),
              "timezone": "America/Los_Angeles" }
        ]
    })
}

/// Create a trip with NO driver — the plan-then-assign shape this issue is about.
async fn create_unassigned_trip(server: &TestServer, token: &str, name: &str) -> String {
    let resp = server.post("/fleet/api/v1/trips")
        .authorization_bearer(token)
        .json(&two_stop_body(name))
        .await;
    assert_eq!(resp.status_code(), 201, "create {name} failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

/// POST /assign. `body_extra` is merged in so a test can add `previous_trip_id`
/// (or deliberately omit it) exactly as the fleet UI would.
async fn assign_trip(
    server: &TestServer, token: &str, trip_id: &str,
    driver_id: &str, truck_id: &str, body_extra: serde_json::Value,
) -> axum_test::TestResponse {
    let mut body = serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id });
    if let Some(map) = body_extra.as_object() {
        for (k, v) in map { body[k] = v.clone(); }
    }
    server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .authorization_bearer(token)
        .json(&body)
        .await
}

async fn previous_trip_id(state: &AppState, trip_id: &str) -> Option<String> {
    state.db.get_trip(trip_id.parse().unwrap()).await.unwrap()
        .previous_trip_id.map(|u| u.to_string())
}

async fn set_status(state: &AppState, trip_id: &str, status: ollie::models::TripStatus) {
    state.db.transition_trip_status(trip_id.parse().unwrap(), status).await.unwrap();
}

// ── the gap this closes ──────────────────────────────────────────────────────

/// The whole point: a trip created with no driver, assigned later, now gets a
/// chain link. Before this it stayed `None` forever and auto-dispatch never fired.
#[tokio::test]
async fn test_plan_then_assign_derives_the_chain_link() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Chain Driver").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN-1").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assert_eq!(assign_trip(&server, &token, &trip_a, &driver_id, &truck_id,
        serde_json::json!({})).await.status_code(), 200);

    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assert_eq!(assign_trip(&server, &token, &trip_b, &driver_id, &truck_id,
        serde_json::json!({})).await.status_code(), 200);

    assert_eq!(previous_trip_id(&state, &trip_b).await, Some(trip_a.clone()),
        "assigning a second trip to the driver must chain it behind their current work");
}

/// The link must survive a failed mileage recompute. ORS is unavailable in these
/// tests, so this asserts the best-effort contract rather than a happy path.
#[tokio::test]
async fn test_chain_link_is_committed_even_though_routing_is_down() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Routing Down").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN-2").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({})).await;
    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    let res = assign_trip(&server, &token, &trip_b, &driver_id, &truck_id,
        serde_json::json!({})).await;

    assert_eq!(res.status_code(), 200, "a routing failure must not fail the assignment");
    assert_eq!(previous_trip_id(&state, &trip_b).await, Some(trip_a),
        "the link is valuable on its own and must be committed regardless");
}

// ── the three states of previous_trip_id ─────────────────────────────────────

/// Explicit `null` means "this starts a new chain" and must beat the derivation.
/// Without this state a dispatcher could not say "no chain" at all.
#[tokio::test]
async fn test_explicit_null_leaves_the_trip_unchained() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "No Chain").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN-3").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({})).await;
    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assign_trip(&server, &token, &trip_b, &driver_id, &truck_id,
        serde_json::json!({ "previous_trip_id": null })).await;

    assert_eq!(previous_trip_id(&state, &trip_b).await, None,
        "an explicit null must not be overridden by the derivation");
}

/// An explicit id pins that predecessor even when the derivation would pick
/// another — the dispatcher's choice wins.
#[tokio::test]
async fn test_explicit_id_beats_the_derivation() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Explicit").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN-4").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({})).await;
    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assign_trip(&server, &token, &trip_b, &driver_id, &truck_id, serde_json::json!({})).await;
    // C would derive to B (the most recent). Pin it to A instead.
    let trip_c = create_unassigned_trip(&server, &token, "C").await;
    assign_trip(&server, &token, &trip_c, &driver_id, &truck_id,
        serde_json::json!({ "previous_trip_id": trip_a })).await;

    assert_eq!(previous_trip_id(&state, &trip_c).await, Some(trip_a),
        "an explicitly chosen predecessor must win over the derived one");
    assert_ne!(previous_trip_id(&state, &trip_c).await, Some(trip_b),
        "the derivation must not overwrite an explicit choice");
}

/// A link already on the record is a stated plan. Re-assigning must not silently
/// repoint it — that would also move the deadhead origin and change driver pay.
#[tokio::test]
async fn test_an_existing_link_is_not_repointed() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Sticky").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN-5").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({})).await;
    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assign_trip(&server, &token, &trip_b, &driver_id, &truck_id, serde_json::json!({})).await;

    // C is pinned to A, then unassigned and re-assigned with no stated preference.
    let trip_c = create_unassigned_trip(&server, &token, "C").await;
    assign_trip(&server, &token, &trip_c, &driver_id, &truck_id,
        serde_json::json!({ "previous_trip_id": trip_a })).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_c}/unassign"))
        .authorization_bearer(&token).await;
    assign_trip(&server, &token, &trip_c, &driver_id, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_c).await, Some(trip_a),
        "a link already on the record must survive a re-assign untouched");
}

// ── which predecessor the derivation picks ───────────────────────────────────

/// A terminal predecessor is useless for dispatch: its completion already fired,
/// so a successor chained to it would never auto-dispatch. Picking one would be a
/// fresh silent stall.
#[tokio::test]
async fn test_derivation_skips_terminal_trips() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Terminal").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN-6").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({})).await;
    set_status(&state, &trip_a, ollie::models::TripStatus::Cancelled).await;

    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assign_trip(&server, &token, &trip_b, &driver_id, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_b).await, None,
        "a finished trip must not be chained behind — its completion already fired");
}

/// A `planned` trip may never run, and `predecessor_blocking_dispatch` hard-blocks
/// the auto path on a non-terminal predecessor, so chaining onto one strands the
/// successor indefinitely.
#[tokio::test]
async fn test_derivation_skips_planned_trips() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Planned").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN-7").await;

    // A planned trip that names the driver but was never assigned.
    let planned = server.post("/fleet/api/v1/trips")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "driver_id": driver_id,
            "stops": two_stop_body("P")["stops"],
        }))
        .await;
    assert_eq!(planned.status_code(), 201, "create planned failed: {}", planned.text());
    let planned_id = planned.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    assert_eq!(
        state.db.get_trip(planned_id.parse().unwrap()).await.unwrap().status,
        ollie::models::TripStatus::Planned,
        "fixture must actually be Planned for this test to mean anything");

    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assign_trip(&server, &token, &trip_b, &driver_id, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_b).await, None,
        "a planned trip may never run; chaining onto it would strand the successor");
}

/// Another driver's work is not this driver's chain.
#[tokio::test]
async fn test_derivation_is_scoped_to_the_driver() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_a = create_driver(&server, &token, "Driver A").await;
    let driver_b = create_driver(&server, &token, "Driver B").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN-8").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_a, &truck_id, serde_json::json!({})).await;

    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assign_trip(&server, &token, &trip_b, &driver_b, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_b).await, None,
        "a chain must not cross drivers");
}

/// The trip being assigned must never become its own predecessor.
#[tokio::test]
async fn test_a_trip_is_never_its_own_predecessor() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Solo").await;
    let truck_id = create_truck(&server, &token, "T-CHAIN-9").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_a).await, None,
        "the driver's only trip is the one being assigned; it cannot follow itself");
}
