// tests/driver_expenses_test.rs
//
// Driver-portal expense endpoints (#233 task 9): uploading a receipt with
// doctype=expense creates an ExpenseRecord; drivers can list their own
// expenses and delete their own un-reviewed ones.

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

/// Driver + truck + an assigned, dispatched, InTransit trip with two stops.
/// Returns (driver_token, trip_id).
async fn setup_driver_with_intransit_trip_two_stops(
    server: &TestServer,
    state: &AppState,
) -> (String, String) {
    let owner_token = setup_owner(server).await;
    let driver_id_str = server.post("/fleet/api/v1/drivers")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "name": "Expense Driver" }))
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
        .json(&serde_json::json!({ "unit_number": format!("T-EXP-{}", uuid::Uuid::new_v4()) }))
        .await
        .json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let trip_id = server.post("/fleet/api/v1/trips")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "stops": [
                { "sequence": 1, "stop_type": "pickup", "name": "Origin", "timezone": "America/Los_Angeles" },
                { "sequence": 2, "stop_type": "delivery", "name": "Destination", "timezone": "America/Los_Angeles" }
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
    state.db.transition_trip_status(trip_uuid, ollie::models::TripStatus::InTransit).await.unwrap();

    let secret = std::env::var("DRIVER_JWT_SECRET").unwrap();
    let token = ollie::api::driver_portal::jwt::encode_driver_jwt(driver_id, 1, &secret).unwrap();
    (token, trip_id)
}

async fn upload_expense(
    server: &TestServer,
    driver_token: &str,
    trip_id: &str,
    category: Option<&str>,
) -> axum_test::TestResponse {
    let mut form = axum_test::multipart::MultipartForm::new()
        .add_text("doctype", "expense");
    if let Some(cat) = category {
        form = form.add_text("expense_category", cat);
    }
    form = form.add_part(
        "file",
        axum_test::multipart::Part::bytes(b"receipt-bytes".to_vec())
            .file_name("receipt.txt")
            .mime_type("text/plain"),
    );
    server
        .post(&format!("/driver/api/v1/trips/{trip_id}/documents"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .multipart(form)
        .await
}

#[tokio::test]
async fn test_driver_expense_upload_creates_expense() {
    let (server, state, _b, _d) = setup().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let upload = upload_expense(&server, &driver_token, &trip_id, Some("fuel")).await;
    let sc = upload.status_code().as_u16();
    assert!(sc == 201 || sc == 202, "got {sc}");

    let list = server.get("/driver/api/v1/expenses")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(list.status_code(), 200);
    let body: serde_json::Value = list.json();
    assert_eq!(body["total"], 1);
    let item = &body["items"][0];
    assert_eq!(item["category"], "fuel");
    assert_eq!(item["status"], "submitted");
    assert!(item["trip_id"].as_str().is_some());
    assert_eq!(item["blob_ids"].as_array().unwrap().len(), 1);
    assert!(item["submitted_by"].as_str().unwrap().starts_with("driver:"));
}

#[tokio::test]
async fn test_driver_expense_upload_rejects_bad_category() {
    let (server, state, _b, _d) = setup().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let upload = upload_expense(&server, &driver_token, &trip_id, Some("snacks")).await;
    assert_eq!(upload.status_code(), 400);
}

#[tokio::test]
async fn test_scale_ticket_doctype_still_accepted() {
    let (server, state, _b, _d) = setup().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let form = axum_test::multipart::MultipartForm::new()
        .add_text("doctype", "scale_ticket")
        .add_part(
            "file",
            axum_test::multipart::Part::bytes(b"scale-bytes".to_vec())
                .file_name("scale.txt")
                .mime_type("text/plain"),
        );
    let upload = server
        .post(&format!("/driver/api/v1/trips/{trip_id}/documents"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .multipart(form)
        .await;
    let sc = upload.status_code().as_u16();
    assert!(sc == 201 || sc == 202, "got {sc}");

    let list = server.get("/driver/api/v1/expenses")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(list.json::<serde_json::Value>()["total"], 0,
        "scale_ticket upload must not create an expense");
}

#[tokio::test]
async fn test_driver_sees_reviewed_outcome_but_cannot_delete() {
    let (server, state, _b, _d) = setup().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;
    let owner_token = setup_owner(&server).await;

    let upload = upload_expense(&server, &driver_token, &trip_id, Some("fuel")).await;
    let sc = upload.status_code().as_u16();
    assert!(sc == 201 || sc == 202);

    let list = server.get("/driver/api/v1/expenses")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    let expense_id = list.json::<serde_json::Value>()["items"][0]["id"]
        .as_str().unwrap().to_string();

    let review = server.post(&format!("/fleet/api/v1/expenses/{expense_id}/review"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({
            "amount": 40.0, "approved_amount": 40.0, "payment_method": "personal",
        }))
        .await;
    assert_eq!(review.status_code(), 200);

    let list = server.get("/driver/api/v1/expenses")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    let item = list.json::<serde_json::Value>()["items"][0].clone();
    assert_eq!(item["status"], "reviewed");
    assert_eq!(item["approved_amount"], 40.0);
    assert_eq!(item["reimbursement"], 40.0);

    let delete = server.delete(&format!("/driver/api/v1/expenses/{expense_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(delete.status_code(), 403);
}

#[tokio::test]
async fn test_driver_can_delete_own_submitted() {
    let (server, state, _b, _d) = setup().await;
    let (driver_token, trip_id) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let upload = upload_expense(&server, &driver_token, &trip_id, Some("tolls")).await;
    let sc = upload.status_code().as_u16();
    assert!(sc == 201 || sc == 202);
    let blob_id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let list = server.get("/driver/api/v1/expenses")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    let expense_id = list.json::<serde_json::Value>()["items"][0]["id"]
        .as_str().unwrap().to_string();

    let delete = server.delete(&format!("/driver/api/v1/expenses/{expense_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(delete.status_code(), 204);

    let list = server.get("/driver/api/v1/expenses")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_token}"))
        .await;
    assert_eq!(list.json::<serde_json::Value>()["total"], 0);

    // Blob itself is not deleted.
    let blob_uuid: uuid::Uuid = blob_id.parse().unwrap();
    assert!(state.db.get_by_id(blob_uuid).await.is_ok(), "blob should not be deleted");
}

#[tokio::test]
async fn test_driver_cannot_see_or_delete_others() {
    let (server, state, _b, _d) = setup().await;
    let (driver_a_token, trip_a) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;
    let (driver_b_token, trip_b) = setup_driver_with_intransit_trip_two_stops(&server, &state).await;

    let upload = upload_expense(&server, &driver_a_token, &trip_a, Some("fuel")).await;
    let sc = upload.status_code().as_u16();
    assert!(sc == 201 || sc == 202);

    // Driver B's list should not show driver A's expense.
    let list_b = server.get("/driver/api/v1/expenses")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_b_token}"))
        .await;
    assert_eq!(list_b.json::<serde_json::Value>()["total"], 0);

    let list_a = server.get("/driver/api/v1/expenses")
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_a_token}"))
        .await;
    let expense_id = list_a.json::<serde_json::Value>()["items"][0]["id"]
        .as_str().unwrap().to_string();

    let delete = server.delete(&format!("/driver/api/v1/expenses/{expense_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {driver_b_token}"))
        .await;
    assert_eq!(delete.status_code(), 404);

    let _ = trip_b; // used only to construct driver B's own trip context
}
