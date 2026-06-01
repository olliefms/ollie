// tests/driver_equipment_test.rs
//
// Regression: the driver-UI Equipment tab showed no truck/trailer assignment for
// a driver whose trip was still `Planned` (created with equipment attached but
// not yet `/assign`-ed). `active_trip_for_driver` omitted `Planned`, so GET
// /equipment returned an empty truck + trailer list, and the picker initialized
// with nothing pre-selected — making a subsequent "Update trailer" look like it
// did nothing. This test asserts a `Planned` trip's equipment surfaces and that
// a driver-side trailer swap persists.

use axum::http::header;
use axum_test::TestServer;
use ollie::{
    ai::OllamaClient, api, config::Config, db::DbClient, storage::BlobStore, AppState,
};
use std::sync::Arc;
use tempfile::TempDir;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

async fn setup() -> (TestServer, AppState, TempDir, TempDir) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    std::env::set_var("DRIVER_JWT_SECRET", "test-driver-jwt-secret-that-is-long-enough");
    std::env::set_var("DRIVER_RP_ID", "localhost");
    std::env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
    std::env::set_var("DISPATCHER_JWT_SECRET", "test-dispatcher-secret-must-be-32b");

    let config = Arc::new(Config::from_env().unwrap());
    let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai = Arc::new(OllamaClient::new(
        "http://localhost:11434", "nomic-embed-text", "llama3.2", "moondream",
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

/// First-run owner bootstrap (idempotent), returning an owner JWT.
async fn setup_owner(server: &TestServer) -> String {
    let resp = server.post("/dispatch/setup")
        .json(&serde_json::json!({
            "email": OWNER_EMAIL, "name": "Owner", "password": OWNER_PASSWORD,
        }))
        .await;
    if resp.status_code() == 200 {
        return resp.json::<serde_json::Value>()["token"].as_str().unwrap().to_string();
    }
    let login = server.post("/dispatch/auth/login")
        .json(&serde_json::json!({ "email": OWNER_EMAIL, "password": OWNER_PASSWORD }))
        .await;
    assert_eq!(login.status_code(), 200, "owner login failed");
    login.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

/// POST to the dispatch surface as the first-run owner.
async fn admin_post(server: &TestServer, path: &str, body: serde_json::Value) -> serde_json::Value {
    let token = setup_owner(server).await;
    server.post(path)
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&body)
        .await
        .json::<serde_json::Value>()
}

async fn driver_token(server: &TestServer, state: &AppState, driver_id: &str) -> String {
    // Set a PIN so credentials (and a token_version) exist.
    let owner = setup_owner(server).await;
    server.post(&format!("/dispatch/api/v1/drivers/{driver_id}/pin"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner}"))
        .json(&serde_json::json!({ "pin": "1234" }))
        .await;
    let uuid = driver_id.parse::<uuid::Uuid>().unwrap();
    let creds = state.db.get_driver_credentials(uuid).await.unwrap().unwrap();
    ollie::api::driver_portal::jwt::encode_driver_jwt(
        uuid, creds.token_version, &state.config.driver_jwt_secret,
    ).unwrap()
}

#[tokio::test]
async fn equipment_tab_shows_assignment_from_active_trip_and_persists_swap() {
    let (server, state, _b, _d) = setup().await;

    // Driver, truck, two trailers.
    let driver_id = admin_post(&server, "/dispatch/api/v1/drivers",
        serde_json::json!({ "name": "Repro Driver", "phone": "555-0100" })).await["id"]
        .as_str().unwrap().to_string();
    let truck_id = admin_post(&server, "/dispatch/api/v1/trucks",
        serde_json::json!({ "unit_number": "TRK-1" })).await["id"]
        .as_str().unwrap().to_string();
    let trailer1 = admin_post(&server, "/dispatch/api/v1/trailers",
        serde_json::json!({ "unit_number": "TRL-1", "owner": "fleet" })).await["id"]
        .as_str().unwrap().to_string();
    let trailer2 = admin_post(&server, "/dispatch/api/v1/trailers",
        serde_json::json!({ "unit_number": "TRL-2", "owner": "fleet" })).await["id"]
        .as_str().unwrap().to_string();

    // Trip with driver, truck, trailer1 assigned.
    let trip = admin_post(&server, "/dispatch/api/v1/trips", serde_json::json!({
        "driver_id": driver_id,
        "truck_id": truck_id,
        "trailer_ids": [trailer1],
    })).await;
    let trip_status = trip["status"].as_str().unwrap().to_string();
    eprintln!("created trip status = {trip_status}");

    let token = driver_token(&server, &state, &driver_id).await;
    let bearer = format!("Bearer {token}");

    // GET /equipment — should reflect the trip's truck + trailer1.
    let eq = server.get("/driver/api/v1/equipment")
        .add_header(header::AUTHORIZATION, &bearer)
        .await;
    assert_eq!(eq.status_code(), 200, "equipment GET should be 200");
    let eq_body = eq.json::<serde_json::Value>();
    eprintln!("GET /equipment => {eq_body}");
    assert!(eq_body["truck"].is_object(), "truck should be present (sourced from active trip)");
    assert_eq!(eq_body["trailers"].as_array().unwrap().len(), 1,
        "trailer1 should show as currently attached");

    // PUT /equipment/trailer — swap to trailer2.
    let put = server.put("/driver/api/v1/equipment/trailer")
        .add_header(header::AUTHORIZATION, &bearer)
        .json(&serde_json::json!({ "trailer_ids": [trailer2] }))
        .await;
    eprintln!("PUT status {} => {}", put.status_code(), put.text());
    assert_eq!(put.status_code(), 200, "trailer update should be 200");
    let put_body = put.json::<serde_json::Value>();
    assert_eq!(put_body["trailers"].as_array().unwrap().len(), 1,
        "PUT response must echo the new trailer (else UI shows 'No trailer' after 'Updated')");

    // GET again — trailer2 should persist.
    let eq2 = server.get("/driver/api/v1/equipment")
        .add_header(header::AUTHORIZATION, &bearer)
        .await
        .json::<serde_json::Value>();
    eprintln!("GET /equipment after swap => {eq2}");
    assert_eq!(eq2["trailers"].as_array().unwrap().len(), 1,
        "trailer2 should persist as currently attached");
    assert_eq!(eq2["trailers"][0]["unit_number"].as_str(), Some("TRL-2"));
}
