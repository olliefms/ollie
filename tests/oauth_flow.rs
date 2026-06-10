// tests/oauth_flow.rs
//
// End-to-end integration tests for the OAuth 2.1 Authorization Server (Task 10).
// Covers: router mount, WWW-Authenticate header on MCP 401, metadata endpoints,
// full authorization-code+PKCE dance, refresh, and bad-PKCE rejection.

use axum::http::header;
use axum_test::TestServer;
use base64::Engine;
use ollie::{
    ai::OllamaClient,
    api,
    config::Config,
    db::DbClient,
    models::{FleetUserApiKey, FleetUserCredentials, FleetUserRecord, FleetUserStatus},
    storage::BlobStore,
    AppState,
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;
use webauthn_rs::prelude::{Url, WebauthnBuilder};

// ---------------------------------------------------------------------------
// Harness (mirrors refresh_token_flow.rs)
// ---------------------------------------------------------------------------

const TEST_BASE_URL: &str = "https://test.example.com";

async fn build_app() -> (TestServer, Arc<DbClient>, TempDir, TempDir) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir   = TempDir::new().unwrap();

    std::env::set_var("DRIVER_JWT_SECRET",       "test-driver-jwt-secret-that-is-long-enough");
    std::env::set_var("DRIVER_RP_ID",            "localhost");
    std::env::set_var("DRIVER_RP_ORIGIN",        "http://localhost:3000");
    std::env::set_var("FLEET_JWT_SECRET",   "test-fleet_user-secret-must-be-32b");
    std::env::set_var("OLLIE_PUBLIC_BASE_URL",   TEST_BASE_URL);

    let config = Arc::new(Config::from_env().unwrap());
    let db     = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store  = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai     = Arc::new(OllamaClient::new(
        // Deliberately unreachable: integration tests must not depend on a live
        // Ollama (a real one on :11434 feeds wrong-dim embeddings into the test schema).
        "http://127.0.0.1:1", "nomic-embed-text", "llama3.2", "moondream",
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

async fn seed_fleet_user(db: &DbClient, email: &str, password: &str) -> Uuid {
    let id  = Uuid::new_v4();
    let now = Utc::now();
    db.upsert_fleet_user(&FleetUserRecord {
        id,
        email:      email.into(),
        name:       "Test Dispatcher".into(),
        status:     FleetUserStatus::Active,
        role:       Default::default(),
        extra_scopes: Vec::new(),
        created_at: now,
        updated_at: now,
    }).await.unwrap();
    let hash = bcrypt::hash(password, 4).unwrap();
    db.upsert_fleet_user_credentials(&FleetUserCredentials {
        fleet_user_id:   id,
        password_hash:   hash,
        token_version:   0,
        failed_attempts: 0,
        locked_until:    None,
        updated_at:      now,
    }).await.unwrap();
    id
}

/// PKCE S256: base64url_nopad(SHA256(verifier))
fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. POST /fleet/mcp with no auth → 401 with WWW-Authenticate containing
///    resource_metadata pointing at the PRM URL.
#[tokio::test]
async fn mcp_401_has_www_authenticate() {
    let (server, _db, _b, _d) = build_app().await;

    let resp = server.post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }))
        .await;

    assert_eq!(resp.status_code(), 401, "unauthenticated MCP must be 401");

    let www_auth = resp.headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate header must be present on MCP 401")
        .to_str()
        .unwrap();

    assert!(www_auth.contains("resource_metadata="), "header must contain resource_metadata");
    assert!(
        www_auth.contains("/.well-known/oauth-protected-resource/fleet/mcp"),
        "header must reference the PRM URL; got: {www_auth}",
    );
}

/// 2. GET /fleet/api/v1/loads with no auth → 401 with NO WWW-Authenticate.
///    REST endpoints must keep bare 401s.
#[tokio::test]
async fn rest_401_has_no_www_authenticate() {
    let (server, _db, _b, _d) = build_app().await;

    let resp = server.get("/fleet/api/v1/loads").await;

    assert_eq!(resp.status_code(), 401, "unauthenticated REST must be 401");
    assert!(
        resp.headers().get(header::WWW_AUTHENTICATE).is_none(),
        "REST 401 must NOT carry WWW-Authenticate",
    );
}

/// 3. OAuth metadata endpoints return 200 with the expected fields.
#[tokio::test]
async fn metadata_endpoints() {
    let (server, _db, _b, _d) = build_app().await;

    // Authorization-server metadata
    let as_resp = server.get("/.well-known/oauth-authorization-server").await;
    assert_eq!(as_resp.status_code(), 200, "authorization-server metadata must be 200");
    let as_body = as_resp.json::<serde_json::Value>();

    let methods = as_body["code_challenge_methods_supported"]
        .as_array()
        .expect("code_challenge_methods_supported must be an array");
    assert_eq!(methods, &[serde_json::json!("S256")], "only S256 must be listed");

    let token_endpoint = as_body["token_endpoint"]
        .as_str()
        .expect("token_endpoint must be a string");
    assert!(token_endpoint.ends_with("/oauth/token"), "token_endpoint must end with /oauth/token; got {token_endpoint}");

    // Protected-resource metadata
    let prm_resp = server.get("/.well-known/oauth-protected-resource/fleet/mcp").await;
    assert_eq!(prm_resp.status_code(), 200, "PRM endpoint must be 200");
    let prm_body = prm_resp.json::<serde_json::Value>();

    let as_list = prm_body["authorization_servers"]
        .as_array()
        .expect("authorization_servers must be an array");
    assert!(!as_list.is_empty(), "authorization_servers must be non-empty");
}

/// 4. Full OAuth dance: register → authorize → token → MCP call → refresh.
#[tokio::test]
async fn full_oauth_dance() {
    let (server, db, _b, _d) = build_app().await;

    let email    = "oauth_dance@example.com";
    let password = "hunter2_long_enough_password";
    seed_fleet_user(&db, email, password).await;

    // 4a. Register client
    let reg_resp = server.post("/oauth/register")
        .json(&serde_json::json!({
            "redirect_uris": ["http://127.0.0.1:9/cb"],
            "client_name": "Test OAuth Client",
        }))
        .await;
    assert_eq!(reg_resp.status_code(), 201, "client registration must be 201");
    let reg_body = reg_resp.json::<serde_json::Value>();
    let client_id = reg_body["client_id"].as_str().expect("client_id must be present").to_string();

    // 4b. Build PKCE
    // verifier must be >= 43 chars; use a fixed base64url string
    let verifier  = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = pkce_challenge(verifier);
    let redirect_uri = "http://127.0.0.1:9/cb";
    let state_val    = "xyzabc";

    // POST to /oauth/authorize as a form
    let auth_resp = server.post("/oauth/authorize")
        .form(&[
            ("response_type",        "code"),
            ("client_id",            &client_id),
            ("redirect_uri",         redirect_uri),
            ("code_challenge",       &challenge),
            ("code_challenge_method","S256"),
            ("state",                state_val),
            ("email",                email),
            ("password",             password),
            ("decision",             "allow"),
        ])
        .await;
    assert_eq!(auth_resp.status_code(), 302, "authorize must redirect; body: {:?}", auth_resp.text());

    let location = auth_resp.headers()
        .get(header::LOCATION)
        .expect("Location header must be present")
        .to_str()
        .unwrap();
    assert!(location.contains("code="), "Location must contain code param; got: {location}");

    let code = location
        .split('?')
        .nth(1)
        .unwrap_or("")
        .split('&')
        .find(|p| p.starts_with("code="))
        .and_then(|p| p.strip_prefix("code="))
        .expect("code param not found in Location");

    // 4c. Exchange code for tokens
    let token_resp = server.post("/oauth/token")
        .form(&[
            ("grant_type",    "authorization_code"),
            ("code",          code),
            ("redirect_uri",  redirect_uri),
            ("client_id",     &client_id),
            ("code_verifier", verifier),
        ])
        .await;
    assert_eq!(token_resp.status_code(), 200, "token exchange must be 200; body: {:?}", token_resp.text());

    let token_body = token_resp.json::<serde_json::Value>();
    let access_token = token_body["access_token"].as_str().expect("access_token must be present").to_string();
    let refresh_token = token_body["refresh_token"].as_str().expect("refresh_token must be present").to_string();

    assert!(!access_token.is_empty(),  "access_token must be non-empty");
    assert!(!refresh_token.is_empty(), "refresh_token must be non-empty");
    assert_eq!(token_body["token_type"].as_str(), Some("Bearer"), "token_type must be Bearer");
    assert_eq!(token_body["expires_in"].as_i64(), Some(28800), "expires_in must be 28800");

    // 4d. Use access_token at /fleet/mcp — should NOT be 401
    let mcp_resp = server.post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {access_token}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "t", "version": "1" }
            }
        }))
        .await;
    assert_ne!(
        mcp_resp.status_code(),
        401,
        "MCP with valid OAuth token must not be 401; status={}, body={:?}",
        mcp_resp.status_code(),
        mcp_resp.text(),
    );
    assert_eq!(mcp_resp.status_code(), 200, "MCP initialize with valid token must be 200");

    // 4e. Refresh the access token
    let refresh_resp = server.post("/oauth/token")
        .form(&[
            ("grant_type",    "refresh_token"),
            ("refresh_token", &refresh_token),
            ("client_id",     &client_id),
        ])
        .await;
    assert_eq!(refresh_resp.status_code(), 200, "refresh grant must be 200");

    let refresh_body = refresh_resp.json::<serde_json::Value>();
    let new_access  = refresh_body["access_token"].as_str().expect("new access_token must be present").to_string();
    let new_refresh = refresh_body["refresh_token"].as_str().expect("new refresh_token must be present").to_string();

    assert!(!new_access.is_empty(),  "refreshed access_token must be non-empty");
    assert!(!new_refresh.is_empty(), "rotated refresh_token must be non-empty");
    assert_ne!(new_refresh, refresh_token, "refresh_token must be rotated");

    // 4f. A refresh with a client_id that doesn't match the token's bound
    //     client must be rejected (OAuth 2.1 §4.1.3). The check runs before
    //     rotation, so new_refresh is left intact.
    let bad_client = server.post("/oauth/token")
        .form(&[
            ("grant_type",    "refresh_token"),
            ("refresh_token", &new_refresh),
            ("client_id",     "00000000-0000-0000-0000-000000000000"),
        ])
        .await;
    assert_eq!(bad_client.status_code(), 400, "refresh with mismatched client_id must be 400");
    assert_eq!(
        bad_client.json::<serde_json::Value>()["error"].as_str(),
        Some("invalid_grant"),
        "mismatched client_id must yield invalid_grant",
    );
}

/// 5b. Static olld_ API key authenticates /fleet/mcp after mcp_router extraction.
///     Regression guard for the headless/scripting auth path.
#[tokio::test]
async fn olld_api_key_authenticates_mcp() {
    let (server, db, _b, _d) = build_app().await;

    let email    = "apikey_mcp@example.com";
    let password = "hunter2_long_enough_password";
    let fleet_user_id = seed_fleet_user(&db, email, password).await;

    let plaintext  = format!("olld_{}", "s3cr3tSuffix1234567890abcdef");
    let key_hash   = hex::encode(Sha256::digest(plaintext.as_bytes()));
    let key_prefix = plaintext.chars().take(12).collect::<String>();
    let now        = Utc::now();

    db.insert_fleet_user_api_key(&FleetUserApiKey {
        id:           Uuid::new_v4(),
        fleet_user_id,
        label:        "test".into(),
        key_hash,
        key_prefix,
        created_at:   now,
        expires_at:   now + chrono::Duration::days(30),
        revoked_at:   None,
        last_used_at: None,
    }).await.unwrap();

    let mcp_resp = server.post("/fleet/mcp")
        .add_header(header::ACCEPT, "application/json, text/event-stream")
        .add_header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "t", "version": "1" }
            }
        }))
        .await;

    assert_eq!(
        mcp_resp.status_code(),
        200,
        "olld_ API key must authenticate /fleet/mcp; status={}, body={:?}",
        mcp_resp.status_code(),
        mcp_resp.text(),
    );
}

/// 5. Bad PKCE verifier → 400 with error == "invalid_grant".
#[tokio::test]
async fn token_rejects_bad_pkce() {
    let (server, db, _b, _d) = build_app().await;

    let email    = "bad_pkce@example.com";
    let password = "hunter2_long_enough_password";
    seed_fleet_user(&db, email, password).await;

    // Register
    let reg_resp = server.post("/oauth/register")
        .json(&serde_json::json!({
            "redirect_uris": ["http://127.0.0.1:9/cb"],
            "client_name": "PKCE test client",
        }))
        .await;
    assert_eq!(reg_resp.status_code(), 201);
    let client_id = reg_resp.json::<serde_json::Value>()["client_id"].as_str().unwrap().to_string();

    // Authorize with a real PKCE pair
    let verifier  = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = pkce_challenge(verifier);
    let redirect_uri = "http://127.0.0.1:9/cb";

    let auth_resp = server.post("/oauth/authorize")
        .form(&[
            ("response_type",        "code"),
            ("client_id",            &client_id),
            ("redirect_uri",         redirect_uri),
            ("code_challenge",       &challenge),
            ("code_challenge_method","S256"),
            ("state",                "s"),
            ("email",                email),
            ("password",             password),
            ("decision",             "allow"),
        ])
        .await;
    assert_eq!(auth_resp.status_code(), 302);

    let location = auth_resp.headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    let code = location
        .split('?')
        .nth(1)
        .unwrap_or("")
        .split('&')
        .find(|p| p.starts_with("code="))
        .and_then(|p| p.strip_prefix("code="))
        .expect("code param not found");

    // Token exchange with WRONG verifier
    let token_resp = server.post("/oauth/token")
        .form(&[
            ("grant_type",    "authorization_code"),
            ("code",          code),
            ("redirect_uri",  redirect_uri),
            ("client_id",     &client_id),
            ("code_verifier", "this-is-definitely-wrong-verifier-000000000000"),
        ])
        .await;

    assert_eq!(token_resp.status_code(), 400, "wrong code_verifier must be 400");
    let body = token_resp.json::<serde_json::Value>();
    assert_eq!(
        body["error"].as_str(),
        Some("invalid_grant"),
        "error must be invalid_grant; got {:?}", body,
    );
}
