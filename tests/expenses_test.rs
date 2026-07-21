// tests/expenses_test.rs
//
// Integration tests for fleet REST expense create/list/get (#233 task 5).

use axum_test::TestServer;
use ollie::{
    ai::OllamaClient,
    api,
    config::Config,
    db::DbClient,
    storage::BlobStore,
    AppState,
};
use std::sync::Arc;
use tempfile::TempDir;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

#[allow(clippy::type_complexity)]
async fn test_server_with_state() -> (
    TestServer,
    TempDir,
    TempDir,
    async_channel::Receiver<uuid::Uuid>,
    AppState,
) {
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
    (server, blob_dir, db_dir, rx, state)
}

/// Create a dispatcher user via the users API (owner-authenticated) and log in,
/// returning the dispatcher's JWT.
async fn create_dispatcher_and_login(server: &TestServer, owner: &str) -> String {
    let email = "dispatcher@example.com";
    let password = "dispatcher-password-123";
    let create = server.post("/fleet/api/v1/users")
        .authorization_bearer(owner)
        .json(&serde_json::json!({
            "email": email, "name": "Dispatcher", "password": password, "role": "dispatcher",
        }))
        .await;
    assert_eq!(create.status_code(), 201, "dispatcher create failed");
    let login = server.post("/fleet/auth/login")
        .json(&serde_json::json!({ "email": email, "password": password }))
        .await;
    assert_eq!(login.status_code(), 200, "dispatcher login failed");
    login.json::<serde_json::Value>()["token"].as_str().unwrap().to_string()
}

async fn test_server() -> (TestServer, TempDir, TempDir, async_channel::Receiver<uuid::Uuid>) {
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

    let (pipeline_tx, rx) = async_channel::bounded(100);
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
        db, store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
        webauthn, auth_challenge_store, reg_challenge_store,
    };
    let server = TestServer::new(api::router(state)).unwrap();
    (server, blob_dir, db_dir, rx)
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

async fn create_expense(server: &TestServer, token: &str, category: &str) -> String {
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(token)
        .json(&serde_json::json!({ "category": category }))
        .await;
    assert_eq!(resp.status_code(), 201);
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

async fn create_truck(server: &TestServer, token: &str) -> String {
    let resp = server.post("/fleet/api/v1/trucks")
        .authorization_bearer(token)
        .json(&serde_json::json!({ "unit_number": format!("T-{}", uuid::Uuid::new_v4()) }))
        .await;
    assert_eq!(resp.status_code(), 201);
    resp.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_create_and_get_expense() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "fuel" }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "submitted");
    assert_eq!(body["category"], "fuel");
    assert!(body["amount"].is_null());
    assert!(body["submitted_by"].as_str().unwrap().starts_with("fleet_user:"));
    let id = body["id"].as_str().unwrap();

    let got = server.get(&format!("/fleet/api/v1/expenses/{id}"))
        .authorization_bearer(&token).await;
    assert_eq!(got.status_code(), 200);
    assert_eq!(got.json::<serde_json::Value>()["id"], *id);
}

#[tokio::test]
async fn test_create_expense_validates_links() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    // unknown driver -> 400
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "category": "repair",
            "driver_id": uuid::Uuid::new_v4(),
        })).await;
    assert_eq!(resp.status_code(), 400);
    // equipment_type without equipment_id -> 400
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "repair", "equipment_type": "truck" }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_list_expenses_filters() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    for cat in ["fuel", "tolls"] {
        let r = server.post("/fleet/api/v1/expenses")
            .authorization_bearer(&token)
            .json(&serde_json::json!({ "category": cat })).await;
        assert_eq!(r.status_code(), 201);
    }
    let all = server.get("/fleet/api/v1/expenses").authorization_bearer(&token).await;
    assert_eq!(all.status_code(), 200);
    assert_eq!(all.json::<serde_json::Value>()["total"], 2);
    let fuel = server.get("/fleet/api/v1/expenses?category=fuel")
        .authorization_bearer(&token).await;
    assert_eq!(fuel.json::<serde_json::Value>()["total"], 1);
    let queue = server.get("/fleet/api/v1/expenses?status=submitted")
        .authorization_bearer(&token).await;
    assert_eq!(queue.json::<serde_json::Value>()["total"], 2);
}

#[tokio::test]
async fn test_expenses_require_auth() {
    let (server, _d1, _d2, _rx) = test_server().await;
    assert_eq!(server.get("/fleet/api/v1/expenses").await.status_code(), 401);
}

#[tokio::test]
async fn test_create_expense_rejects_negative_amount() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "fuel", "amount": -5.0 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_get_expense_not_found() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let resp = server.get(&format!("/fleet/api/v1/expenses/{}", uuid::Uuid::new_v4()))
        .authorization_bearer(&token).await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_create_expense_rejects_unknown_category() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let resp = server.post("/fleet/api/v1/expenses")
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "category": "not-a-real-category" }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_list_expenses_rejects_unknown_status_and_category() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let _ = create_expense(&server, &token, "fuel").await;
    let resp = server.get("/fleet/api/v1/expenses?status=not-a-status")
        .authorization_bearer(&token).await;
    assert_eq!(resp.status_code(), 400);
    let resp = server.get("/fleet/api/v1/expenses?category=not-a-category")
        .authorization_bearer(&token).await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_review_full_partial_and_reject_derivations() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let id = create_expense(&server, &token, "fuel").await;

    // Partial approval on company funds -> deduction of the denied portion.
    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 100.0, "approved_amount": 80.0,
            "payment_method": "company", "review_note": "20 was personal snacks"
        })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "reviewed");
    assert_eq!(body["disposition"], "partial");
    assert!((body["deduction"].as_f64().unwrap() - 20.0).abs() < 1e-9);
    assert!(body.get("reimbursement").is_none() || body["reimbursement"].is_null());
    assert!(body["reviewed_by"].as_str().is_some());

    // Re-review is allowed while unsettled: flip to personal full approval.
    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 100.0, "approved_amount": 100.0, "payment_method": "personal"
        })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["disposition"], "approved");
    assert!((body["reimbursement"].as_f64().unwrap() - 100.0).abs() < 1e-9);
}

#[tokio::test]
async fn test_review_validation() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let id = create_expense(&server, &token, "fuel").await;
    // approved > amount -> 400
    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 50.0, "approved_amount": 60.0, "payment_method": "company"
        })).await;
    assert_eq!(resp.status_code(), 400);
    // negative amount -> 400
    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": -5.0, "approved_amount": 0.0, "payment_method": "company"
        })).await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_review_clears_suggestions() {
    let (server, _d1, _d2, _rx, state) = test_server_with_state().await;
    let token = setup_owner(&server).await;
    let id = create_expense(&server, &token, "fuel").await;
    let uid: uuid::Uuid = id.parse().unwrap();
    state.db.update_expense_suggestions(
        uid, Some(42.0), Some("2026-07-01".into()), Some("Loves".into()), Some("1234".into()),
    ).await.unwrap();

    let resp = server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 42.0, "approved_amount": 42.0, "payment_method": "company"
        })).await;
    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert!(body["suggested_amount"].is_null());
    assert!(body["suggested_vendor"].is_null());
}

#[tokio::test]
async fn test_settled_expense_is_locked() {
    let (server, _d1, _d2, _rx, state) = test_server_with_state().await;
    let token = setup_owner(&server).await;
    let id = create_expense(&server, &token, "fuel").await;
    let uid: uuid::Uuid = id.parse().unwrap();
    // Simulate the future settlements feature.
    let mut rec = state.db.get_expense_by_id(uid).await.unwrap();
    rec.status = "settled".parse().unwrap();
    rec.settlement_id = Some(uuid::Uuid::new_v4());
    state.db.update_expense(&rec).await.unwrap();

    for resp in [
        server.post(&format!("/fleet/api/v1/expenses/{id}/review"))
            .authorization_bearer(&token)
            .json(&serde_json::json!({
                "amount": 1.0, "approved_amount": 1.0, "payment_method": "company"
            })).await,
        server.patch(&format!("/fleet/api/v1/expenses/{id}"))
            .authorization_bearer(&token)
            .json(&serde_json::json!({ "vendor": "x" })).await,
        server.delete(&format!("/fleet/api/v1/expenses/{id}"))
            .authorization_bearer(&token).await,
    ] {
        assert_eq!(resp.status_code(), 409, "settled record must reject mutation");
    }
}

#[tokio::test]
async fn test_dispatcher_scope_enforcement() {
    // Owner creates a dispatcher user; dispatcher can create + edit own submitted,
    // cannot review, cannot edit others' records.
    let (server, _d1, _d2, _rx) = test_server().await;
    let owner = setup_owner(&server).await;
    let disp = create_dispatcher_and_login(&server, &owner).await;
    let own = create_expense(&server, &disp, "tolls").await;
    let other = create_expense(&server, &owner, "fuel").await;

    // Dispatcher edits own submitted record: OK.
    let r = server.patch(&format!("/fleet/api/v1/expenses/{own}"))
        .authorization_bearer(&disp)
        .json(&serde_json::json!({ "vendor": "PA Turnpike" })).await;
    assert_eq!(r.status_code(), 200);
    // Dispatcher edits someone else's record: 403.
    let r = server.patch(&format!("/fleet/api/v1/expenses/{other}"))
        .authorization_bearer(&disp)
        .json(&serde_json::json!({ "vendor": "nope" })).await;
    assert_eq!(r.status_code(), 403);
    // Dispatcher cannot review: 403.
    let r = server.post(&format!("/fleet/api/v1/expenses/{own}/review"))
        .authorization_bearer(&disp)
        .json(&serde_json::json!({
            "amount": 10.0, "approved_amount": 10.0, "payment_method": "company"
        })).await;
    assert_eq!(r.status_code(), 403);
    // Dispatcher deletes own submitted: 204. Owner's record: 403.
    assert_eq!(server.delete(&format!("/fleet/api/v1/expenses/{own}"))
        .authorization_bearer(&disp).await.status_code(), 204);
    assert_eq!(server.delete(&format!("/fleet/api/v1/expenses/{other}"))
        .authorization_bearer(&disp).await.status_code(), 403);
}

#[tokio::test]
async fn test_money_fields_require_approve_scope_on_patch() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let owner = setup_owner(&server).await;
    let disp = create_dispatcher_and_login(&server, &owner).await;
    let own = create_expense(&server, &disp, "fuel").await;
    let r = server.patch(&format!("/fleet/api/v1/expenses/{own}"))
        .authorization_bearer(&disp)
        .json(&serde_json::json!({ "amount": 99.0 })).await;
    assert_eq!(r.status_code(), 403);
}

#[tokio::test]
async fn test_maintenance_expense_crosslink_and_cost_mirror() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    // Create a truck (copy the minimal truck-create request shape from
    // tests/integration_test.rs truck tests).
    let truck_id = create_truck(&server, &token).await; // local helper
    // Expense with equipment link, then a maintenance record linked to it.
    let exp_id = {
        let r = server.post("/fleet/api/v1/expenses")
            .authorization_bearer(&token)
            .json(&serde_json::json!({
                "category": "repair", "equipment_type": "truck", "equipment_id": truck_id,
            })).await;
        assert_eq!(r.status_code(), 201);
        r.json::<serde_json::Value>()["id"].as_str().unwrap().to_string()
    };
    let m = server.post("/fleet/api/v1/maintenance")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "equipment_type": "truck", "equipment_id": truck_id,
            "service_date": "2026-07-20", "category": "repair",
            "description": "roadside tire replacement",
            "expense_id": exp_id,
        })).await;
    assert_eq!(m.status_code(), 201);
    let mid = m.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    // Cross-link written back onto the expense.
    let e = server.get(&format!("/fleet/api/v1/expenses/{exp_id}"))
        .authorization_bearer(&token).await;
    assert_eq!(e.json::<serde_json::Value>()["maintenance_id"], *mid);

    // Review mirrors amount onto maintenance.cost.
    let r = server.post(&format!("/fleet/api/v1/expenses/{exp_id}/review"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "amount": 612.0, "approved_amount": 612.0, "payment_method": "company"
        })).await;
    assert_eq!(r.status_code(), 200);
    let got = server.get(&format!("/fleet/api/v1/maintenance/{mid}"))
        .authorization_bearer(&token).await;
    assert_eq!(got.json::<serde_json::Value>()["cost"], 612.0);

    // Direct cost edits on a linked maintenance record are rejected.
    let bad = server.patch(&format!("/fleet/api/v1/maintenance/{mid}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "cost": 1.0 })).await;
    assert_eq!(bad.status_code(), 400);
}

#[tokio::test]
async fn test_patch_linking_expense_rejects_cost_in_same_body() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let truck_id = create_truck(&server, &token).await;
    let m = server.post("/fleet/api/v1/maintenance")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "equipment_type": "truck", "equipment_id": truck_id,
            "service_date": "2026-07-20", "category": "repair",
            "description": "unlinked oil change",
        })).await;
    assert_eq!(m.status_code(), 201);
    let mid = m.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let exp_id = create_expense(&server, &token, "repair").await;

    let bad = server.patch(&format!("/fleet/api/v1/maintenance/{mid}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "expense_id": exp_id, "cost": 999.0 })).await;
    assert_eq!(bad.status_code(), 400);
}

#[tokio::test]
async fn test_expense_cannot_double_link() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let truck_id = create_truck(&server, &token).await;
    let exp_id = create_expense(&server, &token, "repair").await;

    let a = server.post("/fleet/api/v1/maintenance")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "equipment_type": "truck", "equipment_id": truck_id,
            "service_date": "2026-07-20", "category": "repair",
            "description": "first repair", "expense_id": exp_id,
        })).await;
    assert_eq!(a.status_code(), 201);

    let b = server.post("/fleet/api/v1/maintenance")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "equipment_type": "truck", "equipment_id": truck_id,
            "service_date": "2026-07-20", "category": "repair",
            "description": "second repair, same expense", "expense_id": exp_id,
        })).await;
    assert_eq!(b.status_code(), 400);

    let c = server.post("/fleet/api/v1/maintenance")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "equipment_type": "truck", "equipment_id": truck_id,
            "service_date": "2026-07-20", "category": "repair",
            "description": "third repair, unlinked",
        })).await;
    assert_eq!(c.status_code(), 201);
    let cid = c.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let patch = server.patch(&format!("/fleet/api/v1/maintenance/{cid}"))
        .authorization_bearer(&token)
        .json(&serde_json::json!({ "expense_id": exp_id })).await;
    assert_eq!(patch.status_code(), 400);
}

#[tokio::test]
async fn test_warranty_maintenance_needs_no_expense() {
    let (server, _d1, _d2, _rx) = test_server().await;
    let token = setup_owner(&server).await;
    let truck_id = create_truck(&server, &token).await;
    let m = server.post("/fleet/api/v1/maintenance")
        .authorization_bearer(&token)
        .json(&serde_json::json!({
            "equipment_type": "truck", "equipment_id": truck_id,
            "service_date": "2026-07-20", "category": "repair",
            "description": "warranty turbo replacement", "cost": 0.0,
        })).await;
    assert_eq!(m.status_code(), 201);
    let body: serde_json::Value = m.json();
    assert_eq!(body["cost"], 0.0);
    assert!(body["expense_id"].is_null());
}
