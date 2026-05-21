# Dispatcher API Keys — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add long-lived bearer API keys for dispatcher auth so Claude's remote MCP connector can hit `/dispatch/mcp` directly without a local proxy.

**Architecture:** New LanceDB table `dispatcher_api_keys` stores keys hashed with SHA-256. The middleware (`require_dispatcher_auth`) branches on bearer prefix: `olld_*` validates via DB lookup, everything else validates as JWT. Downstream handlers are unchanged.

**Tech Stack:** Rust/Axum/LanceDB. All dependencies already in `Cargo.toml` (`sha2`, `hex`, `base64`, `uuid`, `chrono`). No new deps needed.

**Spec:** `docs/superpowers/specs/2026-05-20-dispatcher-api-keys-design.md`

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `src/models/dispatcher_api_key.rs` | **Create** | `DispatcherApiKey` DB model struct |
| `src/models/mod.rs` | **Modify** | expose new module |
| `src/error.rs` | **Modify** | add `TooManyRequests` variant (HTTP 429) |
| `src/db/mod.rs` | **Modify** | schema fn, empty batch fn, `dispatcher_api_key_table` field + init |
| `src/db/dispatcher_api_key_ops.rs` | **Create** | insert, upsert, list, get_by_hash, get_by_id, count_active |
| `src/api/dispatcher_portal/jwt.rs` | **Modify** | extend `DispatcherClaims` with optional `api_key_id`/`api_key_label` |
| `src/api/dispatcher_portal/middleware.rs` | **Modify** | rename to `require_dispatcher_auth`, add API-key branch |
| `src/api/dispatcher_portal/api_keys.rs` | **Create** | key generation, request/response types, 3 handlers |
| `src/api/dispatcher_portal/mod.rs` | **Modify** | declare `api_keys` module, add routes, update middleware ref |
| `tests/integration_test.rs` | **Modify** | integration tests for full happy/error path |

---

## Task 1: DispatcherApiKey model

**Files:**
- Create: `src/models/dispatcher_api_key.rs`
- Modify: `src/models/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/models/dispatcher_api_key.rs`:

```rust
// src/models/dispatcher_api_key.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatcherApiKey {
    pub id: Uuid,
    pub dispatcher_id: Uuid,
    pub label: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dispatcher_api_key_clone() {
        let now = Utc::now();
        let key = DispatcherApiKey {
            id: Uuid::new_v4(),
            dispatcher_id: Uuid::new_v4(),
            label: "test".into(),
            key_hash: "abc".into(),
            key_prefix: "olld_a1b2c3".into(),
            created_at: now,
            expires_at: now + chrono::Duration::days(365),
            revoked_at: None,
            last_used_at: None,
        };
        let clone = key.clone();
        assert_eq!(clone.id, key.id);
        assert_eq!(clone.label, "test");
    }
}
```

- [ ] **Step 2: Run test to verify it compiles and passes** (module not yet in mod.rs so expect compile error)

```bash
cargo test -p ollie dispatcher_api_key 2>&1 | head -20
```

Expected: compile error about missing module.

- [ ] **Step 3: Add to models/mod.rs**

In `src/models/mod.rs`, add after the last `pub mod` line:

```rust
pub mod dispatcher_api_key;
```

And after the last `pub use` line:

```rust
pub use dispatcher_api_key::*;
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p ollie dispatcher_api_key 2>&1 | tail -5
```

Expected: `test models::dispatcher_api_key::tests::test_dispatcher_api_key_clone ... ok`

- [ ] **Step 5: Commit**

```bash
git add src/models/dispatcher_api_key.rs src/models/mod.rs
git commit -m "feat(api-keys): add DispatcherApiKey model"
```

---

## Task 2: TooManyRequests error variant

**Files:**
- Modify: `src/error.rs`

- [ ] **Step 1: Write the failing test**

In `src/error.rs` tests block, add:

```rust
#[test]
fn test_too_many_requests_status() {
    assert_eq!(status_of(AppError::TooManyRequests), StatusCode::TOO_MANY_REQUESTS);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p ollie test_too_many_requests 2>&1 | tail -5
```

Expected: compile error, `TooManyRequests` not found.

- [ ] **Step 3: Add variant to AppError**

In `src/error.rs`, add to the enum after `UnprocessableEntity`:

```rust
#[error("too many requests")]
TooManyRequests,
```

Add to the `match &self` in `into_response`:

```rust
Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p ollie test_too_many_requests 2>&1 | tail -5
```

Expected: `test error::tests::test_too_many_requests_status ... ok`

- [ ] **Step 5: Commit**

```bash
git add src/error.rs
git commit -m "feat(api-keys): add TooManyRequests error variant"
```

---

## Task 3: DB schema and table initialisation

**Files:**
- Modify: `src/db/mod.rs`

- [ ] **Step 1: Write the failing test**

In `src/db/mod.rs` tests block, add:

```rust
#[tokio::test]
async fn test_db_client_has_dispatcher_api_key_table() {
    let dir = TempDir::new().unwrap();
    let client = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
    assert_eq!(client.dispatcher_api_key_table.count_rows(None).await.unwrap(), 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p ollie test_db_client_has_dispatcher_api_key_table 2>&1 | tail -5
```

Expected: compile error, `dispatcher_api_key_table` field doesn't exist.

- [ ] **Step 3: Add schema function and empty batch to db/mod.rs**

Add after `empty_dispatcher_credentials_batch` (around line 666):

```rust
pub fn dispatcher_api_key_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("dispatcher_id", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("key_hash", DataType::Utf8, false),
        Field::new("key_prefix", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("expires_at", DataType::Utf8, false),
        Field::new("revoked_at", DataType::Utf8, true),
        Field::new("last_used_at", DataType::Utf8, true),
    ]))
}

fn empty_dispatcher_api_key_batch(schema: Arc<Schema>) -> Result<RecordBatch, AppError> {
    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}
```

- [ ] **Step 4: Add field to DbClient struct**

In `pub struct DbClient`, add after `dispatcher_credentials_table`:

```rust
pub dispatcher_api_key_table: Table,
```

- [ ] **Step 5: Initialize in DbClient::new()**

In `DbClient::new()`, add after the `dispatcher_credentials_table` init (before `Ok(Self { ... })`):

```rust
let dispatcher_api_key_table = open_or_create(
    &conn,
    "dispatcher_api_keys",
    dispatcher_api_key_schema(),
    empty_dispatcher_api_key_batch,
).await?;
```

And add `dispatcher_api_key_table,` to the `Ok(Self { ... })` field list.

- [ ] **Step 6: Run test to verify it passes**

```bash
cargo test -p ollie test_db_client_has_dispatcher_api_key_table 2>&1 | tail -5
```

Expected: `test db::tests::test_db_client_has_dispatcher_api_key_table ... ok`

- [ ] **Step 7: Commit**

```bash
git add src/db/mod.rs
git commit -m "feat(api-keys): add dispatcher_api_key LanceDB table and schema"
```

---

## Task 4: DB CRUD operations

**Files:**
- Create: `src/db/dispatcher_api_key_ops.rs`
- Modify: `src/db/mod.rs` (add module declaration)

- [ ] **Step 1: Write the failing tests**

Create `src/db/dispatcher_api_key_ops.rs` with just the tests (no implementation yet):

```rust
// src/db/dispatcher_api_key_ops.rs
use crate::{
    db::{dispatcher_api_key_schema, DbClient},
    error::AppError,
    models::DispatcherApiKey,
};
use arrow_array::{RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_dispatcher_api_key(&self, _record: &DispatcherApiKey) -> Result<(), AppError> {
        todo!()
    }

    pub async fn upsert_dispatcher_api_key(&self, _record: &DispatcherApiKey) -> Result<(), AppError> {
        todo!()
    }

    pub async fn get_dispatcher_api_key_by_hash(&self, _key_hash: &str) -> Result<Option<DispatcherApiKey>, AppError> {
        todo!()
    }

    pub async fn get_dispatcher_api_key_by_id(&self, _id: Uuid, _dispatcher_id: Uuid) -> Result<Option<DispatcherApiKey>, AppError> {
        todo!()
    }

    pub async fn list_active_dispatcher_api_keys(&self, _dispatcher_id: Uuid) -> Result<Vec<DispatcherApiKey>, AppError> {
        todo!()
    }

    pub async fn count_active_dispatcher_api_keys(&self, _dispatcher_id: Uuid) -> Result<usize, AppError> {
        todo!()
    }
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

    fn sample_key(dispatcher_id: Uuid) -> DispatcherApiKey {
        let now = Utc::now();
        DispatcherApiKey {
            id: Uuid::new_v4(),
            dispatcher_id,
            label: "Test Key".into(),
            key_hash: "abc123hash".into(),
            key_prefix: "olld_a1b2c3".into(),
            created_at: now,
            expires_at: now + chrono::Duration::days(365),
            revoked_at: None,
            last_used_at: None,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_by_hash() {
        let (db, _dir) = test_db().await;
        let dispatcher_id = Uuid::new_v4();
        let key = sample_key(dispatcher_id);
        let hash = key.key_hash.clone();
        db.insert_dispatcher_api_key(&key).await.unwrap();
        let found = db.get_dispatcher_api_key_by_hash(&hash).await.unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.id, key.id);
        assert_eq!(found.label, "Test Key");
    }

    #[tokio::test]
    async fn test_get_by_hash_not_found() {
        let (db, _dir) = test_db().await;
        let result = db.get_dispatcher_api_key_by_hash("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_list_active_excludes_revoked() {
        let (db, _dir) = test_db().await;
        let dispatcher_id = Uuid::new_v4();
        let active = sample_key(dispatcher_id);
        let mut revoked = sample_key(dispatcher_id);
        revoked.id = Uuid::new_v4();
        revoked.key_hash = "other_hash".into();
        revoked.revoked_at = Some(Utc::now());
        db.insert_dispatcher_api_key(&active).await.unwrap();
        db.insert_dispatcher_api_key(&revoked).await.unwrap();
        let list = db.list_active_dispatcher_api_keys(dispatcher_id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, active.id);
    }

    #[tokio::test]
    async fn test_list_active_only_own_keys() {
        let (db, _dir) = test_db().await;
        let d1 = Uuid::new_v4();
        let d2 = Uuid::new_v4();
        let k1 = sample_key(d1);
        let mut k2 = sample_key(d2);
        k2.key_hash = "other_hash2".into();
        db.insert_dispatcher_api_key(&k1).await.unwrap();
        db.insert_dispatcher_api_key(&k2).await.unwrap();
        let list = db.list_active_dispatcher_api_keys(d1).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].dispatcher_id, d1);
    }

    #[tokio::test]
    async fn test_upsert_revokes_key() {
        let (db, _dir) = test_db().await;
        let dispatcher_id = Uuid::new_v4();
        let key = sample_key(dispatcher_id);
        db.insert_dispatcher_api_key(&key).await.unwrap();
        let mut revoked = key.clone();
        revoked.revoked_at = Some(Utc::now());
        db.upsert_dispatcher_api_key(&revoked).await.unwrap();
        let list = db.list_active_dispatcher_api_keys(dispatcher_id).await.unwrap();
        assert_eq!(list.len(), 0);
    }

    #[tokio::test]
    async fn test_get_by_id_ownership() {
        let (db, _dir) = test_db().await;
        let d1 = Uuid::new_v4();
        let d2 = Uuid::new_v4();
        let key = sample_key(d1);
        db.insert_dispatcher_api_key(&key).await.unwrap();
        // correct owner finds it
        let found = db.get_dispatcher_api_key_by_id(key.id, d1).await.unwrap();
        assert!(found.is_some());
        // wrong owner gets None
        let not_found = db.get_dispatcher_api_key_by_id(key.id, d2).await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_count_active_excludes_expired_and_revoked() {
        let (db, _dir) = test_db().await;
        let dispatcher_id = Uuid::new_v4();
        let valid = sample_key(dispatcher_id);
        let mut expired = sample_key(dispatcher_id);
        expired.id = Uuid::new_v4();
        expired.key_hash = "hash_expired".into();
        expired.expires_at = Utc::now() - chrono::Duration::days(1);
        let mut revoked = sample_key(dispatcher_id);
        revoked.id = Uuid::new_v4();
        revoked.key_hash = "hash_revoked".into();
        revoked.revoked_at = Some(Utc::now());
        db.insert_dispatcher_api_key(&valid).await.unwrap();
        db.insert_dispatcher_api_key(&expired).await.unwrap();
        db.insert_dispatcher_api_key(&revoked).await.unwrap();
        let count = db.count_active_dispatcher_api_keys(dispatcher_id).await.unwrap();
        assert_eq!(count, 1);
    }
}
```

- [ ] **Step 2: Add module to db/mod.rs**

In `src/db/mod.rs`, add after the last `pub mod` line:

```rust
pub mod dispatcher_api_key_ops;
```

- [ ] **Step 3: Run tests to verify they compile and fail**

```bash
cargo test -p ollie dispatcher_api_key_ops 2>&1 | tail -10
```

Expected: tests panic with `not yet implemented` (todo!()).

- [ ] **Step 4: Implement the DB ops**

Replace the todo!() stubs in `src/db/dispatcher_api_key_ops.rs` with the full implementation:

```rust
// src/db/dispatcher_api_key_ops.rs
use crate::{
    db::{dispatcher_api_key_schema, DbClient},
    error::AppError,
    models::DispatcherApiKey,
};
use arrow_array::{RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_dispatcher_api_key(&self, record: &DispatcherApiKey) -> Result<(), AppError> {
        let batch = api_key_to_batch(record)?;
        let schema = dispatcher_api_key_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.dispatcher_api_key_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn upsert_dispatcher_api_key(&self, record: &DispatcherApiKey) -> Result<(), AppError> {
        let batch = api_key_to_batch(record)?;
        let schema = dispatcher_api_key_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.dispatcher_api_key_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_dispatcher_api_key_by_hash(&self, key_hash: &str) -> Result<Option<DispatcherApiKey>, AppError> {
        let escaped = key_hash.replace('\'', "''");
        let stream = self.dispatcher_api_key_table.query()
            .only_if(format!("key_hash = '{escaped}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_api_keys(collect_stream(stream).await?)?;
        Ok(records.pop())
    }

    pub async fn get_dispatcher_api_key_by_id(&self, id: Uuid, dispatcher_id: Uuid) -> Result<Option<DispatcherApiKey>, AppError> {
        let id_str = id.to_string();
        let dispatcher_id_str = dispatcher_id.to_string();
        let stream = self.dispatcher_api_key_table.query()
            .only_if(format!("id = '{id_str}' AND dispatcher_id = '{dispatcher_id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_api_keys(collect_stream(stream).await?)?;
        Ok(records.pop())
    }

    pub async fn list_active_dispatcher_api_keys(&self, dispatcher_id: Uuid) -> Result<Vec<DispatcherApiKey>, AppError> {
        let id_str = dispatcher_id.to_string();
        let stream = self.dispatcher_api_key_table.query()
            .only_if(format!("dispatcher_id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let records = batches_to_api_keys(collect_stream(stream).await?)?;
        Ok(records.into_iter().filter(|k| k.revoked_at.is_none()).collect())
    }

    pub async fn count_active_dispatcher_api_keys(&self, dispatcher_id: Uuid) -> Result<usize, AppError> {
        let keys = self.list_active_dispatcher_api_keys(dispatcher_id).await?;
        let now = Utc::now();
        Ok(keys.iter().filter(|k| k.expires_at > now).count())
    }
}

fn api_key_to_batch(record: &DispatcherApiKey) -> Result<RecordBatch, AppError> {
    let schema = dispatcher_api_key_schema();
    let id_str = record.id.to_string();
    let dispatcher_id_str = record.dispatcher_id.to_string();
    let created_str = record.created_at.to_rfc3339();
    let expires_str = record.expires_at.to_rfc3339();
    let revoked_str = record.revoked_at.as_ref().map(|dt| dt.to_rfc3339());
    let last_used_str = record.last_used_at.as_ref().map(|dt| dt.to_rfc3339());

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![id_str.as_str()])),
        Arc::new(StringArray::from(vec![dispatcher_id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.label.as_str()])),
        Arc::new(StringArray::from(vec![record.key_hash.as_str()])),
        Arc::new(StringArray::from(vec![record.key_prefix.as_str()])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
        Arc::new(StringArray::from(vec![expires_str.as_str()])),
        Arc::new(StringArray::from(vec![revoked_str.as_deref()])),
        Arc::new(StringArray::from(vec![last_used_str.as_deref()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_api_keys(batches: Vec<RecordBatch>) -> Result<Vec<DispatcherApiKey>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_api_key(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_api_key(batch: &RecordBatch, i: usize) -> Result<DispatcherApiKey, AppError> {
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

    Ok(DispatcherApiKey {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        dispatcher_id: str_col("dispatcher_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        label: str_col("label"),
        key_hash: str_col("key_hash"),
        key_prefix: str_col("key_prefix"),
        created_at: str_col("created_at").parse().map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        expires_at: str_col("expires_at").parse().map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        revoked_at: opt_str("revoked_at")
            .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
            .transpose()
            .map_err(|e| AppError::Internal(e.to_string()))?,
        last_used_at: opt_str("last_used_at")
            .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
            .transpose()
            .map_err(|e| AppError::Internal(e.to_string()))?,
    })
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    // (tests from Step 1 above — already written)
}
```

- [ ] **Step 5: Run tests to verify they all pass**

```bash
cargo test -p ollie dispatcher_api_key_ops 2>&1 | tail -15
```

Expected: all 7 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/db/dispatcher_api_key_ops.rs src/db/mod.rs
git commit -m "feat(api-keys): add dispatcher_api_key DB operations"
```

---

## Task 5: Extend DispatcherClaims

**Files:**
- Modify: `src/api/dispatcher_portal/jwt.rs`

- [ ] **Step 1: Run existing tests to confirm baseline**

```bash
cargo test -p ollie api::dispatcher_portal::jwt 2>&1 | tail -5
```

Expected: 3 tests pass.

- [ ] **Step 2: Add optional fields to DispatcherClaims**

In `src/api/dispatcher_portal/jwt.rs`, update the `DispatcherClaims` struct. The new fields go after `kid`:

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DispatcherClaims {
    pub dispatcher_id: String,
    pub token_version: i64,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub kid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_label: Option<String>,
}
```

- [ ] **Step 3: Run existing tests to verify they still pass (JWT decode must handle absent new fields)**

```bash
cargo test -p ollie api::dispatcher_portal::jwt 2>&1 | tail -5
```

Expected: all 3 pass (JWT decode populates new fields as None via serde(default)).

- [ ] **Step 4: Commit**

```bash
git add src/api/dispatcher_portal/jwt.rs
git commit -m "feat(api-keys): extend DispatcherClaims with optional api_key_id/label"
```

---

## Task 6: Update middleware — rename and add API-key branch

**Files:**
- Modify: `src/api/dispatcher_portal/middleware.rs`

- [ ] **Step 1: Run existing middleware tests as baseline**

```bash
cargo test -p ollie api::dispatcher_portal::middleware 2>&1 | tail -5
```

Expected: 3 tests pass.

- [ ] **Step 2: Replace middleware.rs content**

Replace `src/api/dispatcher_portal/middleware.rs` entirely:

```rust
// src/api/dispatcher_portal/middleware.rs
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use crate::{
    AppState,
    error::AppError,
    models::DispatcherStatus,
};

use super::jwt::{decode_dispatcher_jwt, DispatcherClaims};

pub async fn require_dispatcher_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?
        .to_owned();

    let claims = if token.starts_with("olld_") {
        validate_api_key(&state, &token).await?
    } else {
        validate_jwt_token(&state, &token).await?
    };

    request.extensions_mut().insert(claims);
    Ok(next.run(request).await)
}

async fn validate_jwt_token(state: &AppState, token: &str) -> Result<DispatcherClaims, AppError> {
    let claims = decode_dispatcher_jwt(token, &state.config.dispatcher_jwt_secret)?;

    let dispatcher_id: Uuid = claims.dispatcher_id.parse()
        .map_err(|_| AppError::Unauthorized)?;

    let creds = state.db.get_dispatcher_credentials(dispatcher_id).await?
        .ok_or(AppError::Unauthorized)?;

    if creds.token_version != claims.token_version {
        return Err(AppError::Unauthorized);
    }

    let dispatcher = state.db.get_dispatcher_by_id(dispatcher_id).await
        .map_err(|_| AppError::Unauthorized)?;

    if dispatcher.status == DispatcherStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Err(AppError::Unauthorized);
        }
    }

    Ok(claims)
}

async fn validate_api_key(state: &AppState, token: &str) -> Result<DispatcherClaims, AppError> {
    let hash = hex::encode(Sha256::digest(token.as_bytes()));

    let key = state.db.get_dispatcher_api_key_by_hash(&hash).await?
        .ok_or(AppError::Unauthorized)?;

    if key.revoked_at.is_some() || key.expires_at <= Utc::now() {
        return Err(AppError::Unauthorized);
    }

    let dispatcher = state.db.get_dispatcher_by_id(key.dispatcher_id).await
        .map_err(|_| AppError::Unauthorized)?;

    if dispatcher.status == DispatcherStatus::Inactive {
        return Err(AppError::Unauthorized);
    }

    let creds = state.db.get_dispatcher_credentials(key.dispatcher_id).await?
        .ok_or(AppError::Unauthorized)?;

    if let Some(locked_until) = creds.locked_until {
        if locked_until > Utc::now() {
            return Err(AppError::Unauthorized);
        }
    }

    let key_for_touch = key.clone();
    let db_for_touch = state.db.clone();
    tokio::spawn(async move {
        let mut k = key_for_touch;
        k.last_used_at = Some(Utc::now());
        if let Err(e) = db_for_touch.upsert_dispatcher_api_key(&k).await {
            tracing::warn!(key_id = %k.id, err = ?e, "failed to update api key last_used_at");
        }
    });

    Ok(DispatcherClaims {
        dispatcher_id: key.dispatcher_id.to_string(),
        token_version: creds.token_version,
        iss: "ollie-dispatcher".into(),
        aud: "ollie-dispatcher".into(),
        exp: 0,
        iat: 0,
        kid: "api-key".into(),
        api_key_id: Some(key.id),
        api_key_label: Some(key.label),
    })
}

#[cfg(test)]
mod tests {
    use axum::{Router, http::StatusCode, middleware::from_fn, routing::get};
    use axum_test::TestServer;
    use crate::error::AppError;

    async fn stub_auth_middleware(
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> Result<axum::response::Response, AppError> {
        let has_bearer = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| !t.is_empty())
            .unwrap_or(false);
        if !has_bearer {
            return Err(AppError::Unauthorized);
        }
        Ok(next.run(req).await)
    }

    fn protected_app() -> Router {
        Router::new()
            .route("/protected", get(|| async { "ok" }))
            .route_layer(from_fn(stub_auth_middleware))
    }

    fn open_app() -> Router {
        Router::new()
            .route("/open", get(|| async { "open" }))
    }

    #[tokio::test]
    async fn test_require_dispatcher_auth_missing_header() {
        let server = TestServer::new(protected_app()).unwrap();
        let resp = server.get("/protected").await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_require_dispatcher_auth_invalid_token() {
        let server = TestServer::new(protected_app()).unwrap();
        let resp = server.get("/protected")
            .add_header(axum::http::header::AUTHORIZATION, "Bearer ")
            .await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_no_auth_routes_unaffected() {
        let server = TestServer::new(open_app()).unwrap();
        let resp = server.get("/open").await;
        assert_eq!(resp.status_code(), StatusCode::OK);
    }
}
```

- [ ] **Step 3: Fix the middleware reference in mod.rs**

In `src/api/dispatcher_portal/mod.rs`, change the one line that calls the old name (around line 66):

```rust
// old
middleware::require_dispatcher_jwt,
// new
middleware::require_dispatcher_auth,
```

- [ ] **Step 4: Run build and middleware tests**

```bash
cargo build -p ollie 2>&1 | tail -3
cargo test -p ollie api::dispatcher_portal::middleware 2>&1 | tail -5
```

Expected: build succeeds; 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/api/dispatcher_portal/middleware.rs src/api/dispatcher_portal/mod.rs
git commit -m "feat(api-keys): rename middleware to require_dispatcher_auth, add API-key branch"
```

---

## Task 7: API key handlers and generation utilities

**Files:**
- Create: `src/api/dispatcher_portal/api_keys.rs`

- [ ] **Step 1: Write the failing tests (key generation utilities)**

Create `src/api/dispatcher_portal/api_keys.rs` with tests first:

```rust
// src/api/dispatcher_portal/api_keys.rs
use axum::{
    Extension,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use base64::Engine;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    AppState,
    api::dispatcher_portal::jwt::DispatcherClaims,
    error::AppError,
    models::DispatcherApiKey,
};

// --- Key generation ---

pub fn generate_api_key() -> String {
    todo!()
}

pub fn hash_api_key(key: &str) -> String {
    todo!()
}

// --- Request / Response types ---

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateApiKeyRequest {
    pub label: String,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateApiKeyResponse {
    pub id: Uuid,
    pub label: String,
    pub key: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiKeyListItem {
    pub id: Uuid,
    pub label: String,
    pub key_prefix: String,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
    pub last_used_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiKeyListResponse {
    pub keys: Vec<ApiKeyListItem>,
}

// --- Handlers (stubs for now) ---

pub async fn create_api_key(
    _state: State<AppState>,
    _claims: Extension<DispatcherClaims>,
    _req: Json<CreateApiKeyRequest>,
) -> Result<impl IntoResponse, AppError> {
    todo!()
}

pub async fn list_api_keys(
    _state: State<AppState>,
    _claims: Extension<DispatcherClaims>,
) -> Result<impl IntoResponse, AppError> {
    todo!()
}

pub async fn revoke_api_key(
    _state: State<AppState>,
    _claims: Extension<DispatcherClaims>,
    _key_id: Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_generate_api_key_format() {
        let key = generate_api_key();
        assert!(key.starts_with("olld_"), "key must start with olld_: {key}");
        assert_eq!(key.len(), 48, "key must be 48 chars: {key}");
    }

    #[test]
    fn test_generate_api_key_prefix_is_first_12_chars() {
        let key = generate_api_key();
        let prefix = &key[..12];
        assert!(prefix.starts_with("olld_"));
        assert_eq!(prefix.len(), 12);
    }

    #[test]
    fn test_generate_api_key_unique() {
        let keys: HashSet<String> = (0..20).map(|_| generate_api_key()).collect();
        assert_eq!(keys.len(), 20, "all 20 generated keys must be unique");
    }

    #[test]
    fn test_hash_api_key_is_hex_sha256() {
        let hash = hash_api_key("olld_testkey");
        assert_eq!(hash.len(), 64, "SHA-256 hex must be 64 chars");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_api_key_stable() {
        let h1 = hash_api_key("olld_testkey");
        let h2 = hash_api_key("olld_testkey");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_api_key_different_inputs() {
        assert_ne!(hash_api_key("olld_aaa"), hash_api_key("olld_bbb"));
    }
}
```

- [ ] **Step 2: Add module to dispatcher_portal/mod.rs temporarily to compile tests**

In `src/api/dispatcher_portal/mod.rs`, add:

```rust
pub mod api_keys;
```

(Routes wired in Task 8.)

- [ ] **Step 3: Run tests to verify they fail on todo!()**

```bash
cargo test -p ollie api::dispatcher_portal::api_keys 2>&1 | tail -10
```

Expected: compile succeeds, tests panic on `todo!()`.

- [ ] **Step 4: Implement generate_api_key and hash_api_key**

Replace the two todo!() utility functions:

```rust
pub fn generate_api_key() -> String {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(a.as_bytes());
    bytes[16..].copy_from_slice(b.as_bytes());
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes);
    format!("olld_{encoded}")
}

pub fn hash_api_key(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}
```

- [ ] **Step 5: Run utility tests to verify they pass**

```bash
cargo test -p ollie api::dispatcher_portal::api_keys 2>&1 | tail -10
```

Expected: 6 unit tests pass, handler stubs still panic on todo!() but those aren't called from unit tests.

- [ ] **Step 6: Implement the three handlers**

Replace the three handler stubs:

```rust
pub async fn create_api_key(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Json(req): Json<CreateApiKeyRequest>,
) -> Result<impl IntoResponse, AppError> {
    if claims.api_key_id.is_some() {
        return Err(AppError::Unauthorized);
    }

    let label = req.label.trim().to_string();
    if label.is_empty() || label.len() > 64 {
        return Err(AppError::BadRequest("label must be 1–64 characters".into()));
    }
    let expires_in_days = req.expires_in_days.unwrap_or(365);
    if expires_in_days < 1 || expires_in_days > 365 {
        return Err(AppError::BadRequest("expires_in_days must be between 1 and 365".into()));
    }

    let dispatcher_id: Uuid = claims.dispatcher_id.parse().map_err(|_| AppError::Unauthorized)?;

    let active = state.db.count_active_dispatcher_api_keys(dispatcher_id).await?;
    if active >= 20 {
        return Err(AppError::TooManyRequests);
    }

    let now = Utc::now();
    let plaintext = generate_api_key();
    let key_hash = hash_api_key(&plaintext);
    let key_prefix = plaintext[..12].to_string();

    let record = DispatcherApiKey {
        id: Uuid::new_v4(),
        dispatcher_id,
        label: label.clone(),
        key_hash,
        key_prefix: key_prefix.clone(),
        created_at: now,
        expires_at: now + chrono::Duration::days(expires_in_days as i64),
        revoked_at: None,
        last_used_at: None,
    };
    state.db.insert_dispatcher_api_key(&record).await?;

    Ok((StatusCode::CREATED, Json(CreateApiKeyResponse {
        id: record.id,
        label,
        key: plaintext,
        key_prefix,
        created_at: record.created_at,
        expires_at: record.expires_at,
    })))
}

pub async fn list_api_keys(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
) -> Result<impl IntoResponse, AppError> {
    let dispatcher_id: Uuid = claims.dispatcher_id.parse().map_err(|_| AppError::Unauthorized)?;
    let keys = state.db.list_active_dispatcher_api_keys(dispatcher_id).await?;
    let items = keys.into_iter().map(|k| ApiKeyListItem {
        id: k.id,
        label: k.label,
        key_prefix: k.key_prefix,
        created_at: k.created_at,
        expires_at: k.expires_at,
        last_used_at: k.last_used_at,
    }).collect();
    Ok(Json(ApiKeyListResponse { keys: items }))
}

pub async fn revoke_api_key(
    State(state): State<AppState>,
    Extension(claims): Extension<DispatcherClaims>,
    Path(key_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    if claims.api_key_id.is_some() {
        return Err(AppError::Unauthorized);
    }

    let dispatcher_id: Uuid = claims.dispatcher_id.parse().map_err(|_| AppError::Unauthorized)?;

    let key = state.db.get_dispatcher_api_key_by_id(key_id, dispatcher_id).await?
        .ok_or(AppError::NotFound)?;

    if key.revoked_at.is_some() {
        return Err(AppError::NotFound);
    }

    let mut updated = key;
    updated.revoked_at = Some(Utc::now());
    state.db.upsert_dispatcher_api_key(&updated).await?;

    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 7: Verify the build compiles cleanly**

```bash
cargo build -p ollie 2>&1 | tail -5
```

Expected: `Finished` with no errors.

- [ ] **Step 8: Commit**

```bash
git add src/api/dispatcher_portal/api_keys.rs
git commit -m "feat(api-keys): add API key handlers and key generation utilities"
```

---

## Task 8: Wire routes

**Files:**
- Modify: `src/api/dispatcher_portal/mod.rs`

- [ ] **Step 1: Update mod.rs**

Replace the full contents of `src/api/dispatcher_portal/mod.rs`:

```rust
// src/api/dispatcher_portal/mod.rs
pub mod api_keys;
pub mod auth;
pub mod blobs;
pub mod data;
pub mod jwt;
pub mod mcp;
pub mod middleware;

use crate::AppState;
use axum::{
    extract::DefaultBodyLimit,
    Router,
    routing::{delete, get, post},
};

pub fn auth_router() -> Router<AppState> {
    Router::new()
        .route("/dispatch/auth/login", post(auth::login))
        .route("/dispatch/auth/refresh", post(auth::refresh))
}

pub fn data_router(state: &AppState) -> Router<AppState> {
    Router::new()
        .route("/dispatch/api/v1/loads", get(data::list_loads).post(data::create_load))
        .route("/dispatch/api/v1/loads/:id", get(data::get_load).put(data::update_load))
        .route("/dispatch/api/v1/trips", get(data::list_trips))
        .route("/dispatch/api/v1/trips/:id", get(data::get_trip))
        .route("/dispatch/api/v1/trips/:id/assign", post(data::assign_trip))
        .route("/dispatch/api/v1/trips/:id/unassign", post(data::unassign_trip))
        .route("/dispatch/api/v1/trips/:id/dispatch", post(data::dispatch_trip))
        .route("/dispatch/api/v1/trips/:id/undispatch", post(data::undispatch_trip))
        .route("/dispatch/api/v1/trips/:id/cancel", post(data::cancel_trip))
        .route("/dispatch/api/v1/trips/:id/complete", post(data::complete_trip))
        .route("/dispatch/api/v1/trips/:id/stops/:seq/arrive", post(data::stop_arrive))
        .route("/dispatch/api/v1/trips/:id/stops/:seq/depart", post(data::stop_depart))
        .route("/dispatch/api/v1/trips/:id/stops/:seq/late", post(data::stop_late))
        .route("/dispatch/api/v1/trips/:id/check-call", post(data::check_call))
        .route("/dispatch/api/v1/drivers", get(data::list_drivers))
        .route("/dispatch/api/v1/drivers/:id", get(data::get_driver))
        .route("/dispatch/api/v1/trucks", get(data::list_trucks))
        .route("/dispatch/api/v1/trucks/:id", get(data::get_truck))
        .route("/dispatch/api/v1/trailers", get(data::list_trailers))
        .route("/dispatch/api/v1/trailers/:id", get(data::get_trailer))
        .route("/dispatch/api/v1/events", get(data::list_events))
        .route("/dispatch/api/v1/loads/count", get(data::count_open_loads))
        .route("/dispatch/api/v1/drivers/count", get(data::count_active_drivers))
        .route("/dispatch/api/v1/blobs/count", get(data::count_pending_documents))
        .route("/dispatch/api/v1/events/count", get(data::count_events_today))
        .route(
            "/dispatch/api/v1/blobs",
            get(blobs::list_blobs).post(blobs::upload_blob).layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route(
            "/dispatch/api/v1/blob/:id",
            get(blobs::get_blob)
                .put(blobs::update_blob)
                .delete(blobs::delete_blob),
        )
        .route("/dispatch/api/v1/blobs/:id/query", post(blobs::query_blob))
        .route("/dispatch/mcp", post(mcp::handle))
        // API key management (GET allowed for both JWT and API-key auth; POST/DELETE require JWT)
        .route("/dispatch/api-keys", post(api_keys::create_api_key).get(api_keys::list_api_keys))
        .route("/dispatch/api-keys/:id", delete(api_keys::revoke_api_key))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_dispatcher_auth,
        ))
}

pub fn dispatcher_portal_router(state: &AppState) -> Router<AppState> {
    auth_router().merge(data_router(state))
}
```

- [ ] **Step 2: Build and run all dispatcher tests**

```bash
cargo build -p ollie 2>&1 | tail -3
cargo test -p ollie api::dispatcher_portal 2>&1 | tail -15
```

Expected: build succeeds; existing dispatcher tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/api/dispatcher_portal/mod.rs
git commit -m "feat(api-keys): wire /dispatch/api-keys routes"
```

---

## Task 9: Integration tests

**Files:**
- Modify: `tests/integration_test.rs`

- [ ] **Step 1: Run the existing dispatcher integration tests as a baseline**

```bash
cargo test -p ollie test_dispatcher 2>&1 | tail -20
```

Expected: all existing dispatcher tests pass.

- [ ] **Step 2: Add integration tests**

Append the following tests to `tests/integration_test.rs`:

```rust
// ============================================================
// Dispatcher API key tests
// ============================================================

async fn create_dispatcher_api_key(
    server: &axum_test::TestServer,
    token: &str,
    label: &str,
) -> serde_json::Value {
    let resp = server.post("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": label }))
        .await;
    assert_eq!(resp.status_code(), 201, "create api key failed: {}", resp.text());
    resp.json::<serde_json::Value>()
}

#[tokio::test]
async fn test_api_key_create_returns_plaintext_once() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikey1@example.com", "pass-apikey1").await;

    let body = create_dispatcher_api_key(&server, &token, "Claude desktop").await;

    assert!(body["key"].as_str().unwrap().starts_with("olld_"));
    assert_eq!(body["key"].as_str().unwrap().len(), 48);
    assert_eq!(body["label"], "Claude desktop");
    assert!(body["key_prefix"].as_str().is_some());
    assert_eq!(&body["key"].as_str().unwrap()[..12], body["key_prefix"].as_str().unwrap());
    assert!(body["id"].as_str().is_some());
    assert!(body["expires_at"].as_str().is_some());
}

#[tokio::test]
async fn test_api_key_custom_expiry() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikey2@example.com", "pass-apikey2").await;

    let resp = server.post("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": "short-lived", "expires_in_days": 7 }))
        .await;
    assert_eq!(resp.status_code(), 201);
    let body = resp.json::<serde_json::Value>();
    let expires: chrono::DateTime<chrono::Utc> = body["expires_at"].as_str().unwrap().parse().unwrap();
    let created: chrono::DateTime<chrono::Utc> = body["created_at"].as_str().unwrap().parse().unwrap();
    let diff = expires - created;
    assert!(diff.num_days() >= 6 && diff.num_days() <= 8, "expected ~7 day expiry, got {}", diff.num_days());
}

#[tokio::test]
async fn test_api_key_expiry_over_365_rejected() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikey3@example.com", "pass-apikey3").await;

    let resp = server.post("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": "too-long", "expires_in_days": 366 }))
        .await;
    assert_eq!(resp.status_code(), 400);
}

#[tokio::test]
async fn test_api_key_list_returns_own_keys_only() {
    let (server, _b, _d, _rx) = test_server().await;
    let t1 = dispatcher_login(&server, "apikeylist1@example.com", "pass1").await;
    let t2 = dispatcher_login(&server, "apikeylist2@example.com", "pass2").await;

    create_dispatcher_api_key(&server, &t1, "d1-key").await;
    create_dispatcher_api_key(&server, &t2, "d2-key").await;

    let resp = server.get("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {t1}"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.json::<serde_json::Value>();
    let keys = body["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["label"], "d1-key");
    assert!(keys[0]["key"].is_null(), "plaintext must not appear in list");
}

#[tokio::test]
async fn test_api_key_list_excludes_revoked() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeyrev1@example.com", "pass-rev1").await;

    let body = create_dispatcher_api_key(&server, &token, "to-revoke").await;
    let key_id = body["id"].as_str().unwrap();

    let del = server.delete(&format!("/dispatch/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(del.status_code(), 204);

    let list = server.get("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    let list_body = list.json::<serde_json::Value>();
    assert_eq!(list_body["keys"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_api_key_revoke_not_found_for_other_dispatcher() {
    let (server, _b, _d, _rx) = test_server().await;
    let t1 = dispatcher_login(&server, "apikeyown1@example.com", "pass-own1").await;
    let t2 = dispatcher_login(&server, "apikeyown2@example.com", "pass-own2").await;

    let body = create_dispatcher_api_key(&server, &t1, "t1-key").await;
    let key_id = body["id"].as_str().unwrap();

    let resp = server.delete(&format!("/dispatch/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {t2}"))
        .await;
    assert_eq!(resp.status_code(), 404);
}

#[tokio::test]
async fn test_api_key_auth_grants_access_to_protected_endpoint() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeyauth1@example.com", "pass-auth1").await;

    let body = create_dispatcher_api_key(&server, &token, "Claude desktop").await;
    let api_key = body["key"].as_str().unwrap();

    let resp = server.get("/dispatch/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .await;
    assert_eq!(resp.status_code(), 200);
}

#[tokio::test]
async fn test_api_key_auth_works_on_mcp_endpoint() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeymcp1@example.com", "pass-mcp1").await;

    let body = create_dispatcher_api_key(&server, &token, "Claude MCP").await;
    let api_key = body["key"].as_str().unwrap();

    let resp = server.post("/dispatch/mcp")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let result = resp.json::<serde_json::Value>();
    assert_eq!(result["result"]["protocolVersion"], "2024-11-05");
}

#[tokio::test]
async fn test_revoked_api_key_rejected() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeyrvk1@example.com", "pass-rvk1").await;

    let body = create_dispatcher_api_key(&server, &token, "to-revoke").await;
    let api_key = body["key"].as_str().unwrap().to_string();
    let key_id = body["id"].as_str().unwrap();

    server.delete(&format!("/dispatch/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;

    let resp = server.get("/dispatch/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_jwt_auth_still_works_after_api_key_feature() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeycompat1@example.com", "pass-compat1").await;

    let resp = server.get("/dispatch/api/v1/loads")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200);
}

#[tokio::test]
async fn test_api_key_create_requires_jwt_not_api_key() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeyself1@example.com", "pass-self1").await;

    let body = create_dispatcher_api_key(&server, &token, "first-key").await;
    let api_key = body["key"].as_str().unwrap();

    let resp = server.post("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .json(&serde_json::json!({ "label": "self-created" }))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_api_key_revoke_requires_jwt_not_api_key() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeyself2@example.com", "pass-self2").await;

    let body = create_dispatcher_api_key(&server, &token, "key-to-revoke").await;
    let api_key = body["key"].as_str().unwrap().to_string();
    let key_id = body["id"].as_str().unwrap();

    let resp = server.delete(&format!("/dispatch/api-keys/{key_id}"))
        .add_header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_api_key_20_key_cap() {
    let (server, _b, _d, _rx) = test_server().await;
    let token = dispatcher_login(&server, "apikeycap1@example.com", "pass-cap1").await;

    for i in 0..20 {
        let resp = server.post("/dispatch/api-keys")
            .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&serde_json::json!({ "label": format!("key-{i}") }))
            .await;
        assert_eq!(resp.status_code(), 201, "key {i} creation should succeed");
    }

    let resp = server.post("/dispatch/api-keys")
        .add_header(header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&serde_json::json!({ "label": "key-21" }))
        .await;
    assert_eq!(resp.status_code(), 429);
}
```

- [ ] **Step 3: Run all new integration tests**

```bash
cargo test -p ollie test_api_key 2>&1 | tail -25
```

Expected: all 12 new tests pass.

- [ ] **Step 4: Run the full test suite to check for regressions**

```bash
cargo test -p ollie 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add tests/integration_test.rs
git commit -m "test(api-keys): add integration tests for dispatcher API keys"
```

---

## Post-implementation checklist

- [ ] Run full test suite one final time: `cargo test -p ollie 2>&1 | tail -5`
- [ ] Run `cargo clippy -p ollie -- -D warnings 2>&1 | tail -10` and fix any warnings
- [ ] File three GitHub issues (from spec out-of-scope table): "Dispatcher API key management UI", "Allow never-expire dispatcher API keys", "Per-key scopes for dispatcher API keys"
- [ ] Open PR to main
