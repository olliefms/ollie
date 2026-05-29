# Refresh-Token Foundation + PWA Migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the "refresh requires an unexpired JWT" scheme with real, long-lived, rotating refresh tokens shared by the dispatcher and driver PWA login flows, so a user is never logged out as long as they return within 14 days.

**Architecture:** A new LanceDB-backed `refresh_tokens` table + ops module, a pure-logic refresh-token service (generate / issue / rotate-with-reuse-detection / revoke), and handler changes that issue a refresh token at login (in an HttpOnly cookie), rotate it on `/refresh` (working even after the 8h access token has expired), and revoke it on a new `/logout`. The access token stays the existing short-lived JWT, unchanged. This plan is Plan 1 of 2; Plan 2 (the OAuth Authorization Server) builds on this refresh service.

**Tech Stack:** Rust, axum 0.7, LanceDB 0.29 (arrow_array RecordBatch ops), chrono, sha2, uuid, rand, axum-test.

---

## Conventions (read before starting)

- **Formatting:** the repo is hand-formatted and NOT rustfmt-compliant; there is no CI fmt check. Match the surrounding compact style by hand. **Never run `cargo fmt`/`cargo fmt --all`.**
- **LanceDB `Utf8` trap:** `DataType::Utf8` is valid ONLY in the Rust Arrow schema. Never put `Utf8` in a SQL string (`only_if` filters, etc.) — SQL uses `STRING`/`VARCHAR`. All three OAuth/refresh tables are brand-new and created via the `open_or_create` + full Arrow schema pattern, so this plan never touches the `add_columns`/`CAST(NULL AS …)` backfill path.
- **Null columns in batches:** always build with an explicit typed array (`StringArray::from(vec![opt.as_deref()])`) so a null column stays `Utf8`-typed, per `src/db/dispatcher_api_key_ops.rs`.
- **Verify with:** `cargo test` and `cargo clippy --all-targets -- -D warnings`. A pre-existing `rustfmt --check` "failure" is expected — ignore it.
- **Per-task loop:** write failing test → run it (see it fail) → implement → run it (see it pass) → `cargo clippy` → commit.

## File Structure

- **Create** `src/models/refresh_token.rs` — the `RefreshToken` struct (one responsibility: the record shape).
- **Modify** `src/models/mod.rs` — register + re-export the model.
- **Modify** `src/db/mod.rs` — `refresh_token_schema()`, `open_or_create` call, `DbClient.refresh_token_table` field, wire into `new()`.
- **Create** `src/db/refresh_token_ops.rs` — all `DbClient` CRUD for refresh tokens (insert / get-by-hash / upsert / revoke-family / list-by-family) + unit tests.
- **Create** `src/api/refresh_tokens.rs` — pure refresh-token service: generate, issue, rotate (with reuse detection), build/clear the Set-Cookie header. Portal-agnostic; reused by both PWA portals and (in Plan 2) the OAuth token endpoint. + unit tests.
- **Modify** `src/api/dispatcher_portal/auth.rs` — `login` issues a refresh token + sets cookie; `refresh` reads the cookie and rotates; new `logout`.
- **Modify** `src/api/dispatcher_portal/mod.rs` — add the `/dispatch/auth/logout` route.
- **Modify** `src/api/driver_portal/auth.rs` and `src/api/driver_portal/mod.rs` — same three changes for the driver PIN/passkey flow.
- **Modify** `src/config.rs` — add `cookie_secure: bool` (derive from `public_base_url` scheme) for the `Secure` cookie attribute in tests vs prod.
- **Create** `tests/refresh_token_flow.rs` (or follow the repo's existing integration-test location) — end-to-end: login sets cookie, refresh-after-expiry, rotation, reuse→revoke, logout, token_version kill.

---

## Task 1: `RefreshToken` model

**Files:**
- Create: `src/models/refresh_token.rs`
- Modify: `src/models/mod.rs`

- [ ] **Step 1: Write the model + a round-trip-shape test**

Create `src/models/refresh_token.rs`:

```rust
// src/models/refresh_token.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A rotating refresh token. `token_hash` is the SHA-256 hex of the opaque
/// secret (the secret itself is never stored). Rows in one rotation chain
/// share a `family_id`; replay of a `consumed_at`-set token revokes the family.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RefreshToken {
    pub id: Uuid,
    pub token_hash: String,
    /// "dispatcher" or "driver"
    pub subject_type: String,
    pub subject_id: Uuid,
    /// NULL = PWA session; set = an OAuth client (Plan 2).
    pub client_id: Option<Uuid>,
    pub family_id: Uuid,
    /// Snapshot of the subject's `token_version` at issue; rotation rejects on mismatch.
    pub token_version: i64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_token_is_active_predicate() {
        let now = Utc::now();
        let active = RefreshToken {
            id: Uuid::new_v4(),
            token_hash: "h".into(),
            subject_type: "dispatcher".into(),
            subject_id: Uuid::new_v4(),
            client_id: None,
            family_id: Uuid::new_v4(),
            token_version: 0,
            issued_at: now,
            expires_at: now + chrono::Duration::days(14),
            consumed_at: None,
            revoked_at: None,
            last_used_at: None,
        };
        assert!(active.revoked_at.is_none() && active.consumed_at.is_none());
        assert!(active.expires_at > now);
    }
}
```

- [ ] **Step 2: Register in `src/models/mod.rs`**

Find the existing `mod dispatcher_api_key;` / `pub use dispatcher_api_key::*;` lines and add the matching pair (match the file's exact ordering/style):

```rust
mod refresh_token;
pub use refresh_token::*;
```

- [ ] **Step 3: Run the test**

Run: `cargo test --lib models::refresh_token`
Expected: PASS (1 test).

- [ ] **Step 4: Clippy + commit**

```bash
cargo clippy --all-targets -- -D warnings
git add src/models/refresh_token.rs src/models/mod.rs
git commit -m "feat(models): add RefreshToken record"
```

---

## Task 2: LanceDB schema + table registration

**Files:**
- Modify: `src/db/mod.rs`

Study the existing `dispatcher_api_key_schema()` / `open_or_create(...)` / `DbClient` field / `new()` wiring in `src/db/mod.rs` and mirror it exactly.

- [ ] **Step 1: Add the schema function**

In `src/db/mod.rs`, next to `dispatcher_api_key_schema()`, add (note `token_version` is `Int64`, everything else `Utf8`; nullable flags match `Option<…>` fields):

```rust
pub fn refresh_token_schema() -> std::sync::Arc<arrow_schema::Schema> {
    std::sync::Arc::new(arrow_schema::Schema::new(vec![
        arrow_schema::Field::new("id", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("token_hash", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("subject_type", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("subject_id", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("client_id", arrow_schema::DataType::Utf8, true),
        arrow_schema::Field::new("family_id", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("token_version", arrow_schema::DataType::Int64, false),
        arrow_schema::Field::new("issued_at", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("expires_at", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("consumed_at", arrow_schema::DataType::Utf8, true),
        arrow_schema::Field::new("revoked_at", arrow_schema::DataType::Utf8, true),
        arrow_schema::Field::new("last_used_at", arrow_schema::DataType::Utf8, true),
    ]))
}
```

> Match how the file refers to arrow types — if `dispatcher_api_key_schema()` imports `arrow_schema::{Schema, Field, DataType}` at the top and uses the short names, use the short names here too instead of the fully-qualified paths above.

- [ ] **Step 2: Add the table field to `DbClient`**

In the `pub struct DbClient { … }` block, after `pub dispatcher_api_key_table: Table,` add:

```rust
    pub refresh_token_table: Table,
```

- [ ] **Step 3: Open/create it in `new()`**

In `DbClient::new`, mirror the `dispatcher_api_key_table` line. The api-key table uses the generic `open_or_create(&conn, "dispatcher_api_keys", dispatcher_api_key_schema()).await?` helper (confirm the exact helper signature in this file and match it):

```rust
        let refresh_token_table = open_or_create(
            &conn,
            "refresh_tokens",
            refresh_token_schema(),
        ).await?;
```

Then add `refresh_token_table,` to the struct-construction list returned at the end of `new()` (next to `dispatcher_api_key_table,`).

- [ ] **Step 4: Compile**

Run: `cargo build`
Expected: builds clean (no handlers use the table yet).

- [ ] **Step 5: Clippy + commit**

```bash
cargo clippy --all-targets -- -D warnings
git add src/db/mod.rs
git commit -m "feat(db): register refresh_tokens LanceDB table"
```

---

## Task 3: Refresh-token DB ops

**Files:**
- Create: `src/db/refresh_token_ops.rs`
- Modify: `src/db/mod.rs` (add `mod refresh_token_ops;`)

This mirrors `src/db/dispatcher_api_key_ops.rs` (read it first for the `collect_stream`, batch-build, and row-read patterns). The one new wrinkle is the `Int64` column `token_version`.

- [ ] **Step 1: Write the ops module with tests**

Create `src/db/refresh_token_ops.rs`:

```rust
// src/db/refresh_token_ops.rs
use crate::{
    db::{refresh_token_schema, DbClient},
    error::AppError,
    models::RefreshToken,
};
use arrow_array::{Array, Int64Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_refresh_token(&self, record: &RefreshToken) -> Result<(), AppError> {
        let batch = refresh_token_to_batch(record)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], refresh_token_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.refresh_token_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn upsert_refresh_token(&self, record: &RefreshToken) -> Result<(), AppError> {
        let batch = refresh_token_to_batch(record)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], refresh_token_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.refresh_token_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_refresh_token_by_hash(&self, token_hash: &str) -> Result<Option<RefreshToken>, AppError> {
        let escaped = token_hash.replace('\'', "''");
        let stream = self.refresh_token_table.query()
            .only_if(format!("token_hash = '{escaped}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_refresh_tokens(collect_stream(stream).await?)?;
        Ok(records.pop())
    }

    pub async fn list_refresh_tokens_by_family(&self, family_id: Uuid) -> Result<Vec<RefreshToken>, AppError> {
        let fam = family_id.to_string();
        let stream = self.refresh_token_table.query()
            .only_if(format!("family_id = '{fam}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_refresh_tokens(collect_stream(stream).await?)
    }

    /// Revoke every row in a family (theft response / logout). Sets `revoked_at` on each.
    pub async fn revoke_refresh_token_family(&self, family_id: Uuid, now: chrono::DateTime<chrono::Utc>) -> Result<(), AppError> {
        let rows = self.list_refresh_tokens_by_family(family_id).await?;
        for mut row in rows {
            if row.revoked_at.is_none() {
                row.revoked_at = Some(now);
                self.upsert_refresh_token(&row).await?;
            }
        }
        Ok(())
    }
}

fn refresh_token_to_batch(r: &RefreshToken) -> Result<RecordBatch, AppError> {
    let schema = refresh_token_schema();
    let id = r.id.to_string();
    let subject_id = r.subject_id.to_string();
    let client_id = r.client_id.map(|c| c.to_string());
    let family_id = r.family_id.to_string();
    let issued = r.issued_at.to_rfc3339();
    let expires = r.expires_at.to_rfc3339();
    let consumed = r.consumed_at.as_ref().map(|d| d.to_rfc3339());
    let revoked = r.revoked_at.as_ref().map(|d| d.to_rfc3339());
    let last_used = r.last_used_at.as_ref().map(|d| d.to_rfc3339());

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![id.as_str()])),
        Arc::new(StringArray::from(vec![r.token_hash.as_str()])),
        Arc::new(StringArray::from(vec![r.subject_type.as_str()])),
        Arc::new(StringArray::from(vec![subject_id.as_str()])),
        Arc::new(StringArray::from(vec![client_id.as_deref()])),
        Arc::new(StringArray::from(vec![family_id.as_str()])),
        Arc::new(Int64Array::from(vec![r.token_version])),
        Arc::new(StringArray::from(vec![issued.as_str()])),
        Arc::new(StringArray::from(vec![expires.as_str()])),
        Arc::new(StringArray::from(vec![consumed.as_deref()])),
        Arc::new(StringArray::from(vec![revoked.as_deref()])),
        Arc::new(StringArray::from(vec![last_used.as_deref()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_refresh_tokens(batches: Vec<RecordBatch>) -> Result<Vec<RefreshToken>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_refresh_token(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_refresh_token(batch: &RecordBatch, i: usize) -> Result<RefreshToken, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string())
            .unwrap_or_default()
    };
    let opt_str = |name: &str| -> Option<String> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) })
    };
    let i64_col = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i))
            .unwrap_or_default()
    };
    let parse_dt = |s: String| s.parse::<chrono::DateTime<chrono::Utc>>()
        .map_err(|e| AppError::Internal(e.to_string()));
    let parse_uuid = |s: String| s.parse::<Uuid>()
        .map_err(|e: uuid::Error| AppError::Internal(e.to_string()));

    Ok(RefreshToken {
        id: parse_uuid(str_col("id"))?,
        token_hash: str_col("token_hash"),
        subject_type: str_col("subject_type"),
        subject_id: parse_uuid(str_col("subject_id"))?,
        client_id: opt_str("client_id").map(parse_uuid).transpose()?,
        family_id: parse_uuid(str_col("family_id"))?,
        token_version: i64_col("token_version"),
        issued_at: parse_dt(str_col("issued_at"))?,
        expires_at: parse_dt(str_col("expires_at"))?,
        consumed_at: opt_str("consumed_at").map(parse_dt).transpose()?,
        revoked_at: opt_str("revoked_at").map(parse_dt).transpose()?,
        last_used_at: opt_str("last_used_at").map(parse_dt).transpose()?,
    })
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

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample(family_id: Uuid, hash: &str) -> RefreshToken {
        let now = Utc::now();
        RefreshToken {
            id: Uuid::new_v4(),
            token_hash: hash.into(),
            subject_type: "dispatcher".into(),
            subject_id: Uuid::new_v4(),
            client_id: None,
            family_id,
            token_version: 0,
            issued_at: now,
            expires_at: now + chrono::Duration::days(14),
            consumed_at: None,
            revoked_at: None,
            last_used_at: None,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_by_hash() {
        let (db, _d) = test_db().await;
        let rt = sample(Uuid::new_v4(), "hash_a");
        db.insert_refresh_token(&rt).await.unwrap();
        let got = db.get_refresh_token_by_hash("hash_a").await.unwrap().unwrap();
        assert_eq!(got.id, rt.id);
        assert_eq!(got.token_version, 0);
        assert!(got.client_id.is_none());
    }

    #[tokio::test]
    async fn test_get_by_hash_missing() {
        let (db, _d) = test_db().await;
        assert!(db.get_refresh_token_by_hash("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_upsert_marks_consumed() {
        let (db, _d) = test_db().await;
        let mut rt = sample(Uuid::new_v4(), "hash_b");
        db.insert_refresh_token(&rt).await.unwrap();
        rt.consumed_at = Some(Utc::now());
        db.upsert_refresh_token(&rt).await.unwrap();
        let got = db.get_refresh_token_by_hash("hash_b").await.unwrap().unwrap();
        assert!(got.consumed_at.is_some());
    }

    #[tokio::test]
    async fn test_revoke_family_revokes_all_rows() {
        let (db, _d) = test_db().await;
        let fam = Uuid::new_v4();
        db.insert_refresh_token(&sample(fam, "hash_c1")).await.unwrap();
        db.insert_refresh_token(&sample(fam, "hash_c2")).await.unwrap();
        db.revoke_refresh_token_family(fam, Utc::now()).await.unwrap();
        let rows = db.list_refresh_tokens_by_family(fam).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.revoked_at.is_some()));
    }
}
```

- [ ] **Step 2: Register the module in `src/db/mod.rs`**

Add next to `mod dispatcher_api_key_ops;`:

```rust
mod refresh_token_ops;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test refresh_token_ops`
Expected: 4 tests PASS.

- [ ] **Step 4: Clippy + commit**

```bash
cargo clippy --all-targets -- -D warnings
git add src/db/refresh_token_ops.rs src/db/mod.rs
git commit -m "feat(db): refresh_tokens CRUD ops"
```

---

## Task 4: Refresh-token service (generate / issue / rotate / cookie)

**Files:**
- Create: `src/api/refresh_tokens.rs`
- Modify: `src/api/mod.rs` (add `pub mod refresh_tokens;`)

Pure, portal-agnostic logic. No HTTP handler here — just functions the handlers call. The constants live here.

- [ ] **Step 1: Write the service + unit tests**

Create `src/api/refresh_tokens.rs`:

```rust
// src/api/refresh_tokens.rs
//
// Shared refresh-token machinery for the PWA login flows (this plan) and the
// OAuth token endpoint (Plan 2). Access tokens stay short-lived JWTs; these
// opaque, hashed, rotating tokens carry the long-lived session.
use crate::{db::DbClient, error::AppError, models::RefreshToken};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const REFRESH_TTL_DAYS: i64 = 14;
/// Cookie name for the PWA refresh token.
pub const REFRESH_COOKIE: &str = "ollie_refresh";

pub fn hash_token(plaintext: &str) -> String {
    hex::encode(Sha256::digest(plaintext.as_bytes()))
}

/// 32 bytes of CSPRNG, base64url (no padding), `ollr_`-prefixed for greppability.
fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    use base64::Engine;
    format!("ollr_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// The plaintext secret (returned to the caller once) plus the stored row.
pub struct IssuedToken {
    pub secret: String,
    pub record: RefreshToken,
}

/// Mint a brand-new refresh token starting a fresh family. Persists the row.
pub async fn issue(
    db: &DbClient,
    subject_type: &str,
    subject_id: Uuid,
    client_id: Option<Uuid>,
    token_version: i64,
    now: DateTime<Utc>,
) -> Result<IssuedToken, AppError> {
    let secret = generate_secret();
    let record = RefreshToken {
        id: Uuid::new_v4(),
        token_hash: hash_token(&secret),
        subject_type: subject_type.to_string(),
        subject_id,
        client_id,
        family_id: Uuid::new_v4(),
        token_version,
        issued_at: now,
        expires_at: now + Duration::days(REFRESH_TTL_DAYS),
        consumed_at: None,
        revoked_at: None,
        last_used_at: None,
    };
    db.insert_refresh_token(&record).await?;
    Ok(IssuedToken { secret, record })
}

/// Outcome of presenting a refresh token.
pub enum RotateResult {
    /// New secret + new row; the access token should be re-minted with `token_version`.
    Rotated(IssuedToken),
    /// Token missing/expired/revoked, or `token_version` mismatch — re-auth required.
    Invalid,
    /// A consumed token was replayed: the family has been revoked. Re-auth required.
    ReusedFamilyRevoked,
}

/// Validate + rotate. Consumes the presented row, appends a new row in the same
/// family with a fresh TTL. Replaying a consumed token revokes the whole family.
/// `current_token_version` is the subject's live `token_version` (the kill switch).
pub async fn rotate(
    db: &DbClient,
    presented_secret: &str,
    current_token_version: i64,
    now: DateTime<Utc>,
) -> Result<RotateResult, AppError> {
    let hash = hash_token(presented_secret);
    let row = match db.get_refresh_token_by_hash(&hash).await? {
        Some(r) => r,
        None => return Ok(RotateResult::Invalid),
    };

    if row.revoked_at.is_some() || row.expires_at <= now {
        return Ok(RotateResult::Invalid);
    }
    if row.token_version != current_token_version {
        return Ok(RotateResult::Invalid);
    }
    // Reuse detection: a consumed token presented again ⇒ theft ⇒ revoke family.
    if row.consumed_at.is_some() {
        db.revoke_refresh_token_family(row.family_id, now).await?;
        return Ok(RotateResult::ReusedFamilyRevoked);
    }

    // Consume the presented row.
    let mut consumed = row.clone();
    consumed.consumed_at = Some(now);
    consumed.last_used_at = Some(now);
    db.upsert_refresh_token(&consumed).await?;

    // Append a new row in the same family with a fresh TTL.
    let secret = generate_secret();
    let next = RefreshToken {
        id: Uuid::new_v4(),
        token_hash: hash_token(&secret),
        subject_type: row.subject_type.clone(),
        subject_id: row.subject_id,
        client_id: row.client_id,
        family_id: row.family_id,
        token_version: current_token_version,
        issued_at: now,
        expires_at: now + Duration::days(REFRESH_TTL_DAYS),
        consumed_at: None,
        revoked_at: None,
        last_used_at: None,
    };
    db.insert_refresh_token(&next).await?;
    Ok(RotateResult::Rotated(IssuedToken { secret, record: next }))
}

/// `Set-Cookie` value for the refresh token (HttpOnly, SameSite=Lax, Path=/).
/// `secure` adds the `Secure` attribute (true in prod / https).
pub fn set_cookie_header(secret: &str, secure: bool) -> String {
    let max_age = REFRESH_TTL_DAYS * 24 * 3600;
    let mut c = format!(
        "{REFRESH_COOKIE}={secret}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}"
    );
    if secure {
        c.push_str("; Secure");
    }
    c
}

/// `Set-Cookie` value that clears the refresh cookie (logout).
pub fn clear_cookie_header(secure: bool) -> String {
    let mut c = format!("{REFRESH_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    if secure {
        c.push_str("; Secure");
    }
    c
}

/// Extract the refresh cookie value from a Cookie header, if present.
pub fn read_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == REFRESH_COOKIE)
        .map(|(_, v)| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    #[tokio::test]
    async fn test_issue_then_rotate_returns_new_secret() {
        let (db, _d) = test_db().await;
        let subj = Uuid::new_v4();
        let issued = issue(&db, "dispatcher", subj, None, 0, Utc::now()).await.unwrap();
        let r = rotate(&db, &issued.secret, 0, Utc::now()).await.unwrap();
        match r {
            RotateResult::Rotated(next) => {
                assert_ne!(next.secret, issued.secret);
                assert_eq!(next.record.family_id, issued.record.family_id);
            }
            _ => panic!("expected Rotated"),
        }
    }

    #[tokio::test]
    async fn test_reused_token_revokes_family() {
        let (db, _d) = test_db().await;
        let subj = Uuid::new_v4();
        let issued = issue(&db, "dispatcher", subj, None, 0, Utc::now()).await.unwrap();
        // First rotate consumes the original.
        let _ = rotate(&db, &issued.secret, 0, Utc::now()).await.unwrap();
        // Replaying the original (now consumed) must trip theft detection.
        let again = rotate(&db, &issued.secret, 0, Utc::now()).await.unwrap();
        assert!(matches!(again, RotateResult::ReusedFamilyRevoked));
        // And the family is dead: the rotated child can no longer rotate.
        let fam_rows = db.list_refresh_tokens_by_family(issued.record.family_id).await.unwrap();
        assert!(fam_rows.iter().all(|r| r.revoked_at.is_some()));
    }

    #[tokio::test]
    async fn test_token_version_mismatch_is_invalid() {
        let (db, _d) = test_db().await;
        let subj = Uuid::new_v4();
        let issued = issue(&db, "dispatcher", subj, None, 0, Utc::now()).await.unwrap();
        // Subject bumped token_version to 1 (e.g. password change) ⇒ refresh rejected.
        let r = rotate(&db, &issued.secret, 1, Utc::now()).await.unwrap();
        assert!(matches!(r, RotateResult::Invalid));
    }

    #[tokio::test]
    async fn test_expired_token_is_invalid() {
        let (db, _d) = test_db().await;
        let subj = Uuid::new_v4();
        // Issue "15 days ago" so it's already past the 14d TTL.
        let past = Utc::now() - chrono::Duration::days(15);
        let issued = issue(&db, "dispatcher", subj, None, 0, past).await.unwrap();
        let r = rotate(&db, &issued.secret, 0, Utc::now()).await.unwrap();
        assert!(matches!(r, RotateResult::Invalid));
    }

    #[test]
    fn test_cookie_headers() {
        let set = set_cookie_header("ollr_abc", true);
        assert!(set.contains("ollie_refresh=ollr_abc"));
        assert!(set.contains("HttpOnly") && set.contains("Secure") && set.contains("Max-Age=1209600"));
        let clear = clear_cookie_header(false);
        assert!(clear.contains("Max-Age=0"));
        assert!(!clear.contains("Secure"));
    }

    #[test]
    fn test_read_cookie() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(axum::http::header::COOKIE, "foo=1; ollie_refresh=ollr_xyz; bar=2".parse().unwrap());
        assert_eq!(read_cookie(&h), Some("ollr_xyz".to_string()));
        let empty = axum::http::HeaderMap::new();
        assert_eq!(read_cookie(&empty), None);
    }
}
```

- [ ] **Step 2: Ensure deps are available**

`sha2`, `hex`, `uuid`, `chrono`, `rand` are already in the tree (used by api keys / jwt). Confirm `base64` is a dependency:

Run: `grep -E '^base64' Cargo.toml`
If absent, add `base64 = "0.22"` under `[dependencies]` (match the formatting of nearby entries) and re-run. (`rand` is already used elsewhere; confirm with `grep -E '^rand' Cargo.toml` and add `rand = "0.8"` only if missing.)

- [ ] **Step 3: Register the module**

In `src/api/mod.rs`, add near the other `pub mod` lines:

```rust
pub mod refresh_tokens;
```

- [ ] **Step 4: Run tests**

Run: `cargo test refresh_tokens`
Expected: 6 tests PASS.

- [ ] **Step 5: Clippy + commit**

```bash
cargo clippy --all-targets -- -D warnings
git add src/api/refresh_tokens.rs src/api/mod.rs Cargo.toml
git commit -m "feat(api): shared refresh-token service (issue/rotate/cookie)"
```

---

## Task 5: Config — `cookie_secure` flag

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Add the field + derive it**

In `pub struct Config { … }` add:

```rust
    pub cookie_secure: bool,
```

In the loader (where `public_base_url` is read), derive it and add `cookie_secure,` to the struct construction:

```rust
        let cookie_secure = public_base_url.starts_with("https://");
```

- [ ] **Step 2: Fix the config test constructor**

There is a `Config` test fixture (around the `assert_eq!(cfg.admin_api_key, "test-key")` test). Add `cookie_secure: false,` to any literal `Config { … }` construction so the test compiles.

- [ ] **Step 3: Build + commit**

Run: `cargo test config`
Expected: PASS.

```bash
cargo clippy --all-targets -- -D warnings
git add src/config.rs
git commit -m "feat(config): add cookie_secure derived from public_base_url"
```

---

## Task 6: Dispatcher PWA — issue at login, rotate on refresh, add logout

**Files:**
- Modify: `src/api/dispatcher_portal/auth.rs`
- Modify: `src/api/dispatcher_portal/mod.rs`

Reuses the existing bcrypt verification in `login`; only the success path and `refresh` change, plus a new `logout`. Response bodies still return `{ "token": "<access JWT>" }`; the refresh token rides in the `Set-Cookie` header.

- [ ] **Step 1: Update `login` to also issue a refresh cookie**

In `src/api/dispatcher_portal/auth.rs`, add imports at the top:

```rust
use crate::api::refresh_tokens;
use axum::http::header::SET_COOKIE;
```

Replace the success return (currently `Ok((StatusCode::OK, Json(LoginResponse { token })).into_response())`) with:

```rust
    let issued = refresh_tokens::issue(
        &state.db, "dispatcher", dispatcher.id, None, creds.token_version, Utc::now(),
    ).await?;
    let cookie = refresh_tokens::set_cookie_header(&issued.secret, state.config.cookie_secure);

    let mut response = (StatusCode::OK, Json(LoginResponse { token })).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        cookie.parse().map_err(|_| AppError::Internal("bad cookie".into()))?,
    );
    Ok(response)
```

- [ ] **Step 2: Rewrite `refresh` to rotate the cookie token (works after access-token expiry)**

Replace the body of `refresh` with a cookie-based rotation. The old JWT `iat`-window logic is removed — the refresh token is now the long-lived credential, independent of the (possibly expired) access token:

```rust
pub async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let secret = refresh_tokens::read_cookie(&headers).ok_or(AppError::Unauthorized)?;

    // We need the subject + current token_version to validate. The refresh row
    // carries subject_id; look it up after a successful rotate. To know the live
    // token_version, fetch by the row first:
    let hash = refresh_tokens::hash_token(&secret);
    let row = state.db.get_refresh_token_by_hash(&hash).await?
        .ok_or(AppError::Unauthorized)?;
    if row.subject_type != "dispatcher" {
        return Err(AppError::Unauthorized);
    }
    let creds = state.db.get_dispatcher_credentials(row.subject_id).await?
        .ok_or(AppError::Unauthorized)?;

    match refresh_tokens::rotate(&state.db, &secret, creds.token_version, Utc::now()).await? {
        refresh_tokens::RotateResult::Rotated(next) => {
            let dispatcher = state.db.get_dispatcher_by_id(row.subject_id).await
                .map_err(|_| AppError::Unauthorized)?;
            if dispatcher.status == DispatcherStatus::Inactive {
                return Err(AppError::Unauthorized);
            }
            let token = encode_dispatcher_jwt(row.subject_id, creds.token_version, &state.config.dispatcher_jwt_secret)?;
            let cookie = refresh_tokens::set_cookie_header(&next.secret, state.config.cookie_secure);
            let mut response = Json(LoginResponse { token }).into_response();
            response.headers_mut().insert(
                SET_COOKIE,
                cookie.parse().map_err(|_| AppError::Internal("bad cookie".into()))?,
            );
            Ok(response)
        }
        _ => Err(AppError::Unauthorized),
    }
}
```

> Remove the now-unused `decode_dispatcher_jwt` import if nothing else in the file uses it (clippy will tell you).

- [ ] **Step 3: Add `logout`**

Append to `src/api/dispatcher_portal/auth.rs`:

```rust
/// Revoke the caller's refresh-token family and clear the cookie.
#[utoipa::path(
    post,
    path = "/dispatch/auth/logout",
    responses((status = 200, description = "Logged out")),
    tag = "dispatch-auth"
)]
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    if let Some(secret) = refresh_tokens::read_cookie(&headers) {
        let hash = refresh_tokens::hash_token(&secret);
        if let Some(row) = state.db.get_refresh_token_by_hash(&hash).await? {
            state.db.revoke_refresh_token_family(row.family_id, Utc::now()).await?;
        }
    }
    let cookie = refresh_tokens::clear_cookie_header(state.config.cookie_secure);
    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        cookie.parse().map_err(|_| AppError::Internal("bad cookie".into()))?,
    );
    Ok(response)
}
```

- [ ] **Step 4: Route it**

In `src/api/dispatcher_portal/mod.rs` `auth_router()`, add:

```rust
        .route("/dispatch/auth/logout", post(auth::logout))
```

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: clean (fix any unused-import warnings clippy flags).

- [ ] **Step 6: Clippy + commit**

```bash
cargo clippy --all-targets -- -D warnings
git add src/api/dispatcher_portal/auth.rs src/api/dispatcher_portal/mod.rs
git commit -m "feat(dispatch-auth): issue/rotate refresh-token cookie; add logout"
```

---

## Task 7: Driver PWA — same three changes

**Files:**
- Modify: `src/api/driver_portal/auth.rs`
- Modify: `src/api/driver_portal/mod.rs`

Mirror Task 6 for the driver flow. Driver login is via PIN/passkey; locate the success path(s) in `driver_portal/auth.rs` (the `pin_auth` handler ends by issuing a driver JWT — that's where to add the issue+cookie) and the driver `refresh` handler.

- [ ] **Step 1: Imports** — add to `src/api/driver_portal/auth.rs`:

```rust
use crate::api::refresh_tokens;
use axum::http::header::SET_COOKIE;
```

- [ ] **Step 2: Issue at login** — at each driver-auth success point that returns a JWT, before returning, issue the refresh token with `subject_type = "driver"` and the driver's id + the driver creds' `token_version`, then attach the `Set-Cookie` header to the response (same shape as Task 6 Step 1). Use the driver's `get_driver_credentials` equivalent for `token_version` (match the existing call the driver `refresh` uses).

```rust
    let issued = refresh_tokens::issue(
        &state.db, "driver", driver.id, None, creds.token_version, Utc::now(),
    ).await?;
    let cookie = refresh_tokens::set_cookie_header(&issued.secret, state.config.cookie_secure);
    let mut response = (StatusCode::OK, Json(/* existing driver login body */)).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        cookie.parse().map_err(|_| AppError::Internal("bad cookie".into()))?,
    );
    Ok(response)
```

- [ ] **Step 3: Rotate on refresh** — rewrite the driver `refresh` handler to read the cookie and rotate, mirroring Task 6 Step 2 but with `subject_type == "driver"`, the driver credential lookup, the driver status check, and `encode_driver_jwt` (use the driver portal's existing JWT encoder name — check `driver_portal/jwt.rs`).

- [ ] **Step 4: Add driver `logout`** — same as Task 6 Step 3, path `/driver/api/v1/auth/logout` (match the driver portal's auth path prefix in `driver_portal/mod.rs`), and route it.

- [ ] **Step 5: Build + clippy + commit**

Run: `cargo build && cargo clippy --all-targets -- -D warnings`
Expected: clean.

```bash
git add src/api/driver_portal/auth.rs src/api/driver_portal/mod.rs
git commit -m "feat(driver-auth): issue/rotate refresh-token cookie; add logout"
```

---

## Task 8: Integration tests — the end-to-end behaviors

**Files:**
- Create: `tests/refresh_token_flow.rs` (or, if the repo keeps integration tests as `#[cfg(test)]` modules, add a module under `src/api/dispatcher_portal/` mirroring the existing middleware tests — check where dispatcher integration tests live and follow that).

These assert the user-visible guarantees, including the regression test for the overnight-logout bug.

- [ ] **Step 1: Write the tests**

The exact harness (building a `TestServer` with a real `AppState` over a `TempDir` DB) should mirror how existing dispatcher integration tests construct the app. Cover:

```rust
// Behaviors to assert (translate to the repo's existing TestServer setup):
//
// 1. login_sets_refresh_cookie:
//    POST /dispatch/auth/login (valid creds) → 200, body has `token`,
//    response has a `Set-Cookie: ollie_refresh=ollr_…; HttpOnly; …` header.
//
// 2. refresh_works_after_access_token_expiry (THE regression test):
//    After login, discard the access JWT entirely (simulate >8h idle) and call
//    POST /dispatch/auth/refresh sending ONLY the refresh cookie (no Authorization
//    header) → 200 with a fresh `token` and a rotated `Set-Cookie`.
//
// 3. refresh_rotates_secret:
//    Two sequential refreshes each return a different cookie value; the first
//    cookie no longer works after the second refresh (rotation).
//
// 4. reused_refresh_token_revokes_family:
//    Capture cookie C0; refresh once (→ C1); replay C0 → 401; then C1 also → 401
//    (family revoked by the reuse of C0).
//
// 5. logout_revokes_and_clears:
//    POST /dispatch/auth/logout with the cookie → 200 with a clearing Set-Cookie
//    (Max-Age=0); a subsequent refresh with the old cookie → 401.
//
// 6. token_version_bump_kills_refresh:
//    After login, bump the dispatcher's token_version in the DB; refresh with the
//    cookie → 401 (the kill switch covers refresh tokens, not just access tokens).
```

Implement each as a `#[tokio::test]`, building the server the same way the existing dispatcher tests do, seeding a dispatcher with a known bcrypt password via the DB ops, and reading the `set-cookie` header from responses (`resp.headers().get_all("set-cookie")`).

- [ ] **Step 2: Run**

Run: `cargo test refresh_token_flow` (or the module path you used)
Expected: all 6 PASS.

- [ ] **Step 3: Full suite + clippy + commit**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
git add tests/refresh_token_flow.rs
git commit -m "test: end-to-end refresh-token PWA flow incl. overnight-refresh regression"
```

---

## Task 9: Front-end token handling (SPA) — note + verification

**Files:**
- Modify: `static/dispatch/*` and `static/driver/*` front-end auth code (locations TBD by inspecting the SPA build).

The backend now sets/reads an HttpOnly cookie the JS cannot see. The SPA must (a) send credentials on `/refresh` and `/logout` (`fetch(..., { credentials: "same-origin" })`), (b) stop trying to read/store the refresh token (it's invisible by design), and (c) on a 401 from a data call, attempt `/refresh` once before redirecting to login.

- [ ] **Step 1:** Locate the SPA's auth/refresh code (search `static/dispatch` for `auth/refresh` and the access-token storage). Confirm `fetch` calls to `/dispatch/auth/refresh` and `/logout` include `credentials: "same-origin"`. Adjust if not.
- [ ] **Step 2:** Manually verify in a browser: log in, wait past (or force-expire) the 8h access token, perform an action → the app silently refreshes and does NOT bounce to login. This is the human-facing acceptance test; if the SPA build can't be exercised, say so explicitly rather than claiming success.
- [ ] **Step 3: Commit** any SPA changes:

```bash
git add static/dispatch static/driver
git commit -m "fix(spa): use cookie credentials for refresh/logout; silent refresh on 401"
```

---

## Self-Review Notes (addressed)

- **Spec coverage:** unified refresh-token model (table, 14d sliding, rotation+reuse-detection, hashed-at-rest, HttpOnly cookie, token_version kill switch) — Tasks 1–8. PWA dispatcher + driver migration — Tasks 6–7, 9. Access token unchanged (still 8h JWT) — preserved by reusing `encode_dispatcher_jwt`. OAuth client rows share this table via the nullable `client_id` — schema ready (Task 1/2), exercised in Plan 2.
- **Out of scope here (Plan 2):** `oauth_clients`/`authorization_codes` tables, DCR, discovery metadata, `/authorize`+consent page, `/token` endpoint, `WWW-Authenticate` on `/dispatch/mcp`.
- **`Utf8` trap:** all schema types are `DataType::Utf8` in Rust; no SQL casts anywhere; `only_if` filters compare string columns with quoted literals only.
- **Type consistency:** `hash_token`, `issue`, `rotate`, `RotateResult`, `set_cookie_header`, `clear_cookie_header`, `read_cookie`, `REFRESH_COOKIE` are defined in Task 4 and used unchanged in Tasks 6–7.
```
