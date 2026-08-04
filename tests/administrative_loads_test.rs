// tests/administrative_loads_test.rs
//
// Administrative (no-trip) loads: revenue with no truck behind it — a weekly
// revenue guarantee, TONU, detention-only billing, layover, an accessorial-only
// freight bill. Such a load has no path to `delivered`, because every route
// there is a trip-side cascade, so it invoices straight from `planned`.
//
// These tests guard the isolation that makes that safe: an administrative load
// must never acquire a trip, and its kind must not be changeable once it has
// moved past `planned`.

use axum::http::header;
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

/// Create a stopless load through the API and return its id.
async fn create_bare_load(
    server: &TestServer, token: &str, load_number: &str, kind: &str,
) -> String {
    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "load_number": load_number,
            "customer_name": "Landstar",
            "stops": [],
            "kind": kind,
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "create failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn create_test_facility(server: &TestServer, token: &str, name: &str, address: &str) -> String {
    let resp = server.post("/fleet/api/v1/facilities")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "name": name, "address": address }))
        .await;
    assert_eq!(resp.status_code(), 201, "create facility failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn create_driver(server: &TestServer, token: &str, name: &str) -> String {
    let resp = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "name": name }))
        .await;
    assert_eq!(resp.status_code(), 201, "create driver failed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn create_truck(server: &TestServer, token: &str, unit_number: &str) -> String {
    let resp = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
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

#[tokio::test]
async fn test_trip_cannot_be_created_against_an_administrative_load() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let load_id = create_bare_load(&server, &token, "4581461", "administrative").await;

    let resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "load_id": load_id, "stops": [] }))
        .await;

    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert!(
        resp.text().contains("administrative"),
        "the error should name the kind so the caller knows why: {}",
        resp.text(),
    );
}

#[tokio::test]
async fn test_kind_cannot_change_once_the_load_has_left_planned() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let load_id = create_bare_load(&server, &token, "4581461", "administrative").await;

    // planned -> invoiced, an edge only an administrative load has.
    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "invoice_number": "JQL-4581461" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Reclassifying it now would leave a status the freight machine can't explain.
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "kind": "freight" }))
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());
}

#[tokio::test]
async fn test_kind_cannot_change_while_the_load_has_trips() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, &token, "Trip Guard Dock", "Chicago, IL").await;

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "load_number": "4581464",
            "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")],
            "kind": "freight",
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let load_id = resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Attach a trip to the load through the real API — this is the state
    // apply_load_kind_change's "load has trips" branch must catch.
    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(trip_resp.status_code(), 201, "trip create failed: {}", trip_resp.text());

    // Reclassifying now would orphan the trip — the guard must reject it.
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "kind": "administrative" }))
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());
    assert!(
        resp.text().contains("trip"),
        "the error should name the reason (trips) so the caller knows why: {}",
        resp.text(),
    );
}

#[tokio::test]
async fn test_kind_flip_to_administrative_does_not_clear_miles_via_routing_guard() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, &token, "Regression Dock", "Dallas, TX").await;

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "load_number": "4581465",
            "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")],
            "kind": "freight",
            "miles": 250.0,
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let body = resp.json::<serde_json::Value>();
    let load_id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["miles"], 250.0, "sanity: load starts with a known mileage");

    // Single update: change the stops AND flip kind to administrative in the
    // same request. The routing-enqueue guard must see the POST-change kind
    // (administrative), not the pre-change one, so it must not treat this as
    // a still-freight load that needs its miles cleared and re-routed.
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "stops": [stop_json(&fac_id, "2026-06-02T08:00:00")],
            "kind": "administrative",
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["kind"], "administrative");
    // Observable consequence of the ordering bug: the routing-enqueue block
    // both clears miles and (mis-)queues the load for ORS routing under one
    // `if`. We can't observe the routing channel directly (setup()'s
    // receiver is wired to the pipeline channel, not the routing one, and
    // the routing receiver is dropped inside setup() so try_send is always
    // a silent no-op either way) — but the miles field is DB-observable and
    // driven by the exact same guard, so it stands in for "was queued".
    assert_eq!(
        body["miles"], 250.0,
        "an administrative load's miles must survive a stops-changing update — reading the \
         PRE-change kind would treat this as still-freight and wipe miles on its way to \
         (incorrectly) queuing the load for ORS routing",
    );

    let refetched = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(refetched.status_code(), 200);
    assert_eq!(
        refetched.json::<serde_json::Value>()["miles"], 250.0,
        "miles must not have been cleared at the DB layer either",
    );
}

#[tokio::test]
async fn test_kind_flip_to_freight_does_clear_miles_via_routing_guard() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, &token, "Mirror Dock", "Dallas, TX").await;

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "load_number": "4581466",
            "customer_name": "Landstar",
            "stops": [],
            "kind": "administrative",
            "miles": 250.0,
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let body = resp.json::<serde_json::Value>();
    let load_id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["miles"], 250.0, "sanity: load starts with a known mileage");

    // Mirror of the freight->administrative regression above: flip an
    // administrative load to freight in the same PUT that supplies new
    // stops. Now the post-change kind IS freight, so the routing-enqueue
    // guard must fire — miles get cleared for re-routing.
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "stops": [stop_json(&fac_id, "2026-06-02T08:00:00")],
            "kind": "freight",
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["kind"], "freight");
    assert_eq!(
        body["miles"], serde_json::Value::Null,
        "a load flipped to freight with new stops must have its stale miles cleared \
         so it gets re-routed",
    );

    let refetched = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(refetched.status_code(), 200);
    assert_eq!(
        refetched.json::<serde_json::Value>()["miles"], serde_json::Value::Null,
        "miles must be cleared at the DB layer too",
    );
}

// ── MCP surface shares the same guard (REST-only coverage was a gap) ──
//
// Helpers copied from tests/expenses_test.rs — each integration test file is
// its own binary, so there's no shared test-support module to import from.

/// Extract the single JSON-RPC message from a Streamable-HTTP SSE response body.
/// rmcp frames each POST reply as one `event: message` / `data: {…}` SSE event.
fn sse_json(body: &str) -> serde_json::Value {
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.trim();
            if rest.is_empty() {
                continue; // priming/keep-alive frames carry no JSON payload
            }
            return serde_json::from_str(rest)
                .unwrap_or_else(|e| panic!("SSE data not JSON ({e}): {rest}"));
        }
    }
    panic!("no SSE `data:` event in body: {body:?}");
}

/// Open an MCP session: `initialize` then the `notifications/initialized` ack.
/// Returns the `Mcp-Session-Id` the server assigned (used on subsequent calls).
async fn mcp_session(server: &TestServer, token: &str) -> String {
    let resp = server
        .post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "ollie-test", "version": "1.0" }
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "initialize HTTP {}", resp.status_code());
    let session = resp
        .headers()
        .get("mcp-session-id")
        .expect("initialize must return an Mcp-Session-Id header")
        .to_str()
        .unwrap()
        .to_string();
    server
        .post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .add_header("mcp-session-id", session.clone())
        .json(&serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    session
}

/// Send one JSON-RPC request on an existing session and return the parsed reply
/// (the full JSON-RPC envelope: `{ jsonrpc, id, result | error }`).
async fn mcp_rpc(
    server: &TestServer,
    token: &str,
    session: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let resp = server
        .post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .add_header("mcp-session-id", session.to_string())
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "MCP {method} HTTP {}", resp.status_code());
    sse_json(&resp.text())
}

#[tokio::test]
async fn test_mcp_update_load_rejects_illegal_kind_change() {
    // All six tests above drive REST. `tool_update_load` in src/api/fleet_portal/
    // mcp.rs runs the identical validate_load_kind_change/apply_load_kind_change
    // guard, but MCP is the surface an AI agent actually calls — this whole
    // feature exists because an agent hit this wall over MCP. Prove the guard
    // holds there too, and that a rejected call writes nothing.
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, &token, "MCP Guard Dock", "Chicago, IL").await;
    let other_fac_id = create_test_facility(&server, &token, "MCP Other Dock", "Atlanta, GA").await;

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "load_number": "4581468",
            "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")],
            "kind": "freight",
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let load_id = resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Attach a trip so the guard has grounds to reject.
    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(trip_resp.status_code(), 201, "trip create failed: {}", trip_resp.text());

    let session = mcp_session(&server, &token).await;
    let body = mcp_rpc(
        &server,
        &token,
        &session,
        "tools/call",
        serde_json::json!({
            "name": "update_load",
            "arguments": {
                "id": load_id,
                "stops": [{
                    "sequence": 1, "stop_type": "pickup", "service_type": "live_load",
                    "facility_id": other_fac_id, "scheduled_arrive": "2026-06-02T08:00:00",
                    "timezone": "America/Chicago",
                }],
                "kind": "administrative",
            }
        }),
    ).await;

    // A domain rejection comes back as a normal JSON-RPC result with
    // isError: true (NOT a JSON-RPC-level error) — see call_tool's
    // ToolError::Domain arm in src/api/fleet_portal/mcp.rs.
    assert!(body["error"].is_null(), "expected a normal result, not a JSON-RPC error: {body}");
    assert_eq!(body["result"]["isError"], true, "body: {body}");
    let text = body["result"]["content"][0]["text"].as_str().expect("content[0].text missing");
    assert!(
        text.contains("trip"),
        "the error should name the reason (trips) so the caller knows why: {text}",
    );

    // Writes nothing: kind and stops must both be untouched.
    let refetched = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(refetched.status_code(), 200);
    let load = refetched.json::<serde_json::Value>();
    assert_eq!(load["kind"], "freight", "rejected MCP kind change must not have written: {load}");
    let stops = load["stops"].clone();
    assert_eq!(
        stops.as_array().map(|a| a.len()), Some(1),
        "rejected MCP call must not have written the new stops either: {stops}",
    );
    assert_eq!(stops[0]["facility_id"], fac_id, "stops must remain the original facility: {stops}");
}

#[tokio::test]
async fn test_rejected_kind_change_does_not_write_stops() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, &token, "Partial Write Dock", "Chicago, IL").await;
    let other_fac_id = create_test_facility(&server, &token, "Other Dock", "Atlanta, GA").await;

    let resp = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "load_number": "4581467",
            "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")],
            "kind": "freight",
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let load_id = resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Give the load a trip so the kind-change guard will reject.
    let trip_resp = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(trip_resp.status_code(), 201, "trip create failed: {}", trip_resp.text());

    // Single PUT: new stops AND a kind change that must be rejected. The
    // stops write must not land — if it did, the load would be left with
    // new stops, stale miles, and no routing re-queue: silent inconsistency.
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({
            "stops": [stop_json(&other_fac_id, "2026-06-02T08:00:00")],
            "kind": "administrative",
        }))
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());

    let refetched = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(refetched.status_code(), 200);
    let stops = refetched.json::<serde_json::Value>()["stops"].clone();
    assert_eq!(
        stops.as_array().map(|a| a.len()), Some(1),
        "the rejected kind change must not have left the new stops written: {stops}",
    );
    assert_eq!(
        stops[0]["facility_id"], fac_id,
        "stops must remain the original facility, not the one from the rejected update: {stops}",
    );
}

/// The reported sequence, verbatim: a weekly revenue guarantee that was paid on
/// a contractor settlement but has no trip behind it. Before this shipped,
/// `invoice_load` answered `cannot transition from 'planned' to 'invoiced'` and
/// the load sat in `planned` forever, so every "what is unsettled?" query
/// returned it week after week.
#[tokio::test]
async fn test_guarantee_load_invoices_and_settles_with_no_trip() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    // Starts as an ordinary load, and is reclassified in place — not recreated.
    let load_id = create_bare_load(&server, &token, "4581461", "freight").await;
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "kind": "administrative" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "invoice_number": "JQL-4581461",
            "invoice_date": "2026-07-29",
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let uuid: uuid::Uuid = load_id.parse().unwrap();
    let settled = state.db.get_load_by_id(uuid).await.unwrap();
    assert_eq!(settled.status, LoadStatus::Settled);
    assert_eq!(settled.invoice_number.as_deref(), Some("JQL-4581461"));
    assert!(state.db.list_trips_for_load(uuid).await.unwrap().is_empty());

    // It drops out of the unsettled queue.
    let resp = server.get("/fleet/api/v1/loads?status=planned")
        .authorization_bearer(&token)
        .await;
    assert_eq!(resp.json::<serde_json::Value>()["total"], serde_json::json!(0));

    // And load_doctor does not flag it. The check gates on Dispatched|InTransit
    // and load_trips_all_delivered is false on an empty survivor set, so this is
    // regression insurance against a later loosening of either.
    let report = ollie::services::doctors::load::run(&state, uuid, false).await.unwrap();
    assert!(
        !report.findings.iter().any(|f| f.check == "load.status_matches_trips"),
        "administrative loads must not be flagged as status-mismatched: {:?}",
        report.findings,
    );
}

/// The headline TONU (Truck Ordered, Not Used) shape: a freight load gets a
/// trip, the trip is cancelled before it ever rolls, and the load must still be
/// reclassifiable to administrative so the TONU fee can be invoiced. Before the
/// fix, `validate_load_kind_change`'s `list_trips_for_load` guard counted the
/// cancelled trip as still-live and refused the reclassification forever.
#[tokio::test]
async fn test_cancelled_trip_does_not_block_reclassification_to_administrative() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, &token, "TONU Dock", "Chicago, IL").await;

    let resp = server.post("/fleet/api/v1/loads")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581469",
            "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")],
            "kind": "freight",
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let load_id = resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_resp = server.post("/fleet/api/v1/trips")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(trip_resp.status_code(), 201, "trip create failed: {}", trip_resp.text());
    let trip_id = trip_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Cancel it — a truck was ordered, then it was not used.
    let cancel_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/cancel"))
        .authorization_bearer(&token)
        .await;
    assert_eq!(cancel_resp.status_code(), 200, "cancel failed: {}", cancel_resp.text());

    let after_cancel = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .await;
    assert_eq!(after_cancel.status_code(), 200);
    assert_eq!(
        after_cancel.json::<serde_json::Value>()["status"], "planned",
        "the trip was never assigned, so the load never left planned",
    );

    // The reclassification the whole feature exists for.
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "kind": "administrative" }))
        .await;
    assert_eq!(resp.status_code(), 200, "kind change rejected: {}", resp.text());
    assert_eq!(resp.json::<serde_json::Value>()["kind"], "administrative");

    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "invoice_number": "JQL-4581469" }))
        .await;
    assert_eq!(resp.status_code(), 200, "invoice failed: {}", resp.text());

    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(resp.status_code(), 200, "settle failed: {}", resp.text());

    let uuid: uuid::Uuid = load_id.parse().unwrap();
    let settled = state.db.get_load_by_id(uuid).await.unwrap();
    assert_eq!(settled.status, LoadStatus::Settled);
}

/// TONU's full shape: a truck is actually ordered — a driver and truck get
/// *assigned* to the trip (load -> `Assigned`) — and only then is it cancelled
/// (load demotes back through the `trip_lifecycle` cascade to `Planned`)
/// before the load is reclassified administrative. The test above never
/// assigns anyone, so the load never leaves `planned` and the demote cascade
/// is never exercised; this one drives it.
#[tokio::test]
async fn test_tonu_load_assigned_then_cancelled_can_be_reclassified() {
    let (server, state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;
    let fac_id = create_test_facility(&server, &token, "TONU Assign Dock", "Chicago, IL").await;
    let driver_id = create_driver(&server, &token, "TONU Driver").await;
    let truck_id = create_truck(&server, &token, "TONU-1").await;

    let resp = server.post("/fleet/api/v1/loads")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581472",
            "customer_name": "Landstar",
            "stops": [stop_json(&fac_id, "2026-06-01T08:00:00")],
            "kind": "freight",
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let load_id = resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_resp = server.post("/fleet/api/v1/trips")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "load_id": load_id }))
        .await;
    assert_eq!(trip_resp.status_code(), 201, "trip create failed: {}", trip_resp.text());
    let trip_id = trip_resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Order the truck: assign a driver and truck to the trip.
    let assign_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "driver_id": driver_id, "truck_id": truck_id }))
        .await;
    assert_eq!(assign_resp.status_code(), 200, "assign failed: {}", assign_resp.text());

    let after_assign = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .await;
    assert_eq!(after_assign.status_code(), 200);
    assert_eq!(
        after_assign.json::<serde_json::Value>()["status"], "assigned",
        "assigning a driver and truck must move the load out of planned",
    );

    // Not used: cancel the trip after the truck was ordered.
    let cancel_resp = server.post(&format!("/fleet/api/v1/trips/{trip_id}/cancel"))
        .authorization_bearer(&token)
        .await;
    assert_eq!(cancel_resp.status_code(), 200, "cancel failed: {}", cancel_resp.text());

    let after_cancel = server.get(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .await;
    assert_eq!(after_cancel.status_code(), 200);
    assert_eq!(
        after_cancel.json::<serde_json::Value>()["status"], "planned",
        "cancelling the only trip holding the load must demote it back to planned",
    );

    // The reclassification the whole feature exists for.
    let resp = server.put(&format!("/fleet/api/v1/loads/{load_id}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "kind": "administrative" }))
        .await;
    assert_eq!(resp.status_code(), 200, "kind change rejected: {}", resp.text());
    assert_eq!(resp.json::<serde_json::Value>()["kind"], "administrative");

    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/invoice"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "invoice_number": "JQL-4581472" }))
        .await;
    assert_eq!(resp.status_code(), 200, "invoice failed: {}", resp.text());

    let resp = server.post(&format!("/fleet/api/v1/loads/{load_id}/settle"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(resp.status_code(), 200, "settle failed: {}", resp.text());

    let uuid: uuid::Uuid = load_id.parse().unwrap();
    let settled = state.db.get_load_by_id(uuid).await.unwrap();
    assert_eq!(settled.status, LoadStatus::Settled);
}

/// Permanent regression guard for a defect fixed earlier in this branch
/// (commit 382f117): `tool_list_loads` advertised a `tags` filter in its
/// schema while silently dropping it before the call ever reached
/// `db.list_loads`. That was an argument-extraction bug in the MCP glue
/// itself, not in `DbClient::list_loads`, so no DB-level test could ever have
/// caught it — only a call through the actual MCP tool proves the glue works.
#[tokio::test]
async fn test_mcp_list_loads_tags_filter_reaches_the_db() {
    let (server, _state, _d1, _d2, _rx) = setup().await;
    let token = setup_owner(&server).await;

    let resp = server.post("/fleet/api/v1/loads")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581470",
            "customer_name": "Landstar",
            "stops": [],
            "kind": "administrative",
            "tags": ["urgent"],
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());
    let urgent_id = resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let resp = server.post("/fleet/api/v1/loads")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "load_number": "4581471",
            "customer_name": "Landstar",
            "stops": [],
            "kind": "administrative",
            "tags": ["routine"],
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "load create failed: {}", resp.text());

    let session = mcp_session(&server, &token).await;
    let body = mcp_rpc(
        &server,
        &token,
        &session,
        "tools/call",
        serde_json::json!({
            "name": "list_loads",
            "arguments": { "tags": ["urgent"] }
        }),
    ).await;

    assert!(body["error"].is_null(), "expected a normal result: {body}");
    assert_ne!(body["result"]["isError"], serde_json::json!(true), "body: {body}");
    let structured = &body["result"]["structuredContent"];
    let items = structured["items"].as_array().expect("items array missing");
    assert_eq!(
        items.len(), 1,
        "only the 'urgent'-tagged load should come back: {structured}",
    );
    assert_eq!(items[0]["id"], urgent_id, "wrong load returned: {structured}");
}
