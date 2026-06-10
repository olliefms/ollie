# Fleet UI CRUD — Phase 0a (Backend Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the backend prerequisites the fleet-UI rewrite depends on — a `/me` identity+scopes endpoint, three-state (set/clear/leave) driver rate-override semantics, and a shared 409 referrer-message helper — all behind tests.

**Architecture:** Pure Rust/axum additions to the existing `fleet_portal` surface. No new crates. `/me` reuses the existing auth middleware (which already computes `effective_scopes` per request). Driver rate overrides switch from `Option<f64>` to `Option<Option<f64>>` using the codebase's existing `double_option` serde pattern, which we first extract to a shared location so terminals and drivers share one helper. A small pure formatter standardizes the "cannot permanently delete: referenced by …" message that every entity's hard-delete handler will use in later phases.

**Tech Stack:** Rust, axum 0.8, serde, utoipa, `axum_test::TestServer` integration tests, LanceDB-backed `DbClient`.

---

## Scope notes (deviations from the spec's Phase 0, intentional)

- **This is Phase 0a (backend) of a split Phase 0.** Phase 0b (frontend foundation: Vitest/happy-dom toolchain, `utils/` + `components/` scaffold, pushState router + login gate, read-only view migration) is a separate plan. The split keeps each plan independently shippable and testable (`cargo test` here; `vitest`/Playwright there).
- **Trip rate-override clear is deferred to Phase 5 (Trips).** `UpdateTripRequest` has no rate-override fields today (verified: `src/models/trip.rs:251`), so "trip rate clear" is net-new functionality coupled to the trip form, not a Phase-0 refactor. Phase 0a does **drivers only**, where the fields already exist.
- **The `archived` flag and per-entity referrer queries move to their consuming phases** (Terminals/Equipment = Phase 1, Facilities = Phase 3, etc.). Each requires entity-specific LanceDB schema/column work best done where it's used. Phase 0a lands only the shared, entity-agnostic piece: the referrer-message formatter.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `src/models/mod.rs` | models hub; home for the shared `double_option` serde helper | Modify |
| `src/models/terminal.rs` | terminal model; currently owns a private `double_option` | Modify (use shared) |
| `src/models/driver.rs` | driver model; `UpdateDriverRequest` rate fields → `Option<Option<f64>>` | Modify |
| `src/db/driver_ops.rs` | `update_driver_rate_overrides` three-state merge | Modify |
| `src/api/fleet_portal/users.rs` | add `GET /fleet/api/v1/me` handler + `MeResponse` + route | Modify |
| `src/api/utils.rs` | shared `referrer_conflict_message` formatter | Modify |
| `tests/integration_test.rs` | integration tests for `/me` and driver rate clear | Modify |

---

## Task 1: Extract `double_option` to a shared helper (DRY)

`double_option` is currently a private fn in `src/models/terminal.rs:10`. Driver
overrides need the same helper, so move it to `src/models/mod.rs` as
`pub(crate)` and have terminal reuse it. Pure refactor — existing terminal tests
must stay green.

**Files:**
- Modify: `src/models/mod.rs`
- Modify: `src/models/terminal.rs:9-16` (remove local fn), `src/models/terminal.rs:74` (import path unchanged via re-export)

- [ ] **Step 1: Add the shared helper to `src/models/mod.rs`**

Append to the end of `src/models/mod.rs`:

```rust
/// Serde deserializer for `Option<Option<T>>` "double option" fields.
///
/// Pair with `#[serde(default, deserialize_with = "double_option")]`:
/// - absent field  → `None`        ("leave unchanged")
/// - explicit null → `Some(None)`  ("clear")
/// - a value       → `Some(Some)`  ("set")
pub(crate) fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}
```

- [ ] **Step 2: Remove the private copy from `src/models/terminal.rs`**

Delete lines 9-16 (the `/// Pair with …` doc comment through the closing `}` of
the local `fn double_option`).

- [ ] **Step 3: Point terminal at the shared helper**

In `src/models/terminal.rs`, the attribute `#[serde(default, deserialize_with = "double_option")]`
on line 74 must resolve to the shared fn. Add an import near the top of the file
(after the existing `use` lines, around line 5):

```rust
use super::double_option;
```

- [ ] **Step 4: Build + run terminal tests to verify the refactor is green**

Run: `cargo test --lib models::terminal`
Expected: PASS (no behavior change). If it reports `double_option` unused or
unresolved, recheck the import path in Step 3.

- [ ] **Step 5: Commit**

```bash
git add src/models/mod.rs src/models/terminal.rs
git commit -m "refactor: extract double_option serde helper to models::mod"
```

---

## Task 2: `GET /fleet/api/v1/me` — identity + effective scopes

Any authenticated fleet principal can read its own identity and authority. No
`require_scope` gate (a user must always be able to learn what it can do). The
auth middleware already populates `claims.effective_scopes` per request
(`src/api/fleet_portal/middleware.rs:67`); identity (name/email/role) comes from
a DB lookup via the existing `caller_identity` helper.

**Files:**
- Modify: `src/api/fleet_portal/users.rs` (add `MeResponse`, `me` handler, route)
- Test: `tests/integration_test.rs`

- [ ] **Step 1: Write the failing integration test**

Append to `tests/integration_test.rs`:

```rust
#[tokio::test]
async fn test_me_returns_owner_identity_and_scopes() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;

    let resp = server.get("/fleet/api/v1/me")
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .await;

    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    assert_eq!(body["email"], OWNER_EMAIL);
    assert_eq!(body["role"], "owner");
    // Owner role bundle is the global superuser scope.
    let scopes: Vec<String> = body["effective_scopes"]
        .as_array().unwrap().iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    assert!(scopes.contains(&"*".to_string()), "owner should have * scope, got {scopes:?}");
}

#[tokio::test]
async fn test_me_without_auth_returns_401() {
    let (server, _b, _d, _rx) = test_server().await;
    let resp = server.get("/fleet/api/v1/me").await;
    assert_eq!(resp.status_code(), 401);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test integration_test test_me_returns_owner_identity_and_scopes`
Expected: FAIL — route not found (404), so the `status_code() == 200` assert trips.

- [ ] **Step 3: Add `MeResponse` and the `me` handler**

In `src/api/fleet_portal/users.rs`, add the response struct near the other
response types (after the imports block, before the HTTP handlers section around
line 370). `serde::Serialize` and `utoipa::ToSchema` are already used in this
file:

```rust
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct MeResponse {
    /// Null when the principal is an API key with no parseable fleet_user id.
    pub fleet_user_id: Option<Uuid>,
    pub name: Option<String>,
    pub email: Option<String>,
    /// Role string: "owner" | "fleet_manager" | "dispatcher".
    pub role: String,
    /// Effective authorization scopes resolved server-side this request.
    pub effective_scopes: Vec<String>,
}
```

Then add the handler alongside the other handlers (e.g. after `get_user`,
around line 416). It does not call `require_scope`:

```rust
#[utoipa::path(
    get,
    path = "/fleet/api/v1/me",
    responses(
        (status = 200, description = "Authenticated principal identity + scopes", body = MeResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("BearerAuth" = [])),
    tag = "users"
)]
pub async fn me(
    State(state): State<AppState>,
    Extension(claims): Extension<FleetUserClaims>,
) -> Result<impl IntoResponse, AppError> {
    // caller_identity parses claims.fleet_user_id and reads role fresh from the
    // DB (falling back to Dispatcher), already defined in this module.
    let (id, role) = caller_identity(&state, &claims).await;
    let (name, email) = match id {
        Some(uid) => match state.db.get_fleet_user_by_id(uid).await {
            Ok(u) => (Some(u.name), Some(u.email)),
            Err(_) => (None, None),
        },
        None => (None, None),
    };
    Ok(Json(MeResponse {
        fleet_user_id: id,
        name,
        email,
        role: role.as_str().to_string(),
        effective_scopes: claims.effective_scopes.clone(),
    }))
}
```

- [ ] **Step 4: Wire the route into `users::router()`**

In `src/api/fleet_portal/users.rs`, the `router()` fn (line 527) imports
`get`/`put`. Add the `/me` route inside the `Router::new()` chain (e.g. directly
after `Router::new()`):

```rust
        .route("/fleet/api/v1/me", get(me))
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --test integration_test test_me_`
Expected: PASS for both `test_me_returns_owner_identity_and_scopes` and
`test_me_without_auth_returns_401`.

- [ ] **Step 6: Commit**

```bash
git add src/api/fleet_portal/users.rs tests/integration_test.rs
git commit -m "feat: add GET /fleet/api/v1/me identity + scopes endpoint"
```

---

## Task 3: Driver rate-override three-state clear (`Option<Option<f64>>`)

Today driver rate overrides are `Option<f64>` (absent == null == "leave
unchanged"), so an override can be set but never cleared back to inherited. Move
the five `UpdateDriverRequest` rate fields to `Option<Option<T>>` via
`double_option`, and update the db merge so an explicit `null` clears.

**Files:**
- Modify: `src/models/driver.rs:131-142` (`UpdateDriverRequest` rate fields)
- Modify: `src/db/driver_ops.rs:145-165` (`update_driver_rate_overrides`)
- Test: `tests/integration_test.rs`

Reference — `apply_driver_patch` (`src/api/fleet_portal/driver_writes.rs:504-524`)
gates on `req.<field>.is_some()` and forwards the values to
`update_driver_rate_overrides`. With `Option<Option<T>>`, `is_some()` means
"present (set or clear)", so the gate already does the right thing and needs **no
change**.

- [ ] **Step 1: Write the failing integration tests**

Append to `tests/integration_test.rs`:

```rust
// Reads a driver's stored loaded_rate_per_mile override (null when inherited).
async fn driver_loaded_rate(server: &axum_test::TestServer, token: &str, id: &str) -> serde_json::Value {
    server.get(&format!("/fleet/api/v1/drivers/{id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["loaded_rate_per_mile"].clone()
}

#[tokio::test]
async fn test_driver_rate_override_set_then_clear() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let driver_id = create_test_driver(&server).await;

    // Set an override.
    let set = server.patch(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": 0.75 }))
        .await;
    assert_eq!(set.status_code(), 200);
    assert_eq!(driver_loaded_rate(&server, &owner_token, &driver_id).await, serde_json::json!(0.75));

    // Clear it with an explicit null → back to inherited (null).
    let clear = server.patch(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": null }))
        .await;
    assert_eq!(clear.status_code(), 200);
    assert!(driver_loaded_rate(&server, &owner_token, &driver_id).await.is_null());
}

#[tokio::test]
async fn test_driver_rate_override_absent_field_leaves_others_unchanged() {
    let (server, _b, _d, _rx) = test_server().await;
    let owner_token = setup_owner(&server).await;
    let driver_id = create_test_driver(&server).await;

    // Set loaded rate.
    server.patch(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "loaded_rate_per_mile": 0.75 }))
        .await;
    // Patch a *different* rate without mentioning loaded → loaded must survive.
    server.patch(&format!("/fleet/api/v1/drivers/{driver_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
        .json(&serde_json::json!({ "deadhead_rate_per_mile": 0.40 }))
        .await;

    assert_eq!(driver_loaded_rate(&server, &owner_token, &driver_id).await, serde_json::json!(0.75));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test integration_test test_driver_rate_override`
Expected: FAIL — with the current `Option<f64>`, sending `null` deserializes to
`None` ("leave unchanged"), so the clear test's `is_null()` assert trips.

- [ ] **Step 3: Change the `UpdateDriverRequest` rate fields**

In `src/models/driver.rs`, add the shared helper import near the top `use` block:

```rust
use super::double_option;
```

Replace the five rate-override fields (currently `#[serde(default)] pub <name>: Option<T>`,
`src/models/driver.rs:133-142`) with double-option variants:

```rust
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub loaded_rate_per_mile: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub deadhead_rate_per_mile: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub extra_stop_fee: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<f64>)]
    pub detention_rate_per_hour: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<u32>)]
    pub free_dwell_minutes: Option<Option<u32>>,
```

Leave `terminal_id: Option<Uuid>` unchanged (not a clearable rate).

- [ ] **Step 4: Update the db merge to honor clear**

In `src/db/driver_ops.rs`, change `update_driver_rate_overrides`
(`src/db/driver_ops.rs:145-165`) so the rate params are `Option<Option<T>>` and
an inner `None` clears. The signature's `terminal_id: Option<Uuid>` stays as-is:

```rust
    pub async fn update_driver_rate_overrides(
        &self,
        id: Uuid,
        terminal_id: Option<Uuid>,
        loaded_rate_per_mile: Option<Option<f64>>,
        deadhead_rate_per_mile: Option<Option<f64>>,
        extra_stop_fee: Option<Option<f64>>,
        detention_rate_per_hour: Option<Option<f64>>,
        free_dwell_minutes: Option<Option<u32>>,
    ) -> Result<DriverRecord, AppError> {
        let mut record = self.get_driver_by_id(id).await?;
        if let Some(v) = terminal_id { record.terminal_id = Some(v); }
        // Outer Some = field present; inner Option = set (Some) or clear (None).
        if let Some(v) = loaded_rate_per_mile { record.loaded_rate_per_mile = v; }
        if let Some(v) = deadhead_rate_per_mile { record.deadhead_rate_per_mile = v; }
        if let Some(v) = extra_stop_fee { record.extra_stop_fee = v; }
        if let Some(v) = detention_rate_per_hour { record.detention_rate_per_hour = v; }
        if let Some(v) = free_dwell_minutes { record.free_dwell_minutes = v; }
        record.updated_at = chrono::Utc::now();
        self.upsert_driver(&record).await?;
        Ok(record)
    }
```

(`apply_driver_patch` already forwards `req.<field>` and gates on `.is_some()`;
no change needed there — the forwarded type is now `Option<Option<T>>`, which
matches the new signature.)

- [ ] **Step 4b: Fix the existing db unit test for the new signature**

The existing `test_update_driver_rate_overrides` (`src/db/driver_ops.rs:515-523`)
passes the old `Some(v)` (set-only) args. Wrap the rate args in the outer option
(`terminal_id` stays `None`). Replace the `db.update_driver_rate_overrides(...)`
call block (lines 515-523) with:

```rust
        let updated = db.update_driver_rate_overrides(
            d.id,
            None,                  // keep existing terminal_id
            Some(Some(0.70)),      // loaded_rate_per_mile (set)
            Some(Some(0.35)),      // deadhead_rate_per_mile (set)
            None,                  // extra_stop_fee absent (unchanged)
            Some(Some(28.0)),      // detention_rate_per_hour (set)
            Some(Some(90)),        // free_dwell_minutes (set)
        ).await.unwrap();
```

Add a clear-path assertion at the end of the test (before the closing `}` on
line 532), proving an inner `None` clears a previously-set override:

```rust
        // Clear loaded_rate_per_mile via an explicit inner None.
        let cleared = db.update_driver_rate_overrides(
            d.id, None, Some(None), None, None, None, None,
        ).await.unwrap();
        assert_eq!(cleared.loaded_rate_per_mile, None);
        // deadhead untouched (absent) — still set.
        assert_eq!(cleared.deadhead_rate_per_mile, Some(0.35));
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --test integration_test test_driver_rate_override`
Expected: PASS for both tests.

- [ ] **Step 6: Run the full suite to confirm no regressions**

Run: `cargo test`
Expected: PASS. (The `apply_driver_patch` callers and any existing driver tests
compile against the new `Option<Option<T>>` forwarding.)

- [ ] **Step 7: Commit**

```bash
git add src/models/driver.rs src/db/driver_ops.rs tests/integration_test.rs
git commit -m "feat: support clearing driver rate overrides via explicit null"
```

---

## Task 4: Shared referrer-conflict message formatter

Every entity's permanent (hard) delete handler in later phases must refuse with
`409` and a message that enumerates referrers. Land the one entity-agnostic,
pure piece now so all phases format it identically. (Per-entity referrer
*queries* live in their own phases.)

**Files:**
- Modify: `src/api/utils.rs` (add `referrer_conflict_message` + unit tests)

Reference — the existing pattern is `AppError::Conflict(String)` → HTTP 409
(see `src/api/fleet_portal/blobs.rs:263` and `tests/integration_test.rs:328`
`test_delete_blob_blocked_when_referenced_by_load`).

- [ ] **Step 1: Write the failing unit test**

Append to `src/api/utils.rs`:

```rust
#[cfg(test)]
mod referrer_message_tests {
    use super::*;

    #[test]
    fn formats_single_referrer_kind_with_count() {
        let msg = referrer_conflict_message("driver", &[("trips", 3)]);
        assert_eq!(msg, "cannot permanently delete driver: referenced by 3 trips");
    }

    #[test]
    fn formats_multiple_referrer_kinds() {
        let msg = referrer_conflict_message("truck", &[("trips", 2), ("drivers", 1)]);
        assert_eq!(
            msg,
            "cannot permanently delete truck: referenced by 2 trips, 1 drivers"
        );
    }

    #[test]
    fn skips_zero_count_referrers() {
        let msg = referrer_conflict_message("facility", &[("loads", 0), ("trips", 4)]);
        assert_eq!(msg, "cannot permanently delete facility: referenced by 4 trips");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib referrer_message_tests`
Expected: FAIL — `referrer_conflict_message` is not defined.

- [ ] **Step 3: Implement the formatter**

Add to `src/api/utils.rs` (above the test module):

```rust
/// Build the 409 body for a blocked permanent delete.
///
/// `entity` is the singular noun being deleted ("driver"); `referrers` pairs a
/// referrer kind ("trips") with how many point at the object. Zero-count pairs
/// are skipped. Callers wrap the result in `AppError::Conflict`.
pub fn referrer_conflict_message(entity: &str, referrers: &[(&str, usize)]) -> String {
    let parts: Vec<String> = referrers
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(kind, n)| format!("{n} {kind}"))
        .collect();
    format!(
        "cannot permanently delete {entity}: referenced by {}",
        parts.join(", ")
    )
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib referrer_message_tests`
Expected: PASS (all three).

- [ ] **Step 5: Commit**

```bash
git add src/api/utils.rs
git commit -m "feat: add shared referrer-conflict message formatter for hard deletes"
```

---

## Done criteria

- [ ] `cargo test` passes.
- [ ] `GET /fleet/api/v1/me` returns identity + `effective_scopes` for an
      authenticated principal, 401 otherwise.
- [ ] Driver rate overrides can be set, cleared (explicit `null`), and left
      unchanged (absent) independently per field.
- [ ] `referrer_conflict_message` is available for later phases' hard-delete
      handlers.
- [ ] `double_option` lives in `models::mod` and is shared by terminal + driver.
