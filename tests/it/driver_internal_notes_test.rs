// tests/it/driver_internal_notes_test.rs
//
// #423: `internal_notes` on Load and Trip is dispatcher-only. The driver surface
// builds its own response structs and never maps the field, so this is already
// true by construction — these tests exist to keep it true.
//
// Every assertion is on the *serialized body*, never on a field name, so a leak
// through a renamed or nested field still fails the test.
//
// The vacuity guard matters as much as the leak check: if the sentinel were
// never persisted, "absent from the driver body" would pass for the wrong
// reason (cf. the AGENTS.md lesson on assertions that hold because a dependency
// is missing). Each test asserts the dispatcher CAN read the sentinel back
// before asserting the driver cannot.

use axum::http::header;
use axum_test::TestServer;
use ollie::{
    ai::OllamaClient, api, config::Config, db::DbClient, storage::BlobStore, AppState,
};
use std::sync::Arc;
use tempfile::TempDir;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

/// Distinctive enough that a substring match cannot collide with anything else
/// in a response body.
const LOAD_SENTINEL: &str = "LOADSENTINEL-b41d7e02-rate-con-4200usd-do-not-leak";
const TRIP_SENTINEL: &str = "TRIPSENTINEL-6c8f9a13-chained-from-T-2026-0135-do-not-leak";

async fn setup() -> (TestServer, AppState, TempDir, TempDir) {
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
        "http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream",
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
    let auth_challenge_store = Arc::new(dashmap::DashMap::new());
    let reg_challenge_store = Arc::new(dashmap::DashMap::new());

    let state = AppState {
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
        webauthn, auth_challenge_store, reg_challenge_store,
    };
    let server = TestServer::new(api::router(state.clone())).unwrap();
    (server, state, blob_dir, db_dir)
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

struct Fixture {
    owner_token: String,
    driver_token: String,
    load_id: String,
    trip_id: String,
}

/// A driver on an InTransit trip whose trip AND parent load both carry
/// `internal_notes`.
async fn setup_trip_with_internal_notes(server: &TestServer, state: &AppState) -> Fixture {
    let owner_token = setup_owner(server).await;

    let driver_id_str = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Internal Notes Driver" }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let driver_id: uuid::Uuid = driver_id_str.parse().unwrap();

    let creds = ollie::models::DriverCredentials {
        driver_id,
        pin_hash: None,
        token_version: 1,
        failed_pin_attempts: 0,
        locked_until: None,
        updated_at: chrono::Utc::now(),
    };
    state.db.upsert_driver_credentials(&creds).await.unwrap();

    let truck_id = server.post("/fleet/api/v1/trucks")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "unit_number": format!("T-INT-{}", uuid::Uuid::new_v4()) }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let load_id = server.post("/fleet/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "customer_name": "Acme Freight",
            "notes": "driver briefing: dock 4, ask for Marty",
            "internal_notes": LOAD_SENTINEL,
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "load_id": load_id,
            "notes": "driver briefing: seal number on the BOL",
            "internal_notes": TRIP_SENTINEL,
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "Origin",
                  "timezone": "America/Los_Angeles" },
                { "sequence": 2, "stop_type": "delivery", "name": "Destination",
                  "timezone": "America/Los_Angeles" }
            ]
        }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let assign = server.post(&format!("/fleet/api/v1/trips/{trip_id}/assign"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "driver_id": driver_id_str, "truck_id": truck_id }))
        .await;
    assert_eq!(assign.status_code(), 200);

    let dispatch = server.post(&format!("/fleet/api/v1/trips/{trip_id}/dispatch"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;
    assert_eq!(dispatch.status_code(), 200);

    let trip_uuid: uuid::Uuid = trip_id.parse().unwrap();
    state.db.transition_trip_status(trip_uuid, ollie::models::TripStatus::InTransit)
        .await.unwrap();

    let secret = std::env::var("DRIVER_JWT_SECRET").unwrap();
    let driver_token =
        ollie::api::driver_portal::jwt::encode_driver_jwt(driver_id, 1, &secret).unwrap();

    Fixture { owner_token, driver_token, load_id, trip_id }
}

#[tokio::test]
async fn test_internal_notes_round_trip_on_the_dispatcher_surface() {
    let (server, state, _b, _d) = setup().await;
    let fx = setup_trip_with_internal_notes(&server, &state).await;

    let load = server.get(&format!("/fleet/api/v1/loads/{}", fx.load_id))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", fx.owner_token))
        .await;
    assert_eq!(load.status_code(), 200);
    assert_eq!(load.json::<serde_json::Value>()["internal_notes"], LOAD_SENTINEL,
        "load internal_notes must round-trip through create + GET");

    let trip = server.get(&format!("/fleet/api/v1/trips/{}", fx.trip_id))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", fx.owner_token))
        .await;
    assert_eq!(trip.status_code(), 200);
    assert_eq!(trip.json::<serde_json::Value>()["internal_notes"], TRIP_SENTINEL,
        "trip internal_notes must round-trip through create + GET");
}

#[tokio::test]
async fn test_internal_notes_updatable_and_still_dispatcher_only() {
    let (server, state, _b, _d) = setup().await;
    let fx = setup_trip_with_internal_notes(&server, &state).await;

    const UPDATED: &str = "UPDATEDSENTINEL-0d5e1f77-escrow-deduction-do-not-leak";

    let put = server.put(&format!("/fleet/api/v1/loads/{}", fx.load_id))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", fx.owner_token))
        .json(&serde_json::json!({ "internal_notes": UPDATED }))
        .await;
    assert_eq!(put.status_code(), 200);
    assert_eq!(put.json::<serde_json::Value>()["internal_notes"], UPDATED);

    let patch = server.patch(&format!("/fleet/api/v1/trips/{}", fx.trip_id))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", fx.owner_token))
        .json(&serde_json::json!({ "internal_notes": UPDATED }))
        .await;
    assert_eq!(patch.status_code(), 200, "PATCH must accept internal_notes");

    let trip = server.get(&format!("/fleet/api/v1/trips/{}", fx.trip_id))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", fx.owner_token))
        .await;
    assert_eq!(trip.json::<serde_json::Value>()["internal_notes"], UPDATED);

    // And the updated value is just as invisible to the driver as the original.
    for path in [
        format!("/driver/api/v1/trips/{}", fx.trip_id),
        format!("/driver/api/v1/trips/{}/stops/1", fx.trip_id),
    ] {
        let resp = server.get(&path)
            .add_header(header::AUTHORIZATION, format!("Bearer {}", fx.driver_token))
            .await;
        let body = String::from_utf8_lossy(resp.as_bytes()).to_string();
        assert!(!body.contains(UPDATED), "updated internal_notes leaked via {path}: {body}");
    }
}

/// The core guarantee: no `/driver/api/v1` response body contains either
/// sentinel, anywhere, under any key.
#[tokio::test]
async fn test_internal_notes_never_appear_in_any_driver_response() {
    let (server, state, _b, _d) = setup().await;
    let fx = setup_trip_with_internal_notes(&server, &state).await;
    let auth = format!("Bearer {}", fx.driver_token);

    // Guard against a vacuous pass: the sentinels must actually be persisted,
    // or every assertion below holds for the wrong reason.
    let fleet_trip = server.get(&format!("/fleet/api/v1/trips/{}", fx.trip_id))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", fx.owner_token))
        .await;
    assert!(
        String::from_utf8_lossy(fleet_trip.as_bytes()).contains(TRIP_SENTINEL),
        "precondition: the dispatcher surface must expose the trip sentinel",
    );
    let fleet_load = server.get(&format!("/fleet/api/v1/loads/{}", fx.load_id))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", fx.owner_token))
        .await;
    assert!(
        String::from_utf8_lossy(fleet_load.as_bytes()).contains(LOAD_SENTINEL),
        "precondition: the dispatcher surface must expose the load sentinel",
    );

    // Upload a driver document so the documents list is non-empty rather than
    // trivially clean.
    let form = axum_test::multipart::MultipartForm::new()
        .add_text("doctype", "bol")
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(b"bol-bytes".to_vec())
                .file_name("bol.txt")
                .mime_type("text/plain"),
        );
    let upload = server.post(&format!("/driver/api/v1/trips/{}/documents", fx.trip_id))
        .add_header(header::AUTHORIZATION, auth.clone())
        .multipart(form)
        .await;
    assert!(
        upload.status_code().is_success(),
        "document upload failed: {}", upload.status_code(),
    );

    let paths = vec![
        "/driver/api/v1/me".to_string(),
        "/driver/api/v1/trips".to_string(),
        "/driver/api/v1/trips?tab=current".to_string(),
        "/driver/api/v1/trips?tab=upcoming".to_string(),
        "/driver/api/v1/trips?tab=past".to_string(),
        "/driver/api/v1/equipment".to_string(),
        format!("/driver/api/v1/trips/{}", fx.trip_id),
        format!("/driver/api/v1/trips/{}/stops/1", fx.trip_id),
        format!("/driver/api/v1/trips/{}/stops/2", fx.trip_id),
        format!("/driver/api/v1/trips/{}/documents", fx.trip_id),
    ];

    for path in paths {
        let resp = server.get(&path)
            .add_header(header::AUTHORIZATION, auth.clone())
            .await;
        assert_eq!(resp.status_code(), 200, "{path} did not return 200");
        let body = String::from_utf8_lossy(resp.as_bytes()).to_string();

        assert!(!body.contains(LOAD_SENTINEL),
            "load internal_notes leaked into {path}: {body}");
        assert!(!body.contains(TRIP_SENTINEL),
            "trip internal_notes leaked into {path}: {body}");
        assert!(!body.contains("internal_notes"),
            "the field name itself appeared in {path}: {body}");
    }
}

/// The driver trip detail still carries the driver-facing `notes` — this change
/// must not have moved the wrong field out of reach.
#[tokio::test]
async fn test_driver_still_sees_the_driver_facing_notes() {
    let (server, state, _b, _d) = setup().await;
    let fx = setup_trip_with_internal_notes(&server, &state).await;

    let resp = server.get(&format!("/driver/api/v1/trips/{}", fx.trip_id))
        .add_header(header::AUTHORIZATION, format!("Bearer {}", fx.driver_token))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["notes"], "driver briefing: seal number on the BOL",
        "trip.notes is the driver briefing and must still be delivered");
}

/// #424: the driver read endpoints and their response schemas are published.
#[tokio::test]
async fn test_openapi_publishes_driver_read_endpoints() {
    let (server, _state, _b, _d) = setup().await;
    let spec: serde_json::Value = server.get("/openapi.json").await.json();

    for path in [
        "/driver/api/v1/me",
        "/driver/api/v1/trips",
        "/driver/api/v1/trips/{id}",
        "/driver/api/v1/trips/{id}/stops/{seq}",
    ] {
        assert!(!spec["paths"][path]["get"].is_null(),
            "{path} GET missing from the OpenAPI spec");
    }

    for schema in [
        "DriverMeResponse",
        "DriverTripListResponse",
        "DriverTripListItem",
        "DriverTripDetailResponse",
        "DriverTripStopSummary",
        "DriverTripLoadSummary",
        "DriverStopDetailResponse",
    ] {
        assert!(!spec["components"]["schemas"][schema].is_null(),
            "{schema} missing from the OpenAPI components");
    }

    // The driver load summary is a deliberate subset — publishing it is what
    // makes that checkable without reading the handler source.
    let load_summary = &spec["components"]["schemas"]["DriverTripLoadSummary"]["properties"];
    for absent in ["notes", "internal_notes", "rate_items", "customer_name", "total_rate_usd"] {
        assert!(load_summary[absent].is_null(),
            "DriverTripLoadSummary must not expose {absent}");
    }
}
