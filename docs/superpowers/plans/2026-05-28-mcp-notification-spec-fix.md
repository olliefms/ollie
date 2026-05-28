# MCP Notification Spec-Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `POST /dispatch/mcp` return `202 Accepted` with an empty body for JSON-RPC notifications (messages with no `id`), so a stock Streamable-HTTP MCP client (`type: "http"`) can complete the handshake instead of choking on the current `200 + error` reply to `notifications/initialized`.

**Architecture:** A single-function change in `src/api/dispatcher_portal/mcp.rs`. `handle`'s return type changes from `Json<JsonRpcResponse>` to `axum::response::Response`. A new first branch returns `202` for any `id`-less request without dispatching or erroring. The existing request path (with `id`) is unchanged except each arm now calls `.into_response()`. A new `#[cfg(test)]` module exercises both paths by calling `handle` directly.

**Tech Stack:** Rust, axum 0.7, serde_json, tempfile + tokio for tests (matching the existing `src/api/facilities.rs` test harness). No dependency changes.

**Spec:** `docs/superpowers/specs/2026-05-28-mcp-notification-spec-fix-design.md`

---

## File Structure

- **Modify:** `src/api/dispatcher_portal/mcp.rs`
  - Extend the `use axum::{...}` import (lines 12-15) to add `http::StatusCode` and `response::{IntoResponse, Response}`.
  - Rewrite `handle` (lines 682-703) to branch on notifications and return `Response`.
  - Append a `#[cfg(test)]` module at the end of the file (after `tool_delete_blob`).

No other files change. No routes, dependencies, or auth touched.

---

### Task 1: Notifications return 202; requests unchanged

**Files:**
- Modify: `src/api/dispatcher_portal/mcp.rs:12-15` (imports)
- Modify: `src/api/dispatcher_portal/mcp.rs:682-703` (`handle`)
- Test: `src/api/dispatcher_portal/mcp.rs` (new `#[cfg(test)] mod tests` at EOF)

- [ ] **Step 1: Write the failing test module**

Append to the very end of `src/api/dispatcher_portal/mcp.rs`. The `test_state` helper mirrors `src/api/facilities.rs:296`; none of these tests touch the DB (all five branches return before `tools/call`), but an `AppState` is required to call `handle`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ai::OllamaClient, config::Config, db::DbClient, routing::RoutingClient,
        storage::BlobStore,
    };
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn test_state() -> (AppState, TempDir, TempDir) {
        let blob_dir = TempDir::new().unwrap();
        let db_dir = TempDir::new().unwrap();
        std::env::set_var("ADMIN_API_KEY", "test-secret");
        std::env::set_var("DRIVER_JWT_SECRET", "test-driver-jwt-secret-that-is-long-enough");
        std::env::set_var("DISPATCHER_JWT_SECRET", "test-dispatcher-jwt-secret-that-is-long-enough");
        std::env::set_var("DRIVER_RP_ID", "localhost");
        std::env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
        let config = Arc::new(Config::from_env().unwrap());
        let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
        let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
        let ai = Arc::new(OllamaClient::new(
            "http://localhost:11434", "nomic-embed-text", "llama3.2", "llava",
        ));
        let geocoding = Arc::new(crate::geocoding::GeocodingClient::new());
        let ors = Arc::new(RoutingClient::new(""));
        let (geocoding_tx, _rx) = async_channel::bounded(10);
        let (routing_tx, _rx2) = async_channel::bounded(10);
        let (pipeline_tx, _rx3) = async_channel::bounded(10);
        let rp_origin = webauthn_rs::prelude::Url::parse("http://localhost:3000").unwrap();
        let webauthn = Arc::new(
            webauthn_rs::prelude::WebauthnBuilder::new("localhost", &rp_origin)
                .unwrap()
                .build()
                .unwrap(),
        );
        let auth_challenge_store = Arc::new(dashmap::DashMap::new());
        let reg_challenge_store = Arc::new(dashmap::DashMap::new());
        let state = AppState {
            db, store, ai, geocoding, ors,
            pipeline_tx, geocoding_tx, routing_tx,
            config, webauthn,
            auth_challenge_store,
            reg_challenge_store,
        };
        (state, blob_dir, db_dir)
    }

    /// Call `handle` directly and return (status, body bytes).
    async fn call(state: &AppState, body: Value) -> (StatusCode, Vec<u8>) {
        let req: JsonRpcRequest = serde_json::from_value(body).unwrap();
        let resp = handle(State(state.clone()), Json(req)).await;
        let (parts, body) = resp.into_parts();
        let bytes = to_bytes(body, usize::MAX).await.unwrap();
        (parts.status, bytes.to_vec())
    }

    #[tokio::test]
    async fn notifications_initialized_returns_202_empty() {
        let (state, _b, _d) = test_state().await;
        let (status, body) = call(&state, json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        })).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(body.is_empty(), "notification response body must be empty");
    }

    #[tokio::test]
    async fn arbitrary_notification_returns_202_empty() {
        let (state, _b, _d) = test_state().await;
        let (status, body) = call(&state, json!({
            "jsonrpc": "2.0",
            "method": "some/unknown/notification"
        })).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn initialize_request_returns_200_result() {
        let (state, _b, _d) = test_state().await;
        let (status, body) = call(&state, json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {}
        })).await;
        assert_eq!(status, StatusCode::OK);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "ollie-dispatcher");
        assert!(v.get("error").is_none());
    }

    #[tokio::test]
    async fn tools_list_request_returns_200_tools() {
        let (state, _b, _d) = test_state().await;
        let (status, body) = call(&state, json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list"
        })).await;
        assert_eq!(status, StatusCode::OK);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert!(v["result"]["tools"].as_array().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn unknown_method_with_id_returns_200_error() {
        let (state, _b, _d) = test_state().await;
        let (status, body) = call(&state, json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "no/such/method"
        })).await;
        assert_eq!(status, StatusCode::OK);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib dispatcher_portal::mcp::tests 2>&1 | tail -30`

Expected: compiles, but `notifications_initialized_returns_202_empty` and `arbitrary_notification_returns_202_empty` FAIL — the current `handle` returns `200` with a JSON-RPC error body, so the status assertion (`202`) fails. The three request tests should already PASS (those branches are unchanged).

- [ ] **Step 3: Extend the axum import**

In `src/api/dispatcher_portal/mcp.rs`, replace the import at lines 12-15:

```rust
use axum::{
    extract::{Path, State},
    Json,
};
```

with:

```rust
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
```

- [ ] **Step 4: Rewrite `handle` to branch on notifications and return `Response`**

Replace the whole `handle` function (lines 682-703) with:

```rust
pub async fn handle(
    State(state): State<AppState>,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    // A JSON-RPC notification (no `id`) gets no Response. Per Streamable HTTP a
    // notification-only POST must return 202 Accepted with an empty body, never a
    // JSON-RPC error — a stock `type: "http"` client treats an error reply to
    // `notifications/initialized` as a broken handshake.
    if req.id.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }

    let id = req.id.clone();

    if req.jsonrpc != "2.0" {
        return Json(JsonRpcResponse::err(id, -32600, "invalid JSON-RPC version")).into_response();
    }

    let result = match req.method.as_str() {
        "initialize" => handle_initialize(),
        "tools/list" => Ok(tools_list()),
        "tools/call" => handle_tool_call(&state, &req.params).await,
        _ => {
            return Json(JsonRpcResponse::err(
                id,
                -32601,
                format!("method not found: {}", req.method),
            ))
            .into_response()
        }
    };

    match result {
        Ok(value) => Json(JsonRpcResponse::ok(id, value)).into_response(),
        Err(e) => Json(JsonRpcResponse::err(id, -32603, e)).into_response(),
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib dispatcher_portal::mcp::tests 2>&1 | tail -30`

Expected: all five tests PASS.

- [ ] **Step 6: Full verification (typecheck, clippy, fmt, full test run)**

Run each and confirm clean:

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
cargo test 2>&1 | tail -20
```

Expected: `fmt` makes no unexpected changes; `clippy` reports no warnings (the old `Json<JsonRpcResponse>` return is fully replaced, so no dead-code or unused-import warnings); the full test suite passes.

- [ ] **Step 7: Commit**

```bash
git add src/api/dispatcher_portal/mcp.rs
git commit -m "$(cat <<'EOF'
fix(mcp): return 202 for JSON-RPC notifications (#105)

A notification-only POST (no `id`, e.g. notifications/initialized) now
returns 202 Accepted with an empty body instead of 200 + a JSON-RPC
error. Unblocks stock Streamable-HTTP (type: "http") MCP clients from
connecting directly, removing the need for a downstream stdio<->HTTP
proxy. Request handling (initialize/tools/list/tools/call) is unchanged.

Co-Authored-By: Claude with claude-opus-4-7[1m] <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**Spec coverage:**
- "notification-only POST → 202, empty body" → Task 1 Steps 3-4 (branch) + tests 1-2. ✓
- "request path unchanged, including -32601 for unknown requests" → Step 4 preserves arms; test 5. ✓
- "return type → Response via IntoResponse" → Steps 3-4. ✓
- "GET/protocolVersion/sessions/auth/deps NOT changed" → no task touches them. ✓
- Tests 1-5 from spec → all five `#[tokio::test]`s present. ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases"; every code step shows complete code. ✓

**Type consistency:** `handle` returns `Response`; every arm calls `.into_response()`; `JsonRpcResponse::{ok,err}` signatures match existing usage (`id: Option<Value>`, `code: i32`); `call` helper deserializes into the existing `JsonRpcRequest`. `json!` and `Value` come from the module's existing `use serde_json::{json, Value};`. ✓
