# OAuth 2.1 Authorization Server for `/dispatch/mcp` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Claude Desktop add Ollie's `/dispatch/mcp` as a remote OAuth connector by implementing the MCP authorization surface: a `401` + `WWW-Authenticate`, discovery metadata (RFC 9728 + 8414), Dynamic Client Registration (RFC 7591), and an authorization-code + PKCE flow whose access token is the existing dispatcher JWT and whose refresh token reuses Plan 1's service.

**Architecture:** A new portal-agnostic `src/api/oauth/` module mounted on public (unauthenticated) routes. It owns two new LanceDB tables (`oauth_clients`, `authorization_codes`), the discovery/DCR/authorize/token handlers, and a server-rendered minimal login+consent page. The `/token` endpoint mints a `DispatcherClaims` JWT (unchanged hot path) + a refresh token (Plan 1's `refresh_tokens::issue`, now with `client_id` set). A `map_response` layer scoped to the `/dispatch/mcp` route decorates its `401` with `WWW-Authenticate`. The AS is parameterized by a resource descriptor; only `dispatcher` is wired here (driver MCP is future work).

**Tech Stack:** Rust, axum 0.7, LanceDB 0.29, chrono, sha2, uuid, base64, axum-test. **Depends on Plan 1** (`refresh-tokens-foundation`) being merged — uses `crate::api::refresh_tokens`.

---

## Conventions (read before starting)

Same as Plan 1: hand-formatted (never `cargo fmt`), `DataType::Utf8` only in Rust schemas (never in SQL strings — both new tables are created fresh via `open_or_create`, no `CAST(NULL …)` path), explicit typed arrays for null columns, verify with `cargo test` + `cargo clippy --all-targets -- -D warnings`, TDD per task, frequent commits.

**Single-page login+consent simplification:** rather than a separate AS-session cookie + remembered-grant table (spec §authorize), this plan uses ONE server-rendered page that authenticates (email+password) and consents (Allow/Deny) in a single POST. This is simpler and the credential prompt only appears at connector setup and after a 14-day refresh lapse — both rare. The `oauth_consent_grants` table is therefore **not** built (it was optional in the spec). Revisit if per-scope consent (#243) lands.

## File Structure

- **Create** `src/models/oauth_client.rs`, `src/models/authorization_code.rs` — record structs.
- **Modify** `src/models/mod.rs` — register both.
- **Modify** `src/db/mod.rs` — two schemas, two `open_or_create` calls, two `DbClient` fields.
- **Create** `src/db/oauth_client_ops.rs`, `src/db/authorization_code_ops.rs` — CRUD (+ consume-once for codes).
- **Create** `src/api/oauth/mod.rs` — router assembly + the `ResourceDescriptor` + shared types/errors (`OauthError`).
- **Create** `src/api/oauth/metadata.rs` — PRM (RFC 9728) + AS metadata (RFC 8414) handlers.
- **Create** `src/api/oauth/register.rs` — DCR (RFC 7591).
- **Create** `src/api/oauth/authorize.rs` — `GET`/`POST /oauth/authorize` + the HTML page.
- **Create** `src/api/oauth/token.rs` — `POST /oauth/token` (authorization_code + refresh_token grants).
- **Modify** `src/api/mod.rs` — register `pub mod oauth;`, merge the oauth router into the root (public), and add the `WWW-Authenticate` `map_response` layer to the `/dispatch/mcp` route.

---

## Task 1: `OAuthClient` + `AuthorizationCode` models

**Files:**
- Create: `src/models/oauth_client.rs`, `src/models/authorization_code.rs`
- Modify: `src/models/mod.rs`

- [ ] **Step 1: Write `oauth_client.rs`**

```rust
// src/models/oauth_client.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A dynamically-registered public OAuth client (RFC 7591). No secret (PKCE).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OAuthClient {
    pub id: Uuid,
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
    pub created_at: DateTime<Utc>,
}
```

- [ ] **Step 2: Write `authorization_code.rs`**

```rust
// src/models/authorization_code.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A one-time authorization code bound to a PKCE challenge and a subject.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthorizationCode {
    pub code_hash: String,
    pub client_id: Uuid,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub subject_type: String,
    pub subject_id: Uuid,
    pub resource: String,
    pub scope: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}
```

- [ ] **Step 3: Register both in `src/models/mod.rs`**

```rust
mod oauth_client;
pub use oauth_client::*;
mod authorization_code;
pub use authorization_code::*;
```

- [ ] **Step 4: Build + commit**

```bash
cargo build
cargo clippy --all-targets -- -D warnings
git add src/models/oauth_client.rs src/models/authorization_code.rs src/models/mod.rs
git commit -m "feat(models): add OAuthClient and AuthorizationCode records"
```

---

## Task 2: LanceDB schemas + table registration

**Files:**
- Modify: `src/db/mod.rs`

`redirect_uris: Vec<String>` is stored as a JSON string column (LanceDB list-of-string handling is heavier than we need for a tiny set; serde_json round-trip is simplest and matches how the codebase stores `tags` as a string in the blob schema).

- [ ] **Step 1: Add both schemas** next to `refresh_token_schema()`:

```rust
pub fn oauth_client_schema() -> std::sync::Arc<arrow_schema::Schema> {
    std::sync::Arc::new(arrow_schema::Schema::new(vec![
        arrow_schema::Field::new("id", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("client_name", arrow_schema::DataType::Utf8, true),
        // JSON-encoded Vec<String>
        arrow_schema::Field::new("redirect_uris", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("created_at", arrow_schema::DataType::Utf8, false),
    ]))
}

pub fn authorization_code_schema() -> std::sync::Arc<arrow_schema::Schema> {
    std::sync::Arc::new(arrow_schema::Schema::new(vec![
        arrow_schema::Field::new("code_hash", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("client_id", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("redirect_uri", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("code_challenge", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("subject_type", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("subject_id", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("resource", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("scope", arrow_schema::DataType::Utf8, true),
        arrow_schema::Field::new("created_at", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("expires_at", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("consumed_at", arrow_schema::DataType::Utf8, true),
    ]))
}
```

- [ ] **Step 2: Add fields to `DbClient`**:

```rust
    pub oauth_client_table: Table,
    pub authorization_code_table: Table,
```

- [ ] **Step 3: Open/create in `new()`**:

```rust
        let oauth_client_table = open_or_create(&conn, "oauth_clients", oauth_client_schema()).await?;
        let authorization_code_table = open_or_create(&conn, "authorization_codes", authorization_code_schema()).await?;
```

Add `oauth_client_table,` and `authorization_code_table,` to the struct construction.

- [ ] **Step 4: Build + commit**

```bash
cargo build
cargo clippy --all-targets -- -D warnings
git add src/db/mod.rs
git commit -m "feat(db): register oauth_clients and authorization_codes tables"
```

---

## Task 3: `oauth_clients` DB ops

**Files:**
- Create: `src/db/oauth_client_ops.rs`
- Modify: `src/db/mod.rs` (`mod oauth_client_ops;`)

- [ ] **Step 1: Write the ops + tests**

```rust
// src/db/oauth_client_ops.rs
use crate::{
    db::{oauth_client_schema, DbClient},
    error::AppError,
    models::OAuthClient,
};
use arrow_array::{Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_oauth_client(&self, c: &OAuthClient) -> Result<(), AppError> {
        let batch = client_to_batch(c)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], oauth_client_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.oauth_client_table.add(reader).execute().await
            .map(|_| ()).map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_oauth_client(&self, id: Uuid) -> Result<Option<OAuthClient>, AppError> {
        let id = id.to_string();
        let stream = self.oauth_client_table.query()
            .only_if(format!("id = '{id}'"))
            .execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut v = batches_to_clients(collect_stream(stream).await?)?;
        Ok(v.pop())
    }
}

fn client_to_batch(c: &OAuthClient) -> Result<RecordBatch, AppError> {
    let id = c.id.to_string();
    let created = c.created_at.to_rfc3339();
    let uris = serde_json::to_string(&c.redirect_uris).map_err(|e| AppError::Internal(e.to_string()))?;
    RecordBatch::try_new(oauth_client_schema(), vec![
        Arc::new(StringArray::from(vec![id.as_str()])),
        Arc::new(StringArray::from(vec![c.client_name.as_deref()])),
        Arc::new(StringArray::from(vec![uris.as_str()])),
        Arc::new(StringArray::from(vec![created.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_clients(batches: Vec<RecordBatch>) -> Result<Vec<OAuthClient>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            let str_col = |name: &str| batch.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| a.value(i).to_string()).unwrap_or_default();
            let opt_str = |name: &str| batch.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) });
            let uris: Vec<String> = serde_json::from_str(&str_col("redirect_uris"))
                .map_err(|e| AppError::Internal(e.to_string()))?;
            out.push(OAuthClient {
                id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
                client_name: opt_str("client_name"),
                redirect_uris: uris,
                created_at: str_col("created_at").parse().map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
            });
        }
    }
    Ok(out)
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_insert_and_get_client() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        let c = OAuthClient {
            id: Uuid::new_v4(),
            client_name: Some("Claude".into()),
            redirect_uris: vec!["http://127.0.0.1:33418/callback".into()],
            created_at: Utc::now(),
        };
        db.insert_oauth_client(&c).await.unwrap();
        let got = db.get_oauth_client(c.id).await.unwrap().unwrap();
        assert_eq!(got.redirect_uris, c.redirect_uris);
        assert_eq!(got.client_name.as_deref(), Some("Claude"));
    }

    #[tokio::test]
    async fn test_get_missing_client() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        assert!(db.get_oauth_client(Uuid::new_v4()).await.unwrap().is_none());
    }
}
```

- [ ] **Step 2: Register** `mod oauth_client_ops;` in `src/db/mod.rs`.
- [ ] **Step 3:** `cargo test oauth_client_ops` → 2 PASS.
- [ ] **Step 4: clippy + commit** `git commit -m "feat(db): oauth_clients CRUD ops"`

---

## Task 4: `authorization_codes` DB ops (insert + consume-once)

**Files:**
- Create: `src/db/authorization_code_ops.rs`
- Modify: `src/db/mod.rs` (`mod authorization_code_ops;`)

The critical method is `consume_authorization_code`: fetch by hash, reject if missing/expired/already-consumed, else mark consumed and return it. At single-instance scale this read-then-write is adequate (note: not atomic across processes — acceptable per the LanceDB single-node deployment).

- [ ] **Step 1: Write the ops + tests**

```rust
// src/db/authorization_code_ops.rs
use crate::{
    db::{authorization_code_schema, DbClient},
    error::AppError,
    models::AuthorizationCode,
};
use arrow_array::{Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;

impl DbClient {
    pub async fn insert_authorization_code(&self, c: &AuthorizationCode) -> Result<(), AppError> {
        let batch = code_to_batch(c)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], authorization_code_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.authorization_code_table.add(reader).execute().await
            .map(|_| ()).map_err(|e| AppError::Internal(e.to_string()))
    }

    /// Validate + consume in one call. Returns the code row on success, or
    /// `None` if missing / expired / already consumed.
    pub async fn consume_authorization_code(
        &self, code_hash: &str, now: DateTime<Utc>,
    ) -> Result<Option<AuthorizationCode>, AppError> {
        let escaped = code_hash.replace('\'', "''");
        let stream = self.authorization_code_table.query()
            .only_if(format!("code_hash = '{escaped}'"))
            .execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut rows = batches_to_codes(collect_stream(stream).await?)?;
        let row = match rows.pop() {
            Some(r) => r,
            None => return Ok(None),
        };
        if row.consumed_at.is_some() || row.expires_at <= now {
            return Ok(None);
        }
        let mut consumed = row.clone();
        consumed.consumed_at = Some(now);
        let batch = code_to_batch(&consumed)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], authorization_code_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.authorization_code_table.merge_insert(&["code_hash"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await.map(|_| ()).map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(Some(row))
    }
}

fn code_to_batch(c: &AuthorizationCode) -> Result<RecordBatch, AppError> {
    let client_id = c.client_id.to_string();
    let subject_id = c.subject_id.to_string();
    let created = c.created_at.to_rfc3339();
    let expires = c.expires_at.to_rfc3339();
    let consumed = c.consumed_at.as_ref().map(|d| d.to_rfc3339());
    RecordBatch::try_new(authorization_code_schema(), vec![
        Arc::new(StringArray::from(vec![c.code_hash.as_str()])),
        Arc::new(StringArray::from(vec![client_id.as_str()])),
        Arc::new(StringArray::from(vec![c.redirect_uri.as_str()])),
        Arc::new(StringArray::from(vec![c.code_challenge.as_str()])),
        Arc::new(StringArray::from(vec![c.subject_type.as_str()])),
        Arc::new(StringArray::from(vec![subject_id.as_str()])),
        Arc::new(StringArray::from(vec![c.resource.as_str()])),
        Arc::new(StringArray::from(vec![c.scope.as_deref()])),
        Arc::new(StringArray::from(vec![created.as_str()])),
        Arc::new(StringArray::from(vec![expires.as_str()])),
        Arc::new(StringArray::from(vec![consumed.as_deref()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_codes(batches: Vec<RecordBatch>) -> Result<Vec<AuthorizationCode>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            let str_col = |name: &str| batch.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| a.value(i).to_string()).unwrap_or_default();
            let opt_str = |name: &str| batch.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) });
            let parse_dt = |s: String| s.parse::<DateTime<Utc>>().map_err(|e| AppError::Internal(e.to_string()));
            out.push(AuthorizationCode {
                code_hash: str_col("code_hash"),
                client_id: str_col("client_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
                redirect_uri: str_col("redirect_uri"),
                code_challenge: str_col("code_challenge"),
                subject_type: str_col("subject_type"),
                subject_id: str_col("subject_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
                resource: str_col("resource"),
                scope: opt_str("scope"),
                created_at: parse_dt(str_col("created_at"))?,
                expires_at: parse_dt(str_col("expires_at"))?,
                consumed_at: opt_str("consumed_at").map(parse_dt).transpose()?,
            });
        }
    }
    Ok(out)
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use tempfile::TempDir;

    fn sample(hash: &str, expires: DateTime<Utc>) -> AuthorizationCode {
        AuthorizationCode {
            code_hash: hash.into(),
            client_id: Uuid::new_v4(),
            redirect_uri: "http://127.0.0.1/cb".into(),
            code_challenge: "chal".into(),
            subject_type: "dispatcher".into(),
            subject_id: Uuid::new_v4(),
            resource: "https://x/dispatch/mcp".into(),
            scope: None,
            created_at: Utc::now(),
            expires_at: expires,
            consumed_at: None,
        }
    }

    #[tokio::test]
    async fn test_consume_once() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        db.insert_authorization_code(&sample("h1", Utc::now() + chrono::Duration::minutes(5))).await.unwrap();
        assert!(db.consume_authorization_code("h1", Utc::now()).await.unwrap().is_some());
        // Second consume fails (already consumed).
        assert!(db.consume_authorization_code("h1", Utc::now()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_consume_expired() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        db.insert_authorization_code(&sample("h2", Utc::now() - chrono::Duration::minutes(1))).await.unwrap();
        assert!(db.consume_authorization_code("h2", Utc::now()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_consume_missing() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        assert!(db.consume_authorization_code("nope", Utc::now()).await.unwrap().is_none());
    }
}
```

- [ ] **Step 2: Register** `mod authorization_code_ops;`.
- [ ] **Step 3:** `cargo test authorization_code_ops` → 3 PASS.
- [ ] **Step 4: clippy + commit** `git commit -m "feat(db): authorization_codes ops with consume-once"`

---

## Task 5: OAuth module skeleton + `ResourceDescriptor` + `OauthError`

**Files:**
- Create: `src/api/oauth/mod.rs`
- Modify: `src/api/mod.rs` (`pub mod oauth;`)

- [ ] **Step 1: Write the module shell**

```rust
// src/api/oauth/mod.rs
//
// Portal-agnostic OAuth 2.1 Authorization Server for MCP connectors.
// One resource wired today: dispatcher (/dispatch/mcp). Driver is future work.
pub mod authorize;
pub mod metadata;
pub mod register;
pub mod token;

use crate::AppState;
use axum::{routing::{get, post}, Router};

pub const DISPATCH_MCP_PATH: &str = "/dispatch/mcp";

/// Absolute issuer/base URL from config (e.g. https://ollie.your-ollie-instance.example.com).
pub fn issuer(state: &AppState) -> String {
    state.config.public_base_url.trim_end_matches('/').to_string()
}

/// The protected-resource URL for the dispatcher MCP endpoint.
pub fn dispatch_resource(state: &AppState) -> String {
    format!("{}{}", issuer(state), DISPATCH_MCP_PATH)
}

/// All OAuth routes — mounted PUBLIC (no dispatcher middleware).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/oauth-authorization-server", get(metadata::authorization_server))
        .route("/.well-known/oauth-protected-resource", get(metadata::protected_resource))
        // Desktop also probes the path-suffixed PRM form:
        .route("/.well-known/oauth-protected-resource/dispatch/mcp", get(metadata::protected_resource))
        .route("/oauth/register", post(register::register))
        .route("/oauth/authorize", get(authorize::authorize_page).post(authorize::authorize_decision))
        .route("/oauth/token", post(token::token))
}

/// OAuth error rendered per-spec. Token/DCR → JSON; authorize → handled inline
/// (redirect vs error page) in `authorize.rs`.
pub enum OauthError {
    InvalidRequest(String),
    InvalidClient(String),
    InvalidGrant(String),
    UnsupportedGrantType,
    InvalidClientMetadata(String),
    ServerError(String),
}

impl axum::response::IntoResponse for OauthError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        let (status, code, desc) = match self {
            OauthError::InvalidRequest(d) => (StatusCode::BAD_REQUEST, "invalid_request", d),
            OauthError::InvalidClient(d) => (StatusCode::UNAUTHORIZED, "invalid_client", d),
            OauthError::InvalidGrant(d) => (StatusCode::BAD_REQUEST, "invalid_grant", d),
            OauthError::UnsupportedGrantType => (StatusCode::BAD_REQUEST, "unsupported_grant_type", String::new()),
            OauthError::InvalidClientMetadata(d) => (StatusCode::BAD_REQUEST, "invalid_client_metadata", d),
            OauthError::ServerError(d) => (StatusCode::INTERNAL_SERVER_ERROR, "server_error", d),
        };
        let body = axum::Json(serde_json::json!({ "error": code, "error_description": desc }));
        (status, body).into_response()
    }
}
```

- [ ] **Step 2: Register** `pub mod oauth;` in `src/api/mod.rs`.
- [ ] **Step 3:** `cargo build` — expect errors only from the not-yet-written submodule fns; create empty stub fns if needed to compile incrementally, OR implement Tasks 6–8 before first build. Recommended: stub each handler to `async fn x() -> &'static str { "" }` temporarily, build, then fill in.
- [ ] **Step 4: commit** once it compiles: `git commit -m "feat(oauth): module skeleton, router, OauthError"`

---

## Task 6: Discovery metadata (RFC 9728 + RFC 8414)

**Files:**
- Create: `src/api/oauth/metadata.rs`

- [ ] **Step 1: Write both handlers + tests**

```rust
// src/api/oauth/metadata.rs
use crate::AppState;
use axum::{extract::State, Json};
use serde_json::json;

/// RFC 9728 Protected Resource Metadata.
pub async fn protected_resource(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "resource": super::dispatch_resource(&state),
        "authorization_servers": [super::issuer(&state)],
    }))
}

/// RFC 8414 Authorization Server Metadata.
pub async fn authorization_server(State(state): State<AppState>) -> Json<serde_json::Value> {
    let iss = super::issuer(&state);
    Json(json!({
        "issuer": iss,
        "authorization_endpoint": format!("{iss}/oauth/authorize"),
        "token_endpoint": format!("{iss}/oauth/token"),
        "registration_endpoint": format!("{iss}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
    }))
}
```

- [ ] **Step 2: Integration test** (add to Task 10's test file, or a quick `#[tokio::test]` building the app): `GET /.well-known/oauth-authorization-server` → 200, JSON `code_challenge_methods_supported == ["S256"]`, `token_endpoint` ends with `/oauth/token`. `GET /.well-known/oauth-protected-resource/dispatch/mcp` → 200, `authorization_servers[0]` equals the issuer.
- [ ] **Step 3:** `cargo build && cargo clippy --all-targets -- -D warnings`
- [ ] **Step 4: commit** `git commit -m "feat(oauth): discovery metadata endpoints"`

---

## Task 7: Dynamic Client Registration (RFC 7591)

**Files:**
- Create: `src/api/oauth/register.rs`

- [ ] **Step 1: Write the handler + tests**

```rust
// src/api/oauth/register.rs
use crate::{models::OAuthClient, AppState};
use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use super::OauthError;

#[derive(Deserialize)]
pub struct RegisterRequest {
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
}

#[derive(Serialize)]
pub struct RegisterResponse {
    pub client_id: String,
    pub token_endpoint_auth_method: &'static str,
    pub redirect_uris: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
}

/// A redirect URI is acceptable if it is an https URL or a loopback http URL.
fn redirect_uri_ok(uri: &str) -> bool {
    if let Ok(u) = url::Url::parse(uri) {
        match u.scheme() {
            "https" => true,
            "http" => matches!(u.host_str(), Some("127.0.0.1") | Some("localhost") | Some("[::1]")),
            // Desktop custom schemes (e.g. claude://) are allowed.
            s => !s.is_empty() && s != "http",
        }
    } else {
        false
    }
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<RegisterResponse>), OauthError> {
    if req.redirect_uris.is_empty() {
        return Err(OauthError::InvalidClientMetadata("redirect_uris required".into()));
    }
    for uri in &req.redirect_uris {
        if !redirect_uri_ok(uri) {
            return Err(OauthError::InvalidClientMetadata(format!("invalid redirect_uri: {uri}")));
        }
    }
    let client = OAuthClient {
        id: Uuid::new_v4(),
        client_name: req.client_name.clone(),
        redirect_uris: req.redirect_uris.clone(),
        created_at: Utc::now(),
    };
    state.db.insert_oauth_client(&client).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(RegisterResponse {
        client_id: client.id.to_string(),
        token_endpoint_auth_method: "none",
        redirect_uris: client.redirect_uris,
        client_name: client.client_name,
    })))
}
```

- [ ] **Step 2: Confirm `url` dep** — `grep -E '^url' Cargo.toml`; if absent add `url = "2"`.
- [ ] **Step 3: Tests** — unit-test `redirect_uri_ok`: `https://x/cb` ✓, `http://127.0.0.1:9/cb` ✓, `http://evil.com/cb` ✗, `claude://cb` ✓, `not a url` ✗.
- [ ] **Step 4:** `cargo test register && cargo clippy --all-targets -- -D warnings`
- [ ] **Step 5: commit** `git commit -m "feat(oauth): dynamic client registration"`

---

## Task 8: Authorize endpoint — login + consent page, then code issuance

**Files:**
- Create: `src/api/oauth/authorize.rs`

Single server-rendered page: `GET` validates params and renders an email/password + Allow/Deny form (OAuth params embedded as hidden inputs). `POST` verifies bcrypt, and on Allow mints a one-time code bound to the PKCE challenge, then 302s to the client's `redirect_uri`.

- [ ] **Step 1: Write the handlers**

```rust
// src/api/oauth/authorize.rs
use crate::{models::AuthorizationCode, AppState};
use axum::{
    extract::{Query, State},
    http::{header::LOCATION, StatusCode},
    response::{Html, IntoResponse, Response},
    Form,
};
use chrono::{Duration, Utc};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Deserialize, Clone)]
pub struct AuthorizeParams {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    #[serde(default)]
    pub code_challenge_method: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
}

#[derive(Deserialize)]
pub struct AuthorizeForm {
    // OAuth params, round-tripped as hidden fields:
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub scope: Option<String>,
    pub resource: Option<String>,
    // User input:
    pub email: String,
    pub password: String,
    pub decision: String, // "allow" | "deny"
}

fn h(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Validate client_id + redirect_uri + PKCE. Returns the matched client on success.
async fn validate(state: &AppState, client_id: &str, redirect_uri: &str, challenge: &str, method: &Option<String>)
    -> Result<crate::models::OAuthClient, String>
{
    if challenge.is_empty() { return Err("PKCE code_challenge required".into()); }
    if let Some(m) = method { if m != "S256" { return Err("only S256 supported".into()); } }
    let id: Uuid = client_id.parse().map_err(|_| "unknown client".to_string())?;
    let client = state.db.get_oauth_client(id).await
        .map_err(|e| e.to_string())?
        .ok_or("unknown client")?;
    if !client.redirect_uris.iter().any(|u| u == redirect_uri) {
        return Err("redirect_uri mismatch".into());
    }
    Ok(client)
}

pub async fn authorize_page(
    State(state): State<AppState>,
    Query(p): Query<AuthorizeParams>,
) -> Response {
    if p.response_type != "code" {
        return (StatusCode::BAD_REQUEST, "unsupported response_type").into_response();
    }
    // Invalid client/redirect ⇒ DO NOT redirect (open-redirect guard); show error page.
    if let Err(e) = validate(&state, &p.client_id, &p.redirect_uri, &p.code_challenge, &p.code_challenge_method).await {
        return (StatusCode::BAD_REQUEST, Html(format!("<h1>Authorization error</h1><p>{}</p>", h(&e)))).into_response();
    }
    let hidden = |k: &str, v: &str| format!(r#"<input type="hidden" name="{k}" value="{}">"#, h(v));
    let page = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Authorize Ollie</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>body{{font-family:system-ui;max-width:24rem;margin:3rem auto;padding:0 1rem}}
input[type=email],input[type=password]{{display:block;width:100%;padding:.5rem;margin:.4rem 0;box-sizing:border-box}}
button{{padding:.6rem 1rem;margin-right:.5rem}}</style></head>
<body><h1>Connect to Ollie</h1>
<p><strong>{client}</strong> wants to access your Ollie dispatcher account.</p>
<form method="post" action="/oauth/authorize">
{p_rt}{p_cid}{p_ru}{p_cc}{p_ccm}{p_state}{p_scope}{p_res}
<label>Email<input type="email" name="email" required autofocus></label>
<label>Password<input type="password" name="password" required></label>
<button type="submit" name="decision" value="allow">Allow</button>
<button type="submit" name="decision" value="deny">Deny</button>
</form></body></html>"#,
        client = h("Claude"),
        p_rt = hidden("response_type", &p.response_type),
        p_cid = hidden("client_id", &p.client_id),
        p_ru = hidden("redirect_uri", &p.redirect_uri),
        p_cc = hidden("code_challenge", &p.code_challenge),
        p_ccm = hidden("code_challenge_method", p.code_challenge_method.as_deref().unwrap_or("S256")),
        p_state = hidden("state", p.state.as_deref().unwrap_or("")),
        p_scope = hidden("scope", p.scope.as_deref().unwrap_or("")),
        p_res = hidden("resource", p.resource.as_deref().unwrap_or("")),
    );
    Html(page).into_response()
}

fn redirect_with(redirect_uri: &str, query: &str) -> Response {
    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    let mut r = StatusCode::FOUND.into_response();
    r.headers_mut().insert(LOCATION, format!("{redirect_uri}{sep}{query}").parse().unwrap());
    r
}

pub async fn authorize_decision(
    State(state): State<AppState>,
    Form(f): Form<AuthorizeForm>,
) -> Response {
    let client = match validate(&state, &f.client_id, &f.redirect_uri, &f.code_challenge, &f.code_challenge_method).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_REQUEST, Html(format!("<h1>Authorization error</h1><p>{}</p>", h(&e)))).into_response(),
    };
    let state_q = f.state.clone().unwrap_or_default();

    if f.decision != "allow" {
        return redirect_with(&f.redirect_uri, &format!("error=access_denied&state={}", urlencode(&state_q)));
    }

    // Authenticate the dispatcher via the existing bcrypt path.
    let email = f.email.trim().to_lowercase();
    let dispatcher = match state.db.get_dispatcher_by_email(&email).await {
        Ok(Some(d)) => d,
        _ => return (StatusCode::UNAUTHORIZED, Html("<h1>Invalid credentials</h1>".to_string())).into_response(),
    };
    let creds = match state.db.get_dispatcher_credentials(dispatcher.id).await {
        Ok(Some(c)) => c,
        _ => return (StatusCode::UNAUTHORIZED, Html("<h1>Invalid credentials</h1>".to_string())).into_response(),
    };
    let pw = f.password.clone();
    let hash = creds.password_hash.clone();
    let ok = tokio::task::spawn_blocking(move || bcrypt::verify(&pw, &hash)).await
        .ok().and_then(|r| r.ok()).unwrap_or(false);
    if !ok {
        return (StatusCode::UNAUTHORIZED, Html("<h1>Invalid credentials</h1>".to_string())).into_response();
    }

    // Mint a one-time code bound to the PKCE challenge + dispatcher + resource.
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    use base64::Engine;
    let code = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let code_hash = hex::encode(Sha256::digest(code.as_bytes()));
    let resource = if f.resource.as_deref().unwrap_or("").is_empty() {
        super::dispatch_resource(&state)
    } else {
        f.resource.clone().unwrap()
    };
    let record = AuthorizationCode {
        code_hash,
        client_id: client.id,
        redirect_uri: f.redirect_uri.clone(),
        code_challenge: f.code_challenge.clone(),
        subject_type: "dispatcher".into(),
        subject_id: dispatcher.id,
        resource,
        scope: if f.scope.as_deref().unwrap_or("").is_empty() { None } else { f.scope.clone() },
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::minutes(5),
        consumed_at: None,
    };
    if let Err(e) = state.db.insert_authorization_code(&record).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Html(format!("<h1>Server error</h1><p>{}</p>", h(&e.to_string())))).into_response();
    }
    redirect_with(&f.redirect_uri, &format!("code={}&state={}", urlencode(&code), urlencode(&state_q)))
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
```

- [ ] **Step 2: Tests** — unit-test `redirect_with` (appends `?` vs `&` correctly) and `h` (escapes `<`, `"`). The full login→code path is covered in Task 10's integration test.
- [ ] **Step 3:** `cargo build && cargo clippy --all-targets -- -D warnings`
- [ ] **Step 4: commit** `git commit -m "feat(oauth): authorize login+consent page and code issuance"`

---

## Task 9: Token endpoint (authorization_code + refresh_token grants)

**Files:**
- Create: `src/api/oauth/token.rs`

The access token is a `DispatcherClaims` JWT (`encode_dispatcher_jwt`); the refresh token reuses `refresh_tokens::issue` (with `client_id` set) and `refresh_tokens::rotate`.

- [ ] **Step 1: Write the handler + a PKCE-verify unit test**

```rust
// src/api/oauth/token.rs
use crate::{
    api::dispatcher_portal::jwt::encode_dispatcher_jwt,
    api::refresh_tokens,
    AppState,
};
use axum::{extract::State, Json, Form};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use super::OauthError;

#[derive(Deserialize)]
pub struct TokenForm {
    pub grant_type: String,
    // authorization_code grant:
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
    pub code_verifier: Option<String>,
    // refresh_token grant:
    pub refresh_token: Option<String>,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub refresh_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

const ACCESS_TTL_SECS: i64 = 8 * 3600;

/// PKCE S256: base64url(SHA256(verifier)) == challenge.
fn pkce_ok(verifier: &str, challenge: &str) -> bool {
    use base64::Engine;
    let digest = Sha256::digest(verifier.as_bytes());
    let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    computed == challenge
}

pub async fn token(
    State(state): State<AppState>,
    Form(f): Form<TokenForm>,
) -> Result<Json<TokenResponse>, OauthError> {
    match f.grant_type.as_str() {
        "authorization_code" => auth_code_grant(&state, f).await,
        "refresh_token" => refresh_grant(&state, f).await,
        _ => Err(OauthError::UnsupportedGrantType),
    }
}

async fn auth_code_grant(state: &AppState, f: TokenForm) -> Result<Json<TokenResponse>, OauthError> {
    let code = f.code.ok_or(OauthError::InvalidRequest("code required".into()))?;
    let redirect_uri = f.redirect_uri.ok_or(OauthError::InvalidRequest("redirect_uri required".into()))?;
    let client_id = f.client_id.ok_or(OauthError::InvalidRequest("client_id required".into()))?;
    let verifier = f.code_verifier.ok_or(OauthError::InvalidRequest("code_verifier required".into()))?;

    let code_hash = hex::encode(Sha256::digest(code.as_bytes()));
    let record = state.db.consume_authorization_code(&code_hash, Utc::now()).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
        .ok_or(OauthError::InvalidGrant("code invalid, expired, or used".into()))?;

    if record.client_id.to_string() != client_id {
        return Err(OauthError::InvalidGrant("client_id mismatch".into()));
    }
    if record.redirect_uri != redirect_uri {
        return Err(OauthError::InvalidGrant("redirect_uri mismatch".into()));
    }
    if !pkce_ok(&verifier, &record.code_challenge) {
        return Err(OauthError::InvalidGrant("PKCE verification failed".into()));
    }

    let creds = state.db.get_dispatcher_credentials(record.subject_id).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
        .ok_or(OauthError::InvalidGrant("unknown subject".into()))?;

    let access = encode_dispatcher_jwt(record.subject_id, creds.token_version, &state.config.dispatcher_jwt_secret)
        .map_err(|e| OauthError::ServerError(e.to_string()))?;
    let issued = refresh_tokens::issue(
        &state.db, "dispatcher", record.subject_id, Some(record.client_id), creds.token_version, Utc::now(),
    ).await.map_err(|e| OauthError::ServerError(e.to_string()))?;

    Ok(Json(TokenResponse {
        access_token: access,
        token_type: "Bearer",
        expires_in: ACCESS_TTL_SECS,
        refresh_token: issued.secret,
        scope: record.scope,
    }))
}

async fn refresh_grant(state: &AppState, f: TokenForm) -> Result<Json<TokenResponse>, OauthError> {
    let secret = f.refresh_token.ok_or(OauthError::InvalidRequest("refresh_token required".into()))?;
    let hash = refresh_tokens::hash_token(&secret);
    let row = state.db.get_refresh_token_by_hash(&hash).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
        .ok_or(OauthError::InvalidGrant("unknown refresh_token".into()))?;
    let creds = state.db.get_dispatcher_credentials(row.subject_id).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
        .ok_or(OauthError::InvalidGrant("unknown subject".into()))?;

    match refresh_tokens::rotate(&state.db, &secret, creds.token_version, Utc::now()).await
        .map_err(|e| OauthError::ServerError(e.to_string()))?
    {
        refresh_tokens::RotateResult::Rotated(next) => {
            let access = encode_dispatcher_jwt(row.subject_id, creds.token_version, &state.config.dispatcher_jwt_secret)
                .map_err(|e| OauthError::ServerError(e.to_string()))?;
            Ok(Json(TokenResponse {
                access_token: access,
                token_type: "Bearer",
                expires_in: ACCESS_TTL_SECS,
                refresh_token: next.secret,
                scope: None,
            }))
        }
        _ => Err(OauthError::InvalidGrant("refresh_token invalid or reused".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_pkce_known_vector() {
        // RFC 7636 Appendix B vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(pkce_ok(verifier, challenge));
        assert!(!pkce_ok("wrong", challenge));
    }
}
```

- [ ] **Step 2:** `cargo test token && cargo clippy --all-targets -- -D warnings` — PKCE vector PASS.
- [ ] **Step 3: commit** `git commit -m "feat(oauth): token endpoint (auth-code + refresh grants)"`

---

## Task 10: Wire the router + `WWW-Authenticate` on `/dispatch/mcp`

**Files:**
- Modify: `src/api/mod.rs`

- [ ] **Step 1: Merge the OAuth router (public, unauthenticated)**

In `router()` in `src/api/mod.rs`, where the public surfaces are merged (next to `.merge(dispatcher_auth)` / `.merge(dispatcher_public)`), add:

```rust
        .merge(crate::api::oauth::router())
```

- [ ] **Step 2: Add the `WWW-Authenticate` layer scoped to the MCP route**

The MCP route is mounted inside `dispatcher_portal::data_router` at `/dispatch/mcp`. Add a `map_response` layer **on that one route** (in `src/api/dispatcher_portal/mod.rs`, modify just the `/dispatch/mcp` route registration) that, when the response is `401`, attaches the header. The value points at the path-suffixed PRM:

```rust
        .route(
            "/dispatch/mcp",
            post(mcp::handle)
                .layer(DefaultBodyLimit::max(1024 * 1024))
                .layer(axum::middleware::map_response_with_state(
                    state.clone(),
                    mcp_www_authenticate,
                )),
        )
```

And add the helper in `src/api/dispatcher_portal/mod.rs`:

```rust
async fn mcp_www_authenticate(
    State(state): State<AppState>,
    mut response: axum::response::Response,
) -> axum::response::Response {
    if response.status() == axum::http::StatusCode::UNAUTHORIZED {
        let prm = format!(
            "{}/.well-known/oauth-protected-resource/dispatch/mcp",
            state.config.public_base_url.trim_end_matches('/'),
        );
        if let Ok(v) = format!("Bearer resource_metadata=\"{prm}\"").parse() {
            response.headers_mut().insert(axum::http::header::WWW_AUTHENTICATE, v);
        }
    }
    response
}
```

(Add `use axum::extract::State;` if not already imported in that file. Confirm `map_response_with_state` is available in the axum 0.7 version in `Cargo.lock`; if not, use `map_response` with a closure capturing the base URL from `state` passed into `data_router`.)

- [ ] **Step 3: Verify REST 401s are NOT affected** — the layer is only on the `/dispatch/mcp` route, so other routes under the same middleware still return bare 401s.

- [ ] **Step 4: Full integration test** — create `tests/oauth_flow.rs` (or the repo's integration-test location) asserting the end-to-end Desktop flow:

```rust
// Behaviors (translate to the repo's TestServer harness with a real AppState/TempDir DB):
//
// 1. mcp_401_has_www_authenticate:
//    POST /dispatch/mcp with no Authorization → 401 AND a
//    `WWW-Authenticate: Bearer resource_metadata="…/.well-known/oauth-protected-resource/dispatch/mcp"`.
//
// 2. rest_401_has_no_www_authenticate:
//    GET /dispatch/api/v1/loads with no auth → 401 and NO WWW-Authenticate header.
//
// 3. metadata_endpoints:
//    GET /.well-known/oauth-authorization-server → S256 + the four endpoints.
//    GET /.well-known/oauth-protected-resource/dispatch/mcp → resource + authorization_servers.
//
// 4. full_oauth_dance:
//    a. POST /oauth/register {redirect_uris:["http://127.0.0.1:9/cb"]} → 201 client_id.
//    b. Build PKCE verifier/challenge (S256). POST /oauth/authorize (form) with a seeded
//       dispatcher's email+password, decision=allow, the challenge, client_id, redirect_uri
//       → 302 Location to redirect_uri with ?code=…&state=….
//    c. POST /oauth/token grant_type=authorization_code with code+verifier+client_id+redirect_uri
//       → 200 {access_token, refresh_token, token_type:"Bearer", expires_in:28800}.
//    d. POST /dispatch/mcp with `Authorization: Bearer <access_token>` and an `initialize`
//       JSON-RPC body → 200 (the OAuth-minted JWT is accepted by the unchanged hot path).
//    e. POST /oauth/token grant_type=refresh_token with the refresh_token → 200 with a NEW
//       access_token + rotated refresh_token.
//
// 5. token_rejects_bad_pkce:
//    Repeat 4a–4b, then POST /oauth/token with a WRONG code_verifier → 400 invalid_grant.
//
// 6. olld_key_still_works:
//    POST /dispatch/mcp with a seeded `olld_` key → 200 (headless path unaffected).
```

- [ ] **Step 5:** `cargo test && cargo clippy --all-targets -- -D warnings`
- [ ] **Step 6: commit** `git commit -m "feat(oauth): wire router + WWW-Authenticate on /dispatch/mcp; e2e tests"`

---

## Task 11: Manual acceptance — the real #305 test

- [ ] **Step 1:** Deploy the branch to a reachable HTTPS host (Desktop requires https for non-loopback). Add `https://<host>/dispatch/mcp` as a remote MCP connector in Claude Desktop.
- [ ] **Step 2:** Confirm the connector setup runs DCR → opens the Ollie authorize page → after Allow, the connector attaches and `tools/list` works. The prior failure ref was `oauth_error=mcp_registration_failed`; success = the connector lists Ollie's tools.
- [ ] **Step 3:** If Desktop can't be exercised (no https host available), say so explicitly rather than claiming success; the `tests/oauth_flow.rs` `full_oauth_dance` is the automated stand-in.

---

## Self-Review Notes (addressed)

- **Spec coverage:** `WWW-Authenticate` scoped to MCP route (Task 10); PRM RFC 9728 + AS metadata RFC 8414 (Task 6); DCR RFC 7591 public-client (Task 7); authorize + PKCE + consent (Task 8); token auth-code + refresh grants, access token = dispatcher JWT, refresh via Plan 1 (Task 9); `olld_` coexistence asserted (Task 10 test 6); driver parameterization left as a documented seam (resource descriptor in `oauth/mod.rs`, not wired). 
- **Deviation from spec, intentional:** single-page login+consent, no AS-session cookie, `oauth_consent_grants` not built (was optional). Recorded at top.
- **`Utf8` trap:** both tables created via `open_or_create` with `DataType::Utf8` Rust schemas; `only_if` filters use quoted string literals only; no `CAST`.
- **Type consistency:** `OauthError` variants (Task 5) used in Tasks 7/9; `consume_authorization_code` (Task 4) used in Task 9; `refresh_tokens::{issue,rotate,hash_token,RotateResult}` (Plan 1) used in Task 9; `encode_dispatcher_jwt` reused unchanged; `dispatch_resource`/`issuer` (Task 5) used in Tasks 6/8/10.
- **Dependency on Plan 1:** must be merged first (`refresh_tokens` module, `refresh_token` table, `cookie_secure` config).
```
