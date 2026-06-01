// tests/refresh_token_flow.rs
//
// End-to-end integration tests for the dispatcher PWA refresh-token flow.
// Covers: login sets cookie, refresh without access token (overnight-refresh
// regression), rotation, reuse detection, logout, and token_version kill switch.

use axum::http::header;
use axum_test::TestServer;
use ollie::{
    ai::OllamaClient,
    api,
    config::Config,
    db::DbClient,
    models::{DispatcherCredentials, DispatcherRecord, DispatcherStatus},
    storage::BlobStore,
    AppState,
};
use chrono::Utc;
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

async fn build_app() -> (TestServer, Arc<DbClient>, TempDir, TempDir) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir   = TempDir::new().unwrap();

    std::env::set_var("ADMIN_API_KEY",        "test-secret");
    std::env::set_var("DRIVER_JWT_SECRET",    "test-driver-jwt-secret-that-is-long-enough");
    std::env::set_var("DRIVER_RP_ID",         "localhost");
    std::env::set_var("DRIVER_RP_ORIGIN",     "http://localhost:3000");
    std::env::set_var("DISPATCHER_JWT_SECRET","test-dispatcher-secret-must-be-32b");

    let config = Arc::new(Config::from_env().unwrap());
    let db     = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store  = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai     = Arc::new(OllamaClient::new(
        "http://localhost:11434", "nomic-embed-text", "llama3.2", "moondream",
    ));
    let geocoding  = Arc::new(ollie::geocoding::GeocodingClient::new());
    let ors        = Arc::new(ollie::routing::RoutingClient::new(""));

    let (pipeline_tx, _rx)   = async_channel::bounded(100);
    let (geocoding_tx, _grx) = async_channel::bounded(100);
    let (routing_tx, _rrx)   = async_channel::bounded(100);

    let rp_origin = Url::parse("http://localhost:3000").unwrap();
    let webauthn  = Arc::new(
        WebauthnBuilder::new("localhost", &rp_origin).unwrap().build().unwrap(),
    );
    let auth_challenge_store = Arc::new(dashmap::DashMap::new());
    let reg_challenge_store  = Arc::new(dashmap::DashMap::new());

    let state = AppState {
        db: db.clone(), store, ai, geocoding, ors,
        pipeline_tx, geocoding_tx, routing_tx, config,
        webauthn, auth_challenge_store, reg_challenge_store,
    };
    let server = TestServer::new(api::router(state)).unwrap();
    (server, db, blob_dir, db_dir)
}

/// Seed a dispatcher with a known password and return its Uuid.
/// Uses bcrypt cost 4 so tests run fast.
async fn seed_dispatcher(db: &DbClient, email: &str, password: &str) -> Uuid {
    let id  = Uuid::new_v4();
    let now = Utc::now();

    db.upsert_dispatcher(&DispatcherRecord {
        id,
        email:      email.into(),
        name:       "Test Dispatcher".into(),
        status:     DispatcherStatus::Active,
        role:       Default::default(),
        extra_scopes: Vec::new(),
        created_at: now,
        updated_at: now,
    }).await.unwrap();

    let hash = bcrypt::hash(password, 4).unwrap();
    db.upsert_dispatcher_credentials(&DispatcherCredentials {
        dispatcher_id:   id,
        password_hash:   hash,
        token_version:   0,
        failed_attempts: 0,
        locked_until:    None,
        updated_at:      now,
    }).await.unwrap();

    id
}

/// POST /dispatch/auth/login and return the raw `set-cookie` header value.
/// Panics if the response is not 200 or the header is absent.
async fn login_get_cookie(server: &TestServer, email: &str, password: &str) -> String {
    let resp = server.post("/dispatch/auth/login")
        .json(&serde_json::json!({ "email": email, "password": password }))
        .await;
    assert_eq!(resp.status_code(), 200, "login should succeed");
    resp.headers()
        .get("set-cookie")
        .expect("set-cookie header absent on login")
        .to_str()
        .unwrap()
        .to_string()
}

/// Extract the `ollie_refresh=<value>` token from a raw set-cookie string.
fn extract_refresh_value(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .expect("empty set-cookie")
        .trim()
        .strip_prefix("ollie_refresh=")
        .expect("cookie is not ollie_refresh")
        .to_string()
}

/// Build a `Cookie: ollie_refresh=<secret>` header value.
fn cookie_header(secret: &str) -> String {
    format!("ollie_refresh={secret}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. Login sets the refresh cookie with the correct attributes.
#[tokio::test]
async fn login_sets_refresh_cookie() {
    let (server, db, _b, _d) = build_app().await;
    seed_dispatcher(&db, "disp@example.com", "correct horse").await;

    let resp = server.post("/dispatch/auth/login")
        .json(&serde_json::json!({ "email": "disp@example.com", "password": "correct horse" }))
        .await;

    assert_eq!(resp.status_code(), 200);

    let body = resp.json::<serde_json::Value>();
    assert!(
        body["token"].as_str().map(|t| !t.is_empty()).unwrap_or(false),
        "body should have a non-empty token"
    );

    let set_cookie = resp.headers()
        .get("set-cookie")
        .expect("set-cookie header must be present")
        .to_str()
        .unwrap();

    assert!(set_cookie.contains("ollie_refresh=ollr_"), "cookie value should start with ollr_");
    assert!(set_cookie.contains("HttpOnly"), "cookie should be HttpOnly");
}

/// 2. Key regression: refresh works with only the cookie — no Authorization header needed.
#[tokio::test]
async fn refresh_after_access_expiry() {
    let (server, db, _b, _d) = build_app().await;
    seed_dispatcher(&db, "disp2@example.com", "correct horse").await;

    let set_cookie = login_get_cookie(&server, "disp2@example.com", "correct horse").await;
    let secret     = extract_refresh_value(&set_cookie);

    // POST /dispatch/auth/refresh with ONLY the Cookie header (no Authorization).
    let resp = server.post("/dispatch/auth/refresh")
        .add_header(header::COOKIE, cookie_header(&secret))
        .await;

    assert_eq!(resp.status_code(), 200, "refresh should succeed without access token");

    let body = resp.json::<serde_json::Value>();
    assert!(
        body["token"].as_str().map(|t| !t.is_empty()).unwrap_or(false),
        "refresh response should contain a non-empty token"
    );

    let new_set_cookie = resp.headers()
        .get("set-cookie")
        .expect("refresh should return a new set-cookie")
        .to_str()
        .unwrap();

    assert!(new_set_cookie.contains("ollie_refresh=ollr_"), "rotated cookie should start with ollr_");
}

/// 3. Refresh rotates the secret; the old (consumed) token is rejected.
#[tokio::test]
async fn refresh_rotates_secret() {
    let (server, db, _b, _d) = build_app().await;
    seed_dispatcher(&db, "disp3@example.com", "correct horse").await;

    let sc0  = login_get_cookie(&server, "disp3@example.com", "correct horse").await;
    let tok0 = extract_refresh_value(&sc0);

    // First refresh: C0 → C1
    let r1 = server.post("/dispatch/auth/refresh")
        .add_header(header::COOKIE, cookie_header(&tok0))
        .await;
    assert_eq!(r1.status_code(), 200);
    let sc1  = r1.headers().get("set-cookie").unwrap().to_str().unwrap().to_string();
    let tok1 = extract_refresh_value(&sc1);

    assert_ne!(tok0, tok1, "rotated cookie must differ from original");

    // Replay C0 (consumed) → 401
    let r2 = server.post("/dispatch/auth/refresh")
        .add_header(header::COOKIE, cookie_header(&tok0))
        .await;
    assert_eq!(r2.status_code(), 401, "replaying consumed token should fail");
}

/// 4. Replaying a consumed token revokes the whole family; C1 also becomes invalid.
#[tokio::test]
async fn reused_refresh_revokes_family() {
    let (server, db, _b, _d) = build_app().await;
    seed_dispatcher(&db, "disp4@example.com", "correct horse").await;

    // Login → C0
    let sc0  = login_get_cookie(&server, "disp4@example.com", "correct horse").await;
    let tok0 = extract_refresh_value(&sc0);

    // C0 → C1
    let r1   = server.post("/dispatch/auth/refresh")
        .add_header(header::COOKIE, cookie_header(&tok0))
        .await;
    assert_eq!(r1.status_code(), 200);
    let tok1 = extract_refresh_value(r1.headers().get("set-cookie").unwrap().to_str().unwrap());

    // Replay C0 → 401 (triggers family revocation)
    let reuse = server.post("/dispatch/auth/refresh")
        .add_header(header::COOKIE, cookie_header(&tok0))
        .await;
    assert_eq!(reuse.status_code(), 401);

    // C1 is now also invalid (family was revoked)
    let r2 = server.post("/dispatch/auth/refresh")
        .add_header(header::COOKIE, cookie_header(&tok1))
        .await;
    assert_eq!(r2.status_code(), 401, "C1 should also be invalid after family revocation");
}

/// 5. Logout revokes the token and sets a clearing cookie (Max-Age=0).
#[tokio::test]
async fn logout_revokes_and_clears() {
    let (server, db, _b, _d) = build_app().await;
    seed_dispatcher(&db, "disp5@example.com", "correct horse").await;

    let sc     = login_get_cookie(&server, "disp5@example.com", "correct horse").await;
    let secret = extract_refresh_value(&sc);

    // Logout
    let logout = server.post("/dispatch/auth/logout")
        .add_header(header::COOKIE, cookie_header(&secret))
        .await;
    assert_eq!(logout.status_code(), 200);

    let clear_cookie = logout.headers()
        .get("set-cookie")
        .expect("logout should set a clearing cookie")
        .to_str()
        .unwrap();
    assert!(clear_cookie.contains("Max-Age=0"), "clearing cookie must have Max-Age=0");

    // Refresh with the old cookie must now fail
    let after = server.post("/dispatch/auth/refresh")
        .add_header(header::COOKIE, cookie_header(&secret))
        .await;
    assert_eq!(after.status_code(), 401, "refresh after logout should fail");
}

/// 6. Bumping token_version in the DB invalidates an existing refresh token.
#[tokio::test]
async fn token_version_bump_kills_refresh() {
    let (server, db, _b, _d) = build_app().await;
    let id = seed_dispatcher(&db, "disp6@example.com", "correct horse").await;

    let sc     = login_get_cookie(&server, "disp6@example.com", "correct horse").await;
    let secret = extract_refresh_value(&sc);

    // Bump token_version
    let mut creds = db.get_dispatcher_credentials(id).await.unwrap().unwrap();
    creds.token_version += 1;
    creds.updated_at     = Utc::now();
    db.upsert_dispatcher_credentials(&creds).await.unwrap();

    // Refresh with the old cookie (token_version mismatch) → 401
    let resp = server.post("/dispatch/auth/refresh")
        .add_header(header::COOKIE, cookie_header(&secret))
        .await;
    assert_eq!(resp.status_code(), 401, "refresh must fail after token_version bump");
}
