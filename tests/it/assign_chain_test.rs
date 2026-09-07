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
        "an explicitly chosen predecessor must win over the derived one (which would be {trip_b})");
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

// ── chain shape: walk to the tail, never fork ────────────────────────────────

/// The bug a "most recently created" derivation would cause. Trips are created
/// B, C, A — A last, as when a hot load is booked today while next week's legs
/// were planned last week. Picking the newest chainable trip would point BOTH B
/// and C at A, and `try_auto_dispatch_next_for_driver` refuses to resolve two
/// candidates, so nothing would ever roll.
#[tokio::test]
async fn test_derivation_walks_to_the_chain_tail_not_the_newest_trip() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Tail Walker").await;
    let truck_id = create_truck(&server, &token, "T-TAIL-1").await;

    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    let trip_c = create_unassigned_trip(&server, &token, "C").await;
    let trip_a = create_unassigned_trip(&server, &token, "A").await;

    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({})).await;
    assign_trip(&server, &token, &trip_b, &driver_id, &truck_id, serde_json::json!({})).await;
    assign_trip(&server, &token, &trip_c, &driver_id, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_b).await, Some(trip_a.clone()),
        "B follows A, the only trip in the chain when B was assigned");
    assert_eq!(previous_trip_id(&state, &trip_c).await, Some(trip_b),
        "C must follow the TAIL of the chain, not the most recently created trip");
    assert_ne!(previous_trip_id(&state, &trip_c).await, Some(trip_a),
        "two successors on one predecessor is the ambiguity that dispatches nothing");
}

/// Two tails means parallel chains and no right answer. #433 established that a
/// wrong chain is worse than none, so the derivation declines.
#[tokio::test]
async fn test_derivation_declines_when_the_driver_has_parallel_chains() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Forked").await;
    let truck_id = create_truck(&server, &token, "T-TAIL-2").await;

    // Two unchained assigned trips = two tails.
    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id,
        serde_json::json!({ "previous_trip_id": null })).await;
    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assign_trip(&server, &token, &trip_b, &driver_id, &truck_id,
        serde_json::json!({ "previous_trip_id": null })).await;

    let trip_c = create_unassigned_trip(&server, &token, "C").await;
    assign_trip(&server, &token, &trip_c, &driver_id, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_c).await, None,
        "with two possible tails there is no right answer; guessing is what #433 removed");
}

/// Re-assigning a trip that has been DISPATCHED but has not started rolling
/// succeeds today, because `assign` has no status precondition and reuses the
/// `Dispatched -> Assigned` edge that exists for `undispatch`. It must not
/// re-derive: by then the trip's own successor is the only chainable candidate,
/// and chaining to it builds a 2-cycle that blocks both trips from ever
/// dispatching. See `test_an_in_transit_trip_cannot_be_reassigned` for the
/// boundary — a trip actually in progress is rejected outright.
#[tokio::test]
async fn test_reassigning_a_dispatched_trip_does_not_chain_it_to_its_own_successor() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Swapper").await;
    let truck_id = create_truck(&server, &token, "T-SWAP-1").await;
    let truck_two = create_truck(&server, &token, "T-SWAP-2").await;

    let trip_p = create_unassigned_trip(&server, &token, "P").await;
    assign_trip(&server, &token, &trip_p, &driver_id, &truck_id,
        serde_json::json!({ "previous_trip_id": null })).await;
    set_status(&state, &trip_p, ollie::models::TripStatus::Dispatched).await;

    let trip_q = create_unassigned_trip(&server, &token, "Q").await;
    assign_trip(&server, &token, &trip_q, &driver_id, &truck_id, serde_json::json!({})).await;
    assert_eq!(previous_trip_id(&state, &trip_q).await, Some(trip_p.clone()),
        "fixture: Q must chain behind P or this test proves nothing");

    // Second assignment on the released-but-not-rolling trip.
    let res = assign_trip(&server, &token, &trip_p, &driver_id, &truck_two,
        serde_json::json!({})).await;
    assert_eq!(res.status_code(), 200, "re-assign failed: {}", res.text());

    assert_eq!(previous_trip_id(&state, &trip_p).await, None,
        "a re-assign must not point a running trip at its own successor");
    assert_ne!(previous_trip_id(&state, &trip_p).await, Some(trip_q),
        "P -> Q -> P is a cycle that blocks both trips from ever dispatching");
}

// ── validation of a caller-supplied link ─────────────────────────────────────

/// A dangling id is worse than none: `predecessor_blocking_dispatch` treats an
/// unreadable predecessor as a hard block, stranding the trip permanently.
#[tokio::test]
async fn test_pinning_a_nonexistent_predecessor_is_rejected() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Dangling").await;
    let truck_id = create_truck(&server, &token, "T-VAL-1").await;

    let trip = create_unassigned_trip(&server, &token, "A").await;
    let res = assign_trip(&server, &token, &trip, &driver_id, &truck_id, serde_json::json!({
        "previous_trip_id": "00000000-0000-0000-0000-000000000000"
    })).await;

    assert_eq!(res.status_code(), 422, "a dangling chain link must be rejected: {}", res.text());
}

/// Auto-dispatch only ever looks at one driver's trips, so a cross-driver link
/// can never fire; it would only block this trip.
#[tokio::test]
async fn test_pinning_another_drivers_trip_is_rejected() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_a = create_driver(&server, &token, "Driver A").await;
    let driver_b = create_driver(&server, &token, "Driver B").await;
    let truck_id = create_truck(&server, &token, "T-VAL-2").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_a, &truck_id, serde_json::json!({})).await;

    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    let res = assign_trip(&server, &token, &trip_b, &driver_b, &truck_id, serde_json::json!({
        "previous_trip_id": trip_a
    })).await;

    assert_eq!(res.status_code(), 422, "a cross-driver chain link must be rejected: {}", res.text());
}

#[tokio::test]
async fn test_a_trip_cannot_be_pinned_to_itself() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Self").await;
    let truck_id = create_truck(&server, &token, "T-VAL-3").await;

    let trip = create_unassigned_trip(&server, &token, "A").await;
    let res = assign_trip(&server, &token, &trip, &driver_id, &truck_id, serde_json::json!({
        "previous_trip_id": trip
    })).await;

    assert_eq!(res.status_code(), 422, "a trip cannot follow itself: {}", res.text());
}

/// Pinning a trip that already follows this one closes a cycle.
#[tokio::test]
async fn test_pinning_a_successor_is_rejected_as_a_cycle() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Cycle").await;
    let truck_id = create_truck(&server, &token, "T-VAL-4").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id,
        serde_json::json!({ "previous_trip_id": null })).await;
    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assign_trip(&server, &token, &trip_b, &driver_id, &truck_id, serde_json::json!({})).await;
    assert_eq!(previous_trip_id(&state, &trip_b).await, Some(trip_a.clone()),
        "fixture: B must follow A");

    // Now try to make A follow B.
    server.post(&format!("/fleet/api/v1/trips/{trip_a}/unassign"))
        .authorization_bearer(&token).await;
    let res = assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({
        "previous_trip_id": trip_b
    })).await;

    assert_eq!(res.status_code(), 422, "A -> B -> A must be rejected: {}", res.text());
}

/// A link pointing at another driver's trip is stale, not a plan: it can never
/// fire and blocks this trip. Re-deriving beats preserving it.
#[tokio::test]
async fn test_a_stale_cross_driver_link_is_re_derived() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_a = create_driver(&server, &token, "Driver A").await;
    let driver_b = create_driver(&server, &token, "Driver B").await;
    let truck_id = create_truck(&server, &token, "T-STALE-1").await;

    // A1 belongs to driver A; X is chained behind it.
    let trip_a1 = create_unassigned_trip(&server, &token, "A1").await;
    assign_trip(&server, &token, &trip_a1, &driver_a, &truck_id, serde_json::json!({})).await;
    let trip_x = create_unassigned_trip(&server, &token, "X").await;
    assign_trip(&server, &token, &trip_x, &driver_a, &truck_id, serde_json::json!({})).await;
    assert_eq!(previous_trip_id(&state, &trip_x).await, Some(trip_a1.clone()));

    // Driver B picks up their own work, then X is handed to driver B.
    let trip_b1 = create_unassigned_trip(&server, &token, "B1").await;
    assign_trip(&server, &token, &trip_b1, &driver_b, &truck_id, serde_json::json!({})).await;
    server.post(&format!("/fleet/api/v1/trips/{trip_x}/unassign"))
        .authorization_bearer(&token).await;
    assign_trip(&server, &token, &trip_x, &driver_b, &truck_id, serde_json::json!({})).await;

    assert_ne!(previous_trip_id(&state, &trip_x).await, Some(trip_a1),
        "a link to another driver's trip can never fire and must not survive a driver change");
    assert_eq!(previous_trip_id(&state, &trip_x).await, Some(trip_b1),
        "it should re-derive onto the new driver's chain");
}

/// Mileage is frozen after settlement, and this field recomputes it. Matches
/// `apply_trip_patch`, which returns 409 rather than silently dropping the value.
#[tokio::test]
async fn test_settled_trip_rejects_an_explicit_chain_link() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Settled").await;
    let truck_id = create_truck(&server, &token, "T-SETTLED-1").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({})).await;
    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    state.db.update_trip_settlement(
        trip_b.parse().unwrap(), Some("SETTLE-1".into()), None, None, None,
    ).await.unwrap();

    let res = assign_trip(&server, &token, &trip_b, &driver_id, &truck_id, serde_json::json!({
        "previous_trip_id": trip_a
    })).await;

    assert_eq!(res.status_code(), 409,
        "a settled trip's chain link is frozen: {}", res.text());
    assert_eq!(previous_trip_id(&state, &trip_b).await, None,
        "and nothing was written before the rejection");
}

/// A predecessor with NO driver is the ordinary chain origin the create path
/// writes: it supplies the deadhead origin for mileage and is not "another
/// driver's trip". Treating it as stale would silently erase a routing input,
/// which is exactly what an over-broad cross-driver check did here once.
#[tokio::test]
async fn test_a_driverless_predecessor_is_left_alone() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Origin Keeper").await;
    let truck_id = create_truck(&server, &token, "T-ORIGIN-1").await;

    // A prior trip with no driver at all — where the truck last was.
    let origin = create_unassigned_trip(&server, &token, "Origin").await;
    assert!(
        state.db.get_trip(origin.parse().unwrap()).await.unwrap().driver_id.is_none(),
        "fixture must have no driver or this test proves nothing");

    let create = server.post("/fleet/api/v1/trips")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "previous_trip_id": origin,
            "stops": two_stop_body("X")["stops"],
        }))
        .await;
    assert_eq!(create.status_code(), 201, "create X failed: {}", create.text());
    let trip_x = create.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    assign_trip(&server, &token, &trip_x, &driver_id, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_x).await, Some(origin),
        "a driverless chain origin must survive assignment untouched");
}

/// The same distinction on the explicit path: pinning a driverless predecessor
/// is legitimate and must not be rejected as cross-driver.
#[tokio::test]
async fn test_pinning_a_driverless_predecessor_is_accepted() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Pinner").await;
    let truck_id = create_truck(&server, &token, "T-ORIGIN-2").await;

    let origin = create_unassigned_trip(&server, &token, "Origin").await;
    let trip_x = create_unassigned_trip(&server, &token, "X").await;

    let res = assign_trip(&server, &token, &trip_x, &driver_id, &truck_id, serde_json::json!({
        "previous_trip_id": origin
    })).await;

    assert_eq!(res.status_code(), 200, "pinning a driverless origin failed: {}", res.text());
    assert_eq!(previous_trip_id(&state, &trip_x).await, Some(origin));
}

/// A trip that is actually rolling cannot be re-assigned at all: there is no
/// `InTransit -> Assigned` edge in `can_transition_to`, so `assign` fails at the
/// status transition before any of the chain logic runs. Pinned here because the
/// chain-derivation guard was originally justified by a "mid-run truck swap"
/// that does not exist — swapping equipment under a rolling trip means a new
/// trip, not a re-assignment.
#[tokio::test]
async fn test_an_in_transit_trip_cannot_be_reassigned() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Rolling").await;
    let truck_id = create_truck(&server, &token, "T-ROLL-1").await;
    let truck_two = create_truck(&server, &token, "T-ROLL-2").await;

    let trip = create_unassigned_trip(&server, &token, "R").await;
    assign_trip(&server, &token, &trip, &driver_id, &truck_id, serde_json::json!({})).await;
    set_status(&state, &trip, ollie::models::TripStatus::Dispatched).await;
    set_status(&state, &trip, ollie::models::TripStatus::InTransit).await;

    let res = assign_trip(&server, &token, &trip, &driver_id, &truck_two, serde_json::json!({})).await;

    assert_eq!(res.status_code(), 409,
        "an in-transit trip must not be re-assignable: {}", res.text());
}

// ── guards that iteration 2 found untested ───────────────────────────────────

/// A trip that something is already queued behind is a predecessor, not a tail.
/// Here the successor belongs to the SAME driver, so the derived tail's chain
/// leads back and the cycle guard would also catch it — see
/// `test_derivation_declines_when_another_drivers_trip_follows_this_one` for the
/// case where the successor check is the only thing standing.
#[tokio::test]
async fn test_derivation_declines_for_a_trip_that_already_has_a_successor() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Predecessor").await;
    let truck_id = create_truck(&server, &token, "T-SUCC-1").await;

    // T is driverless and Planned; Z is queued behind it.
    let trip_t = create_unassigned_trip(&server, &token, "T").await;
    let trip_z = create_unassigned_trip(&server, &token, "Z").await;
    assign_trip(&server, &token, &trip_z, &driver_id, &truck_id, serde_json::json!({
        "previous_trip_id": trip_t
    })).await;
    assert_eq!(previous_trip_id(&state, &trip_z).await, Some(trip_t.clone()),
        "fixture: Z must be queued behind T");

    // Now T itself is assigned to the same driver, with no stated preference.
    // The only tail on offer is Z — its own successor.
    assign_trip(&server, &token, &trip_t, &driver_id, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_t).await, None,
        "a trip with work already queued behind it must not be chained to that work");
}

/// A cycle that loops back through a TERMINAL leg slips past the successor check,
/// which only rejects non-terminal successors. The explicit path rejects this via
/// `validate_chain_link`; the derived path must agree.
#[tokio::test]
async fn test_derivation_declines_a_cycle_through_a_cancelled_leg() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Looper").await;
    let truck_id = create_truck(&server, &token, "T-LOOP-1").await;

    let trip_t = create_unassigned_trip(&server, &token, "T").await;
    let trip_sc = create_unassigned_trip(&server, &token, "Sc").await;
    assign_trip(&server, &token, &trip_sc, &driver_id, &truck_id, serde_json::json!({
        "previous_trip_id": trip_t
    })).await;
    let trip_z = create_unassigned_trip(&server, &token, "Z").await;
    assign_trip(&server, &token, &trip_z, &driver_id, &truck_id, serde_json::json!({})).await;
    assert_eq!(previous_trip_id(&state, &trip_z).await, Some(trip_sc.clone()),
        "fixture: Z must chain behind Sc");

    // Sc goes terminal, so it no longer blocks T's successor check.
    set_status(&state, &trip_sc, ollie::models::TripStatus::Cancelled).await;

    // T's only tail is Z, but T -> Z -> Sc -> T is a cycle.
    assign_trip(&server, &token, &trip_t, &driver_id, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_t).await, None,
        "the derived path must refuse a cycle the explicit path would reject");
    assert_ne!(previous_trip_id(&state, &trip_t).await, Some(trip_z),
        "T -> Z -> Sc -> T closes a loop through the cancelled leg");
}

/// A stale cross-driver link must NOT bypass the first-assignment gate. Deriving
/// on an already-released trip would repoint its deadhead origin, and deadhead
/// feeds driver pay — the same reason a settled trip is left alone. The broken
/// link stays visible instead; #440 covers whether this re-assign should be
/// possible at all.
#[tokio::test]
async fn test_a_stale_link_does_not_re_derive_on_a_released_trip() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_a = create_driver(&server, &token, "Driver A").await;
    let driver_b = create_driver(&server, &token, "Driver B").await;
    let truck_id = create_truck(&server, &token, "T-STALE-2").await;

    let trip_a1 = create_unassigned_trip(&server, &token, "A1").await;
    assign_trip(&server, &token, &trip_a1, &driver_a, &truck_id, serde_json::json!({})).await;
    let trip_x = create_unassigned_trip(&server, &token, "X").await;
    assign_trip(&server, &token, &trip_x, &driver_a, &truck_id, serde_json::json!({})).await;
    assert_eq!(previous_trip_id(&state, &trip_x).await, Some(trip_a1.clone()),
        "fixture: X must chain behind A1");

    // X is released, then re-assigned to a different driver without an unassign.
    set_status(&state, &trip_x, ollie::models::TripStatus::Dispatched).await;
    let trip_b1 = create_unassigned_trip(&server, &token, "B1").await;
    assign_trip(&server, &token, &trip_b1, &driver_b, &truck_id, serde_json::json!({})).await;
    assign_trip(&server, &token, &trip_x, &driver_b, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_x).await, Some(trip_a1),
        "a released trip's deadhead origin must not move under it, stale or not");
}

/// The read surface must resolve the chain link's LABEL, not just its id — the
/// UI falls back to a raw UUID prefix without it, which AGENTS.md forbids.
#[tokio::test]
async fn test_trip_detail_resolves_the_predecessor_trip_number() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_id = create_driver(&server, &token, "Labelled").await;
    let truck_id = create_truck(&server, &token, "T-LABEL-1").await;

    let trip_a = create_unassigned_trip(&server, &token, "A").await;
    assign_trip(&server, &token, &trip_a, &driver_id, &truck_id, serde_json::json!({})).await;
    let trip_b = create_unassigned_trip(&server, &token, "B").await;
    assign_trip(&server, &token, &trip_b, &driver_id, &truck_id, serde_json::json!({})).await;

    let detail = server.get(&format!("/fleet/api/v1/trips/{trip_b}"))
        .authorization_bearer(&token).await;
    assert_eq!(detail.status_code(), 200);
    let body: serde_json::Value = detail.json();

    assert_eq!(body["previous_trip_id"].as_str(), Some(trip_a.as_str()));
    let a_number = server.get(&format!("/fleet/api/v1/trips/{trip_a}"))
        .authorization_bearer(&token).await
        .json::<serde_json::Value>()["trip_number"].as_str().unwrap().to_string();
    assert_eq!(body["previous_trip_number"].as_str(), Some(a_number.as_str()),
        "the detail surface must resolve the predecessor's trip number: {body}");
}

/// The case where the successor check is load-bearing on its own. T already has a
/// non-terminal successor S, but S belongs to a DIFFERENT driver, so S is absent
/// from the tail computation for this driver and the derived tail's chain never
/// leads back to T — the cycle guard sees nothing wrong. Only the successor check
/// stops T being queued behind W while S is queued behind T.
///
/// Verified by mutation: deleting the successor check makes this fail (and
/// nothing else in the suite).
#[tokio::test]
async fn test_derivation_declines_when_another_drivers_trip_follows_this_one() {
    let (server, state, _b, _d, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let driver_d = create_driver(&server, &token, "Driver D").await;
    let driver_e = create_driver(&server, &token, "Driver E").await;
    let truck_id = create_truck(&server, &token, "T-SUCC-2").await;

    // T is driverless; S belongs to driver E and is queued behind T.
    let trip_t = create_unassigned_trip(&server, &token, "T").await;
    let trip_s = create_unassigned_trip(&server, &token, "S").await;
    assign_trip(&server, &token, &trip_s, &driver_e, &truck_id, serde_json::json!({
        "previous_trip_id": trip_t
    })).await;
    assert_eq!(previous_trip_id(&state, &trip_s).await, Some(trip_t.clone()),
        "fixture: S must be queued behind T");

    // Driver D has their own unchained work, which would otherwise be the tail.
    let trip_w = create_unassigned_trip(&server, &token, "W").await;
    assign_trip(&server, &token, &trip_w, &driver_d, &truck_id, serde_json::json!({})).await;

    assign_trip(&server, &token, &trip_t, &driver_d, &truck_id, serde_json::json!({})).await;

    assert_eq!(previous_trip_id(&state, &trip_t).await, None,
        "a trip with work already queued behind it must not be re-queued elsewhere");
    assert_ne!(previous_trip_id(&state, &trip_t).await, Some(trip_w),
        "chaining T behind W while S follows T reorders a chain nobody asked to reorder");
}

// ── #434: the auto-dispatch contract is published ────────────────────────────

/// Auto-dispatch changes the status of a record the caller did not name, and
/// cascades to driver, truck, trailer and load. That was in neither `llms.txt`
/// nor `openapi.json`, so an integrator recording a stop departure had no way to
/// know a second trip might move.
///
/// This asserts the *contract surfaces* carry it. It deliberately keys on the
/// distinguishing facts (the two event type names, the chain-only rule) rather
/// than on prose, so a rewording passes and a silent removal does not.
#[tokio::test]
async fn test_auto_dispatch_is_documented_in_llms_txt() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let body = server.get("/llms.txt").await.text();

    // Every needle must be absent from main, or it guards nothing. "auto_dispatch"
    // alone would be satisfied by the event names below; "previous_trip_id" was
    // already in this file. Both were dropped for that reason.
    for needle in [
        "Auto-dispatch on delivery",
        "trip.auto_dispatch_no_chain",
        "trip.auto_dispatch_ambiguous",
        "candidate_trip_ids",
        "unchained_assigned_trip_ids",
        "triggered_by_trip_id",
        "never auto-dispatches",
        "best-effort",
        "emit no event",
        "does NOT auto-dispatch",
    ] {
        assert!(body.contains(needle),
            "llms.txt must document the auto-dispatch rule; missing: {needle}");
    }
}

/// The MCP tool descriptions are the surface an agent actually reads, and they
/// are hand-maintained JSON — the likeliest thing to drift out of step.
#[tokio::test]
async fn test_auto_dispatch_is_documented_in_the_mcp_tool_descriptions() {
    let raw = include_str!("../../src/api/fleet_portal/mcp_tools.json");
    let spec: serde_json::Value = serde_json::from_str(raw).expect("mcp_tools.json parses");
    let tools = spec.get("tools").and_then(|t| t.as_array())
        .or_else(|| spec.as_array())
        .expect("tool list");

    let describe = |name: &str| -> String {
        tools.iter()
            .find(|t| t["name"] == name)
            .and_then(|t| t["description"].as_str())
            .unwrap_or_else(|| panic!("{name} tool missing"))
            .to_string()
    };

    let depart = describe("stop_depart");
    assert!(depart.contains("AUTO-DISPATCH A DIFFERENT TRIP"),
        "stop_depart must declare the side effect; got: {depart}");
    assert!(depart.contains("chain-only"),
        "stop_depart must state the selection rule; got: {depart}");
    assert!(depart.contains("emit no event"),
        "stop_depart must not imply the resource moves are journalled; got: {depart}");

    let complete = describe("complete_trip");
    assert!(complete.contains("Does NOT auto-dispatch"),
        "complete_trip must say it is not the trigger; got: {complete}");
}

/// The side effect belongs on the paths that actually cause it — both stop
/// departure surfaces — and explicitly NOT on complete, which does not trigger it.
#[tokio::test]
async fn test_auto_dispatch_side_effect_is_declared_on_the_departure_paths() {
    let (server, _state, _b, _d, _rx) = setup().await;
    let spec: serde_json::Value = server.get("/openapi.json").await.json();

    // Keyed on the SUCCESS response specifically: joining every description would
    // still pass if the side-effect text drifted onto the 404.
    let described = |path: &str, method: &str, code: &str| -> String {
        spec["paths"][path][method]["responses"][code]["description"]
            .as_str().unwrap_or_default().to_string()
    };

    for (path, method) in [
        ("/fleet/api/v1/trips/{id}/stops/{seq}/depart", "post"),
        ("/driver/api/v1/trips/{id}/stops/{seq}", "patch"),
    ] {
        let text = described(path, method, "200");
        assert!(text.contains("AUTO-DISPATCH A DIFFERENT TRIP"),
            "{path} {method} must declare that it can dispatch another trip; got: {text}");
        assert!(text.contains("chain-only"),
            "{path} {method} must state the selection rule; got: {text}");
    }

    let complete = described("/fleet/api/v1/trips/{id}/complete", "post", "204");
    assert!(complete.contains("does NOT auto-dispatch"),
        "complete must say it is not the trigger — the successor already rolled at \
         delivery, and #434 assumed otherwise; got: {complete}");
}
