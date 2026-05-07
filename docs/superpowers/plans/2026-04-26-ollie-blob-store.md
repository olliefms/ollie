# Ollie Blob Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a RESTful, RAG-enabled blob store in Rust with async AI processing, content-addressed deduplication, and Docker deployment.

**Architecture:** Single Axum crate with focused modules — storage (sharded filesystem), db (LanceDB), ai (Ollama client), pipeline (async worker), and api (handlers + auth). All shared state lives in `Arc<AppState>` injected via Axum's `State` extractor. A tokio mpsc channel decouples upload from AI processing.

**Tech Stack:** Rust 1.78+, Axum 0.7, axum-extra 0.9, LanceDB 0.9, Ollama HTTP API, lopdf, tokio, async-channel, Arrow (arrow-array + arrow-schema), Docker (multi-stage build + docker-compose)

---

## File Map

```
Cargo.toml
.gitignore
.dockerignore
Dockerfile
docker-compose.yml
src/
  lib.rs
  main.rs
  config.rs
  error.rs
  models.rs
  api/
    mod.rs
    auth.rs
    blobs.rs
    blob.rs
  storage/
    mod.rs
    shard.rs
  db/
    mod.rs
    ops.rs
  ai/
    mod.rs
    extract.rs
    embed.rs
    summarize.rs
  pipeline/
    mod.rs
    worker.rs
    recovery.rs
tests/
  integration_test.rs
```

---

## Task 1: Project Scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `.gitignore`
- Create: `src/lib.rs`, `src/main.rs` (stubs)

- [ ] **Step 1: Initialize the project**

```bash
cargo init --name ollie /Users/jimp7508/src/ollie
```

- [ ] **Step 2: Replace `Cargo.toml` with full dependency set**

```toml
[package]
name = "ollie"
version = "1.0.0"
edition = "2021"

[lib]
name = "ollie"
path = "src/lib.rs"

[[bin]]
name = "ollie"
path = "src/main.rs"

[dependencies]
axum = { version = "0.7", features = ["multipart"] }
axum-extra = { version = "0.9", features = ["query"] }
tokio = { version = "1", features = ["full"] }
tower-http = { version = "0.5", features = ["trace"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
sha2 = "0.10"
hex = "0.4"
lancedb = "0.9"
arrow-array = "52"
arrow-schema = "52"
async-channel = "2"
reqwest = { version = "0.12", features = ["json"] }
mime_guess = "2"
lopdf = "0.32"
base64 = { version = "0.22", features = ["std"] }
chrono = { version = "0.4", features = ["serde"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
thiserror = "1"
dotenvy = "0.15"
futures = "0.3"
bytes = "1"
anyhow = "1"

[dev-dependencies]
axum-test = "15"
tempfile = "3"
```

> **Note:** LanceDB and arrow-array versions must be compatible — lancedb 0.9 bundles a specific arrow version. If there are version conflicts, check lancedb's own `Cargo.toml` on crates.io to find the correct arrow pin and update arrow-array/arrow-schema to match. Run `cargo build` early to flush out version mismatches.

- [ ] **Step 3: Create module directory structure**

```bash
mkdir -p /Users/jimp7508/src/ollie/src/{api,storage,db,ai,pipeline}
touch /Users/jimp7508/src/ollie/src/api/mod.rs \
      /Users/jimp7508/src/ollie/src/storage/mod.rs \
      /Users/jimp7508/src/ollie/src/db/mod.rs \
      /Users/jimp7508/src/ollie/src/ai/mod.rs \
      /Users/jimp7508/src/ollie/src/pipeline/mod.rs
touch /Users/jimp7508/src/ollie/src/{config,error,models}.rs \
      /Users/jimp7508/src/ollie/src/api/{auth,blobs,blob}.rs \
      /Users/jimp7508/src/ollie/src/storage/shard.rs \
      /Users/jimp7508/src/ollie/src/db/ops.rs \
      /Users/jimp7508/src/ollie/src/ai/{extract,embed,summarize}.rs \
      /Users/jimp7508/src/ollie/src/pipeline/{worker,recovery}.rs
mkdir -p /Users/jimp7508/src/ollie/tests
touch /Users/jimp7508/src/ollie/tests/integration_test.rs
```

- [ ] **Step 4: Write stub `src/lib.rs`**

```rust
// src/lib.rs
pub mod ai;
pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod models;
pub mod pipeline;
pub mod storage;

use ai::OllamaClient;
use config::Config;
use db::DbClient;
use std::sync::Arc;
use storage::BlobStore;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbClient>,
    pub store: Arc<BlobStore>,
    pub ai: Arc<OllamaClient>,
    pub pipeline_tx: async_channel::Sender<Uuid>,
    pub config: Arc<Config>,
}
```

- [ ] **Step 5: Write stub `src/main.rs`**

```rust
// src/main.rs
fn main() {
    println!("ollie starting");
}
```

- [ ] **Step 6: Verify the project compiles**

```bash
cargo build
```

Expected: compiles. Empty module files produce no errors in Rust.

- [ ] **Step 7: Create `.gitignore`**

```
/target
.env
data/
```

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml .gitignore src/ tests/
git commit -m "feat: scaffold ollie project"
```

---

## Task 2: Config

**Files:**
- Create: `src/config.rs`

- [ ] **Step 1: Write the failing test in `src/config.rs`**

```rust
// src/config.rs
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub admin_api_key: String,
    pub port: u16,
    pub blob_store_path: String,
    pub lancedb_path: String,
    pub ollama_base_url: String,
    pub ollama_embed_model: String,
    pub ollama_summary_model: String,
    pub ollama_vision_model: String,
    pub ollama_embed_dim: usize,
    pub pipeline_workers: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_env() {
        env::set_var("ADMIN_API_KEY", "test-key");
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.admin_api_key, "test-key");
        assert_eq!(cfg.port, 3000);
        assert_eq!(cfg.pipeline_workers, 1);
        assert_eq!(cfg.ollama_embed_model, "nomic-embed-text");
        assert_eq!(cfg.ollama_summary_model, "llama3.2");
        assert_eq!(cfg.ollama_vision_model, "llava");
        assert_eq!(cfg.ollama_embed_dim, 768);
        env::remove_var("ADMIN_API_KEY");
    }

    #[test]
    fn test_config_missing_api_key() {
        env::remove_var("ADMIN_API_KEY");
        assert!(Config::from_env().is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test config
```

Expected: FAIL — `Config::from_env` not found.

- [ ] **Step 3: Implement `Config::from_env` above the tests block**

```rust
impl Config {
    pub fn from_env() -> Result<Self, String> {
        let admin_api_key = env::var("ADMIN_API_KEY")
            .map_err(|_| "ADMIN_API_KEY is required")?;
        Ok(Self {
            admin_api_key,
            port: env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(3000),
            blob_store_path: env::var("BLOB_STORE_PATH")
                .unwrap_or_else(|_| "./data/blobs".into()),
            lancedb_path: env::var("LANCEDB_PATH")
                .unwrap_or_else(|_| "./data/lancedb".into()),
            ollama_base_url: env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".into()),
            ollama_embed_model: env::var("OLLAMA_EMBED_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".into()),
            ollama_summary_model: env::var("OLLAMA_SUMMARY_MODEL")
                .unwrap_or_else(|_| "llama3.2".into()),
            ollama_vision_model: env::var("OLLAMA_VISION_MODEL")
                .unwrap_or_else(|_| "llava".into()),
            ollama_embed_dim: env::var("OLLAMA_EMBED_DIM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(768),
            pipeline_workers: env::var("PIPELINE_WORKERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
        })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test config
```

Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: add Config from env vars with embed dim"
```

---

## Task 3: Models

**Files:**
- Create: `src/models.rs`

- [ ] **Step 1: Write models with serialization tests**

```rust
// src/models.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BlobStatus {
    Pending,
    Processing,
    Ready,
    Failed,
}

impl BlobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for BlobStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "processing" => Ok(Self::Processing),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobRecord {
    pub id: Uuid,
    pub owner_id: i64,
    pub checksum: String,
    pub name: String,
    pub mime_type: String,
    pub size: i64,
    pub status: BlobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBlobRequest {
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Returned by GET /api/v1/blobs — no embedding, optional score
#[derive(Debug, Clone, Serialize)]
pub struct BlobListItem {
    pub id: Uuid,
    pub owner_id: i64,
    pub name: String,
    pub mime_type: String,
    pub size: i64,
    pub status: BlobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl From<BlobRecord> for BlobListItem {
    fn from(r: BlobRecord) -> Self {
        Self {
            id: r.id, owner_id: r.owner_id, name: r.name,
            mime_type: r.mime_type, size: r.size, status: r.status,
            summary: r.summary, tags: r.tags, created_at: r.created_at,
            score: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BlobListResponse {
    pub total: usize,
    pub items: Vec<BlobListItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_roundtrip() {
        for s in ["pending", "processing", "ready", "failed"] {
            let status: BlobStatus = s.parse().unwrap();
            assert_eq!(status.as_str(), s);
        }
    }

    #[test]
    fn test_blob_record_embedding_skipped_in_json() {
        let record = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "abc".into(),
            name: "file.txt".into(), mime_type: "text/plain".into(), size: 100,
            status: BlobStatus::Ready, error: None, summary: Some("a summary".into()),
            tags: vec!["a".into()],
            embedding: Some(vec![0.1, 0.2, 0.3]),
            created_at: Utc::now(), updated_at: Utc::now(),
        };
        let json = serde_json::to_value(&record).unwrap();
        assert!(json.get("embedding").is_none(), "embedding must not appear in JSON output");
        assert!(json.get("error").is_none());
    }
}
```

> **Note:** `#[serde(skip)]` on `embedding` means it is never serialized or deserialized via serde. The raw vector is stored and retrieved via LanceDB directly, not via the JSON API.

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test models
```

Expected: FAIL — module not declared.

- [ ] **Step 3: Verify `src/lib.rs` already declares `pub mod models;`** (done in Task 1). Run tests again:

```bash
cargo test models
```

Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add src/models.rs
git commit -m "feat: add BlobRecord models and status enum"
```

---

## Task 4: Error Types

**Files:**
- Create: `src/error.rs`

- [ ] **Step 1: Write the error type with an HTTP response test**

```rust
// src/error.rs
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_of(e: AppError) -> StatusCode {
        e.into_response().status()
    }

    #[test]
    fn test_error_status_codes() {
        assert_eq!(status_of(AppError::NotFound), StatusCode::NOT_FOUND);
        assert_eq!(status_of(AppError::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(status_of(AppError::BadRequest("x".into())), StatusCode::BAD_REQUEST);
        assert_eq!(status_of(AppError::Conflict("x".into())), StatusCode::CONFLICT);
        assert_eq!(status_of(AppError::Internal("x".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test error
```

Expected: FAIL — `from_env` not called (compile error from config module since error is already declared in lib.rs).

- [ ] **Step 3: Run test to verify it passes (lib.rs already has `pub mod error;`)**

```bash
cargo test error
```

Expected: 1 passed.

- [ ] **Step 4: Commit**

```bash
git add src/error.rs
git commit -m "feat: add AppError with HTTP status mapping"
```

---

## Task 5: Storage — Shard Path

**Files:**
- Create: `src/storage/shard.rs`
- Modify: `src/storage/mod.rs`

- [ ] **Step 1: Write the failing test in `src/storage/shard.rs`**

```rust
// src/storage/shard.rs
use std::path::PathBuf;

pub fn shard_path(base: &str, checksum: &str) -> PathBuf {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_path_two_levels() {
        // SHA-256 of "hello" = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let checksum = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let path = shard_path("/data/blobs", checksum);
        assert_eq!(
            path,
            PathBuf::from("/data/blobs/2c/f2/2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
    }

    #[test]
    fn test_shard_path_different_checksum() {
        let checksum = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab";
        let path = shard_path("/store", checksum);
        assert_eq!(
            path,
            PathBuf::from("/store/ab/cd/abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab")
        );
    }
}
```

- [ ] **Step 2: Declare the submodule in `src/storage/mod.rs`**

```rust
// src/storage/mod.rs
pub mod shard;
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test storage::shard
```

Expected: FAIL — `todo!()` panic.

- [ ] **Step 4: Implement `shard_path`**

```rust
// src/storage/shard.rs — replace todo!():
pub fn shard_path(base: &str, checksum: &str) -> PathBuf {
    let level1 = &checksum[0..2];
    let level2 = &checksum[2..4];
    PathBuf::from(base).join(level1).join(level2).join(checksum)
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test storage::shard
```

Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add src/storage/
git commit -m "feat: add shard path derivation (2-level, 1-byte per level)"
```

---

## Task 6: Storage — BlobStore

**Files:**
- Modify: `src/storage/mod.rs`

- [ ] **Step 1: Write the failing tests — replace `src/storage/mod.rs`**

```rust
// src/storage/mod.rs
pub mod shard;

use crate::error::AppError;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;

pub struct BlobStore {
    base: String,
}

impl BlobStore {
    pub fn new(base: &str) -> Self {
        Self { base: base.to_string() }
    }

    pub fn path_for(&self, checksum: &str) -> PathBuf {
        shard::shard_path(&self.base, checksum)
    }

    pub async fn write(&self, data: &Bytes) -> Result<String, AppError> {
        todo!()
    }

    pub async fn read(&self, checksum: &str) -> Result<Bytes, AppError> {
        todo!()
    }

    pub async fn delete(&self, checksum: &str) -> Result<(), AppError> {
        todo!()
    }

    pub async fn exists(&self, checksum: &str) -> bool {
        todo!()
    }
}

pub fn compute_checksum(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn temp_store() -> (BlobStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = BlobStore::new(dir.path().to_str().unwrap());
        (store, dir)
    }

    #[tokio::test]
    async fn test_write_and_read_roundtrip() {
        let (store, _dir) = temp_store().await;
        let data = Bytes::from("hello world");
        let checksum = store.write(&data).await.unwrap();
        let read_back = store.read(&checksum).await.unwrap();
        assert_eq!(data, read_back);
    }

    #[tokio::test]
    async fn test_checksum_matches_sha256_of_hello() {
        let data = b"hello";
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(compute_checksum(data), expected);
    }

    #[tokio::test]
    async fn test_exists_false_before_write() {
        let (store, _dir) = temp_store().await;
        assert!(!store.exists("deadbeef00000000deadbeef00000000deadbeef00000000deadbeef00000000").await);
    }

    #[tokio::test]
    async fn test_delete_removes_file() {
        let (store, _dir) = temp_store().await;
        let data = Bytes::from("to delete");
        let checksum = store.write(&data).await.unwrap();
        assert!(store.exists(&checksum).await);
        store.delete(&checksum).await.unwrap();
        assert!(!store.exists(&checksum).await);
    }

    #[tokio::test]
    async fn test_read_missing_returns_not_found() {
        let (store, _dir) = temp_store().await;
        let result = store
            .read("0000000000000000000000000000000000000000000000000000000000000000")
            .await;
        assert!(matches!(result, Err(AppError::NotFound)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test storage
```

Expected: multiple FAILs — `todo!()` panics.

- [ ] **Step 3: Implement BlobStore methods — replace the `todo!()` bodies**

```rust
pub async fn write(&self, data: &Bytes) -> Result<String, AppError> {
    let checksum = compute_checksum(data);
    let path = self.path_for(&checksum);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(&path, data).await?;
    Ok(checksum)
}

pub async fn read(&self, checksum: &str) -> Result<Bytes, AppError> {
    let path = self.path_for(checksum);
    match fs::read(&path).await {
        Ok(data) => Ok(Bytes::from(data)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AppError::NotFound),
        Err(e) => Err(AppError::Internal(e.to_string())),
    }
}

pub async fn delete(&self, checksum: &str) -> Result<(), AppError> {
    let path = self.path_for(checksum);
    match fs::remove_file(&path).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::Internal(e.to_string())),
    }
}

pub async fn exists(&self, checksum: &str) -> bool {
    self.path_for(checksum).exists()
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test storage
```

Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add src/storage/mod.rs
git commit -m "feat: add BlobStore with SHA-256 sharded filesystem"
```

---

## Task 7: DB — Connection & Schema

**Files:**
- Create: `src/db/mod.rs`

The embedding column uses `FixedSizeList<Float32>` with dimension from config — required by LanceDB for building a vector index and for `nearest_to()` ANN search.

- [ ] **Step 1: Write the connection test in `src/db/mod.rs`**

```rust
// src/db/mod.rs
pub mod ops;

use crate::error::AppError;
use arrow_schema::{DataType, Field, Schema};
use lancedb::Table;
use std::sync::Arc;

pub struct DbClient {
    pub table: Table,
    pub embed_dim: usize,
}

impl DbClient {
    pub async fn new(path: &str, embed_dim: usize) -> Result<Self, AppError> {
        todo!()
    }
}

pub fn blob_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("checksum", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("mime_type", DataType::Utf8, false),
        Field::new("size", DataType::Int64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("error", DataType::Utf8, true),
        Field::new("summary", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embed_dim as i32,
            ),
            true,
        ),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_db_client_creates_table() {
        let dir = TempDir::new().unwrap();
        let client = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        let count = client.table.count_rows(None).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_schema_has_fixed_size_embedding() {
        let schema = blob_schema(768);
        let field = schema.field_with_name("embedding").unwrap();
        assert!(matches!(field.data_type(), DataType::FixedSizeList(_, 768)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test db
```

Expected: FAIL — `todo!()`.

- [ ] **Step 3: Implement `DbClient::new`**

```rust
// src/db/mod.rs — implement new():
use arrow_array::{
    FixedSizeListArray, Float32Array, Int64Array, RecordBatch,
    RecordBatchIterator, StringArray,
};

impl DbClient {
    pub async fn new(path: &str, embed_dim: usize) -> Result<Self, AppError> {
        let conn = lancedb::connect(path)
            .execute()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let table = match conn.open_table("blobs").execute().await {
            Ok(t) => t,
            Err(_) => {
                let schema = blob_schema(embed_dim);
                let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
                let batch = RecordBatch::try_new(
                    schema.clone(),
                    vec![
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(Int64Array::from(Vec::<i64>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(Int64Array::from(Vec::<i64>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(
                            FixedSizeListArray::from_iter_primitive::<
                                arrow_array::types::Float32Type,
                                _,
                                _,
                            >(nulls, embed_dim as i32),
                        ),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                    ],
                )
                .map_err(|e| AppError::Internal(e.to_string()))?;

                let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
                conn.create_table("blobs", Box::new(iter))
                    .execute()
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?
            }
        };

        Ok(Self { table, embed_dim })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test db
```

Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src/db/mod.rs
git commit -m "feat: add LanceDB connection with FixedSizeList embedding schema"
```

---

## Task 8: DB — Operations

**Files:**
- Create: `src/db/ops.rs`

> **Critical:** All `StringArray` construction from owned `String` values must bind intermediates to `let` first to avoid dangling `&str` references. Pattern: `let id_str = record.id.to_string(); StringArray::from(vec![id_str.as_str()])`.

- [ ] **Step 1: Write failing tests in `src/db/ops.rs`**

```rust
// src/db/ops.rs
use crate::{
    db::{blob_schema, DbClient},
    error::AppError,
    models::{BlobListItem, BlobRecord, BlobStatus},
};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int64Array, RecordBatch,
    RecordBatchIterator, StringArray,
};
use chrono::Utc;
use futures::TryStreamExt;
use std::sync::Arc;
use uuid::Uuid;

// Forward-declare methods tested below; implement in Step 3
impl DbClient {
    pub async fn insert(&self, record: &BlobRecord) -> Result<(), AppError> { todo!() }
    pub async fn get_by_id(&self, id: Uuid) -> Result<BlobRecord, AppError> { todo!() }
    pub async fn count_by_checksum(&self, checksum: &str) -> Result<usize, AppError> { todo!() }
    pub async fn get_one_by_checksum(&self, checksum: &str) -> Result<Option<BlobRecord>, AppError> { todo!() }
    pub async fn mark_processing(&self, id: Uuid) -> Result<(), AppError> { todo!() }
    pub async fn mark_ready(&self, id: Uuid, summary: Option<String>, embedding: Option<Vec<f32>>) -> Result<(), AppError> { todo!() }
    pub async fn mark_failed(&self, id: Uuid, error: String) -> Result<(), AppError> { todo!() }
    pub async fn update_metadata(&self, id: Uuid, name: Option<String>, tags: Option<Vec<String>>) -> Result<BlobRecord, AppError> { todo!() }
    pub async fn delete_by_id(&self, id: Uuid) -> Result<(), AppError> { todo!() }
    pub async fn list(&self, name_filter: Option<&str>, tag_filter: &[String], limit: usize, offset: usize) -> Result<(usize, Vec<BlobListItem>), AppError> { todo!() }
    pub async fn search(&self, embedding: Vec<f32>, name_filter: Option<&str>, tag_filter: &[String], limit: usize) -> Result<Vec<BlobListItem>, AppError> { todo!() }
    pub async fn list_non_ready_ids(&self) -> Result<Vec<Uuid>, AppError> { todo!() }
    pub async fn create_vector_index(&self) -> Result<(), AppError> { todo!() }
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

    fn sample_record() -> BlobRecord {
        let now = Utc::now();
        BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "abc123".into(),
            name: "test.txt".into(), mime_type: "text/plain".into(), size: 42,
            status: BlobStatus::Pending, error: None, summary: None,
            tags: vec!["tag1".into()], embedding: None,
            created_at: now, updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_by_id() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        let fetched = db.get_by_id(record.id).await.unwrap();
        assert_eq!(fetched.id, record.id);
        assert_eq!(fetched.name, "test.txt");
        assert_eq!(fetched.tags, vec!["tag1"]);
    }

    #[tokio::test]
    async fn test_get_by_id_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_by_id(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_count_by_checksum_dedup() {
        let (db, _dir) = test_db().await;
        let mut r1 = sample_record();
        let mut r2 = sample_record();
        r2.id = Uuid::new_v4();
        r1.checksum = "shared".into();
        r2.checksum = "shared".into();
        db.insert(&r1).await.unwrap();
        db.insert(&r2).await.unwrap();
        assert_eq!(db.count_by_checksum("shared").await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_mark_ready_sets_embedding_and_summary() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        db.mark_ready(record.id, Some("summary".into()), Some(vec![1.0, 2.0, 3.0, 4.0]))
            .await
            .unwrap();
        let fetched = db.get_by_id(record.id).await.unwrap();
        assert_eq!(fetched.status, BlobStatus::Ready);
        assert_eq!(fetched.summary.as_deref(), Some("summary"));
        assert!(fetched.embedding.is_some());
    }

    #[tokio::test]
    async fn test_mark_failed_stores_error() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        db.mark_failed(record.id, "ollama timeout".into()).await.unwrap();
        let fetched = db.get_by_id(record.id).await.unwrap();
        assert_eq!(fetched.status, BlobStatus::Failed);
        assert_eq!(fetched.error.as_deref(), Some("ollama timeout"));
    }

    #[tokio::test]
    async fn test_mark_processing_preserves_existing_fields() {
        let (db, _dir) = test_db().await;
        let mut record = sample_record();
        record.summary = Some("pre-existing".into());
        record.embedding = Some(vec![0.1, 0.2, 0.3, 0.4]);
        db.insert(&record).await.unwrap();
        db.mark_processing(record.id).await.unwrap();
        let fetched = db.get_by_id(record.id).await.unwrap();
        assert_eq!(fetched.status, BlobStatus::Processing);
        assert_eq!(fetched.summary.as_deref(), Some("pre-existing"),
            "mark_processing must not wipe existing summary");
    }

    #[tokio::test]
    async fn test_delete_by_id() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        db.delete_by_id(record.id).await.unwrap();
        assert!(matches!(db.get_by_id(record.id).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_update_metadata() {
        let (db, _dir) = test_db().await;
        let record = sample_record();
        db.insert(&record).await.unwrap();
        let updated = db.update_metadata(record.id, Some("new.txt".into()), Some(vec!["x".into()]))
            .await.unwrap();
        assert_eq!(updated.name, "new.txt");
        assert_eq!(updated.tags, vec!["x"]);
    }

    #[tokio::test]
    async fn test_list_non_ready_ids() {
        let (db, _dir) = test_db().await;
        let r1 = sample_record();
        let mut r2 = sample_record();
        r2.id = Uuid::new_v4();
        r2.status = BlobStatus::Ready;
        db.insert(&r1).await.unwrap();
        db.insert(&r2).await.unwrap();
        let ids = db.list_non_ready_ids().await.unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], r1.id);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test db::ops
```

Expected: multiple FAILs — `todo!()`.

- [ ] **Step 3: Implement helper functions and all ops methods**

Add these helpers at the bottom of `src/db/ops.rs` (below the tests block), then implement each method:

```rust
// --- Helpers ---

fn record_to_batch(record: &BlobRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = blob_schema(embed_dim);

    // Bind all owned strings before borrowing as &str
    let id_str = record.id.to_string();
    let tags_json = serde_json::to_string(&record.tags)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let created = record.created_at.to_rfc3339();
    let updated = record.updated_at.to_rfc3339();
    let status_str = record.status.as_str();

    let embedding_col: Arc<dyn arrow_array::Array> = match &record.embedding {
        Some(v) => {
            let floats: Vec<Option<f32>> = v.iter().map(|&f| Some(f)).collect();
            Arc::new(FixedSizeListArray::from_iter_primitive::<
                arrow_array::types::Float32Type,
                _,
                _,
            >(vec![Some(floats)], embed_dim as i32))
        }
        None => Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type,
            _,
            _,
        >(vec![None::<Vec<Option<f32>>>], embed_dim as i32)),
    };

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![id_str.as_str()])),
            Arc::new(Int64Array::from(vec![record.owner_id])),
            Arc::new(StringArray::from(vec![record.checksum.as_str()])),
            Arc::new(StringArray::from(vec![record.name.as_str()])),
            Arc::new(StringArray::from(vec![record.mime_type.as_str()])),
            Arc::new(Int64Array::from(vec![record.size])),
            Arc::new(StringArray::from(vec![status_str])),
            Arc::new(StringArray::from(vec![record.error.as_deref()])),
            Arc::new(StringArray::from(vec![record.summary.as_deref()])),
            Arc::new(StringArray::from(vec![tags_json.as_str()])),
            embedding_col,
            Arc::new(StringArray::from(vec![created.as_str()])),
            Arc::new(StringArray::from(vec![updated.as_str()])),
        ],
    )
    .map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_records(batches: Vec<RecordBatch>) -> Result<Vec<BlobRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_record(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_record(batch: &RecordBatch, i: usize) -> Result<BlobRecord, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string())
            .unwrap_or_default()
    };
    let opt_str_col = |name: &str| -> Option<String> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) })
    };
    let i64_col = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i))
            .unwrap_or(0)
    };

    let tags: Vec<String> = serde_json::from_str(&str_col("tags")).unwrap_or_default();
    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(BlobRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        owner_id: i64_col("owner_id"),
        checksum: str_col("checksum"),
        name: str_col("name"),
        mime_type: str_col("mime_type"),
        size: i64_col("size"),
        status: str_col("status").parse().map_err(|e: String| AppError::Internal(e))?,
        error: opt_str_col("error"),
        summary: opt_str_col("summary"),
        tags,
        embedding,
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_filter(name: Option<&str>, tags: &[String], extra: Option<&str>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(e) = extra { parts.push(e.to_string()); }
    if let Some(n) = name { parts.push(format!("name LIKE '%{n}%'")); }
    for tag in tags { parts.push(format!("tags LIKE '%\"{tag}\"%'")); }
    if parts.is_empty() { None } else { Some(parts.join(" AND ")) }
}

async fn collect_stream(
    stream: lancedb::query::QueryExecutionStream,
) -> Result<Vec<RecordBatch>, AppError> {
    use futures::TryStreamExt;
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}
```

Now implement each method in the `impl DbClient` block:

```rust
// src/db/ops.rs — replace todo!() in impl DbClient

pub async fn insert(&self, record: &BlobRecord) -> Result<(), AppError> {
    let batch = record_to_batch(record, self.embed_dim)?;
    let schema = blob_schema(self.embed_dim);
    let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
    self.table.add(Box::new(iter)).execute().await
        .map_err(|e| AppError::Internal(e.to_string()))
}

pub async fn get_by_id(&self, id: Uuid) -> Result<BlobRecord, AppError> {
    let id_str = id.to_string();
    let stream = self.table.query()
        .filter(format!("id = '{id_str}'"))
        .execute().await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let batches = collect_stream(stream).await?;
    batches_to_records(batches)?
        .into_iter().next()
        .ok_or(AppError::NotFound)
}

pub async fn count_by_checksum(&self, checksum: &str) -> Result<usize, AppError> {
    self.table.count_rows(Some(format!("checksum = '{checksum}'")))
        .await.map_err(|e| AppError::Internal(e.to_string()))
}

pub async fn get_one_by_checksum(&self, checksum: &str) -> Result<Option<BlobRecord>, AppError> {
    let stream = self.table.query()
        .filter(format!("checksum = '{checksum}'"))
        .limit(1)
        .execute().await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(batches_to_records(collect_stream(stream).await?)?.into_iter().next())
}

pub async fn mark_processing(&self, id: Uuid) -> Result<(), AppError> {
    // Only changes status — preserves all other fields including summary/embedding
    let mut record = self.get_by_id(id).await?;
    record.status = BlobStatus::Processing;
    record.updated_at = Utc::now();
    self.delete_by_id(id).await?;
    self.insert(&record).await
}

pub async fn mark_ready(&self, id: Uuid, summary: Option<String>, embedding: Option<Vec<f32>>) -> Result<(), AppError> {
    let mut record = self.get_by_id(id).await?;
    record.status = BlobStatus::Ready;
    record.summary = summary;
    record.embedding = embedding;
    record.error = None;
    record.updated_at = Utc::now();
    self.delete_by_id(id).await?;
    self.insert(&record).await
}

pub async fn mark_failed(&self, id: Uuid, error: String) -> Result<(), AppError> {
    let mut record = self.get_by_id(id).await?;
    record.status = BlobStatus::Failed;
    record.error = Some(error);
    record.updated_at = Utc::now();
    self.delete_by_id(id).await?;
    self.insert(&record).await
}

pub async fn update_metadata(&self, id: Uuid, name: Option<String>, tags: Option<Vec<String>>) -> Result<BlobRecord, AppError> {
    let mut record = self.get_by_id(id).await?;
    if let Some(n) = name { record.name = n; }
    if let Some(t) = tags { record.tags = t; }
    record.updated_at = Utc::now();
    self.delete_by_id(id).await?;
    self.insert(&record).await?;
    Ok(record)
}

pub async fn delete_by_id(&self, id: Uuid) -> Result<(), AppError> {
    let id_str = id.to_string();
    self.table.delete(&format!("id = '{id_str}'")).await
        .map_err(|e| AppError::Internal(e.to_string()))
}

pub async fn list(&self, name_filter: Option<&str>, tag_filter: &[String], limit: usize, offset: usize) -> Result<(usize, Vec<BlobListItem>), AppError> {
    let filter = build_filter(name_filter, tag_filter, None);
    let total = self.table.count_rows(filter.clone()).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut q = self.table.query().limit(limit + offset);
    if let Some(f) = filter { q = q.filter(f); }
    let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
    let items: Vec<BlobListItem> = batches_to_records(collect_stream(stream).await?)?
        .into_iter()
        .skip(offset)
        .map(BlobListItem::from)
        .collect();
    Ok((total, items))
}

pub async fn search(&self, embedding: Vec<f32>, name_filter: Option<&str>, tag_filter: &[String], limit: usize) -> Result<Vec<BlobListItem>, AppError> {
    let filter = build_filter(name_filter, tag_filter, Some("status = 'ready'"));
    let mut q = self.table.query()
        .nearest_to(embedding)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .limit(limit);
    if let Some(f) = filter { q = q.filter(f); }
    let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
    let batches = collect_stream(stream).await?;
    let mut items = Vec::new();
    for batch in &batches {
        let distance_col = batch.column_by_name("_distance")
            .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
            .map(|a| (0..a.len()).map(|i| a.value(i)).collect::<Vec<f32>>());
        for (i, record) in batches_to_records(vec![batch.clone()])?.into_iter().enumerate() {
            let mut item = BlobListItem::from(record);
            if let Some(ref d) = distance_col {
                item.score = Some(1.0 / (1.0 + d[i]));
            }
            items.push(item);
        }
    }
    Ok(items)
}

pub async fn list_non_ready_ids(&self) -> Result<Vec<Uuid>, AppError> {
    let stream = self.table.query()
        .filter("status = 'pending' OR status = 'processing'")
        .execute().await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(batches_to_records(collect_stream(stream).await?)?
        .into_iter().map(|r| r.id).collect())
}

pub async fn create_vector_index(&self) -> Result<(), AppError> {
    self.table
        .create_index(&["embedding"], lancedb::index::Index::IvfPq(Default::default()))
        .execute()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test db
```

Expected: all db tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/db/ops.rs src/db/mod.rs
git commit -m "feat: add LanceDB CRUD, search, and vector index operations"
```

---

## Task 9: AI — Ollama Client

**Files:**
- Create: `src/ai/mod.rs`

- [ ] **Step 1: Write the client struct and unit tests**

```rust
// src/ai/mod.rs
pub mod embed;
pub mod extract;
pub mod summarize;

use crate::error::AppError;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct OllamaClient {
    pub base_url: String,
    pub embed_model: String,
    pub summary_model: String,
    pub vision_model: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    images: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

impl OllamaClient {
    pub fn new(base_url: &str, embed_model: &str, summary_model: &str, vision_model: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            embed_model: embed_model.to_string(),
            summary_model: summary_model.to_string(),
            vision_model: vision_model.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, AppError> {
        let resp: EmbedResponse = self.client
            .post(format!("{}/api/embeddings", self.base_url))
            .json(&EmbedRequest { model: &self.embed_model, prompt: text })
            .send().await
            .map_err(|e| AppError::Internal(format!("ollama embed: {e}")))?
            .error_for_status()
            .map_err(|e| AppError::Internal(format!("ollama embed status: {e}")))?
            .json().await
            .map_err(|e| AppError::Internal(format!("ollama embed parse: {e}")))?;
        Ok(resp.embedding)
    }

    pub async fn generate(&self, model: &str, prompt: &str, image_b64: Option<String>) -> Result<String, AppError> {
        let resp: GenerateResponse = self.client
            .post(format!("{}/api/generate", self.base_url))
            .json(&GenerateRequest {
                model, prompt, stream: false,
                images: image_b64.map(|b| vec![b]),
            })
            .send().await
            .map_err(|e| AppError::Internal(format!("ollama generate: {e}")))?
            .error_for_status()
            .map_err(|e| AppError::Internal(format!("ollama generate status: {e}")))?
            .json().await
            .map_err(|e| AppError::Internal(format!("ollama generate parse: {e}")))?;
        Ok(resp.response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_constructs() {
        let c = OllamaClient::new("http://localhost:11434", "nomic-embed-text", "llama3.2", "llava");
        assert_eq!(c.embed_model, "nomic-embed-text");
    }

    #[test]
    fn test_base_url_strips_trailing_slash() {
        let c = OllamaClient::new("http://localhost:11434/", "a", "b", "c");
        assert_eq!(c.base_url, "http://localhost:11434");
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

```bash
cargo test ai
```

Expected: 2 passed (no network calls in these tests).

- [ ] **Step 3: Commit**

```bash
git add src/ai/mod.rs
git commit -m "feat: add Ollama HTTP client"
```

---

## Task 10: AI — Content Extraction

**Files:**
- Create: `src/ai/extract.rs`

> **Note on PDF vision fallback:** lopdf extracts text from native PDFs. Scanned (image-only) PDFs cannot be rasterized without a native library (pdfium, poppler). When lopdf returns fewer than 50 words, this implementation passes the raw PDF bytes to the vision model. Some vision models (llava 1.6+) can process PDF bytes directly; others may not. This is acceptable for v1.0.0. Rasterization support can be added in a future minor version.

- [ ] **Step 1: Write the failing tests**

```rust
// src/ai/extract.rs
use bytes::Bytes;

pub enum Extractable {
    Text(String),
    /// Raw bytes to send to the vision model (image or sparse PDF)
    ImageBytes(Bytes),
    Unsupported,
}

pub fn extract_content(data: &Bytes, mime_type: &str) -> Extractable {
    todo!()
}

pub fn bytes_to_base64(data: &Bytes) -> String {
    todo!()
}

fn extract_pdf_text(data: &[u8]) -> String {
    todo!()
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_plain_text() {
        let data = Bytes::from("hello world this is text");
        assert!(matches!(extract_content(&data, "text/plain"), Extractable::Text(t) if t == "hello world this is text"));
    }

    #[test]
    fn test_extract_json() {
        let data = Bytes::from(r#"{"key": "value"}"#);
        assert!(matches!(extract_content(&data, "application/json"), Extractable::Text(_)));
    }

    #[test]
    fn test_extract_image_returns_bytes() {
        let data = Bytes::from(vec![0xFF, 0xD8, 0xFF]);
        assert!(matches!(extract_content(&data, "image/jpeg"), Extractable::ImageBytes(_)));
    }

    #[test]
    fn test_extract_binary_returns_unsupported() {
        let data = Bytes::from(vec![0x00, 0x01, 0x02]);
        assert!(matches!(extract_content(&data, "application/octet-stream"), Extractable::Unsupported));
    }

    #[test]
    fn test_word_count() {
        assert_eq!(word_count("hello world foo"), 3);
        assert_eq!(word_count(""), 0);
    }

    #[test]
    fn test_bytes_to_base64_roundtrips() {
        use base64::{engine::general_purpose, Engine as _};
        let data = Bytes::from("test data");
        let b64 = bytes_to_base64(&data);
        let decoded = general_purpose::STANDARD.decode(&b64).unwrap();
        assert_eq!(decoded, b"test data");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test ai::extract
```

Expected: FAIL — `todo!()`.

- [ ] **Step 3: Implement**

```rust
// src/ai/extract.rs — replace todo!() implementations:

use base64::{engine::general_purpose, Engine as _};

pub fn extract_content(data: &Bytes, mime_type: &str) -> Extractable {
    if mime_type.starts_with("text/")
        || mime_type == "application/json"
        || mime_type == "application/xml"
        || mime_type.contains("javascript")
    {
        return Extractable::Text(String::from_utf8_lossy(data).into_owned());
    }

    if mime_type == "application/pdf" {
        let text = extract_pdf_text(data);
        if word_count(&text) >= 50 {
            return Extractable::Text(text);
        }
        return Extractable::ImageBytes(data.clone());
    }

    if mime_type.starts_with("image/") {
        return Extractable::ImageBytes(data.clone());
    }

    Extractable::Unsupported
}

pub fn bytes_to_base64(data: &Bytes) -> String {
    general_purpose::STANDARD.encode(data)
}

fn extract_pdf_text(data: &[u8]) -> String {
    let Ok(doc) = lopdf::Document::load_mem(data) else {
        return String::new();
    };
    let page_nums: Vec<u32> = doc.get_pages().keys().copied().collect();
    let mut text = String::new();
    for page_num in page_nums {
        if let Ok(page_text) = doc.extract_text(&[page_num]) {
            text.push_str(&page_text);
            text.push('\n');
        }
    }
    text
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test ai::extract
```

Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add src/ai/extract.rs
git commit -m "feat: add content extraction for text, PDF, and image types"
```

---

## Task 11: AI — Embed & Summarize

**Files:**
- Create: `src/ai/embed.rs`
- Create: `src/ai/summarize.rs`

- [ ] **Step 1: Write `src/ai/embed.rs`**

```rust
// src/ai/embed.rs
use crate::{ai::OllamaClient, error::AppError};

pub async fn embed_text(client: &OllamaClient, text: &str) -> Result<Vec<f32>, AppError> {
    if text.trim().is_empty() {
        return Err(AppError::Internal("cannot embed empty text".into()));
    }
    // Truncate to ~8000 chars to stay within model context limits
    let truncated = if text.len() > 8000 { &text[..8000] } else { text };
    client.embed(truncated).await
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore] // requires live Ollama: cargo test ai::embed -- --ignored
    async fn test_embed_returns_non_empty_vector() {
        use super::*;
        use crate::ai::OllamaClient;
        let client = OllamaClient::new("http://localhost:11434", "nomic-embed-text", "llama3.2", "llava");
        let vec = embed_text(&client, "the quick brown fox").await.unwrap();
        assert!(!vec.is_empty());
    }
}
```

- [ ] **Step 2: Write `src/ai/summarize.rs`**

```rust
// src/ai/summarize.rs
use crate::{
    ai::{extract::bytes_to_base64, OllamaClient},
    error::AppError,
};
use bytes::Bytes;

pub async fn summarize_text(client: &OllamaClient, text: &str) -> Result<String, AppError> {
    let truncated = if text.len() > 4000 { &text[..4000] } else { text };
    let prompt = format!(
        "Provide a concise 1-2 sentence summary of the following content. \
        Respond with only the summary, no preamble:\n\n{truncated}"
    );
    client.generate(&client.summary_model.clone(), &prompt, None).await
}

pub async fn describe_image(client: &OllamaClient, data: &Bytes) -> Result<String, AppError> {
    let b64 = bytes_to_base64(data);
    let prompt = "Describe the content of this image in 1-2 concise sentences. \
                  If this is a document or contains text, summarize what it says. \
                  Respond with only the description, no preamble.";
    client.generate(&client.vision_model.clone(), prompt, Some(b64)).await
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore] // requires live Ollama: cargo test ai::summarize -- --ignored
    async fn test_summarize_text_returns_non_empty() {
        use super::*;
        use crate::ai::OllamaClient;
        let client = OllamaClient::new("http://localhost:11434", "nomic-embed-text", "llama3.2", "llava");
        let summary = summarize_text(&client, "Rust is a systems programming language focused on safety.").await.unwrap();
        assert!(!summary.is_empty());
    }
}
```

- [ ] **Step 3: Run the non-ignored tests**

```bash
cargo test ai
```

Expected: all pass (ignored tests skipped).

- [ ] **Step 4: Commit**

```bash
git add src/ai/embed.rs src/ai/summarize.rs
git commit -m "feat: add embed and summarize wrappers for Ollama"
```

---

## Task 12: Pipeline — Worker

**Files:**
- Create: `src/pipeline/worker.rs`

The worker transitions status carefully: `mark_processing` only changes status, preserving any pre-existing fields. On any Ollama error the record is marked `Failed` with the error message stored.

- [ ] **Step 1: Write the worker**

```rust
// src/pipeline/worker.rs
use crate::{
    ai::{
        embed::embed_text,
        extract::{extract_content, Extractable},
        summarize::{describe_image, summarize_text},
        OllamaClient,
    },
    db::DbClient,
    error::AppError,
    models::BlobStatus,
    storage::BlobStore,
};
use std::sync::Arc;
use uuid::Uuid;

pub async fn process_blob(
    id: Uuid,
    db: &DbClient,
    store: &BlobStore,
    ai: &OllamaClient,
) -> Result<(), AppError> {
    db.mark_processing(id).await?;

    let record = db.get_by_id(id).await?;
    let data = store.read(&record.checksum).await?;
    let extractable = extract_content(&data, &record.mime_type);

    let result: Result<(Option<String>, Option<Vec<f32>>), AppError> = async {
        match extractable {
            Extractable::Text(text) => {
                let summary = summarize_text(ai, &text).await?;
                let embed_source = if summary.is_empty() { &text } else { &summary };
                let embedding = embed_text(ai, embed_source).await?;
                Ok((Some(summary), Some(embedding)))
            }
            Extractable::ImageBytes(bytes) => {
                let description = describe_image(ai, &bytes).await?;
                let embedding = embed_text(ai, &description).await?;
                Ok((Some(description), Some(embedding)))
            }
            Extractable::Unsupported => Ok((None, None)),
        }
    }.await;

    match result {
        Ok((summary, embedding)) => {
            db.mark_ready(id, summary, embedding).await?;
            tracing::info!("pipeline completed for {id}");
        }
        Err(e) => {
            tracing::error!("pipeline failed for {id}: {e}");
            db.mark_failed(id, e.to_string()).await?;
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build
```

Expected: clean compile.

- [ ] **Step 3: Commit**

```bash
git add src/pipeline/worker.rs
git commit -m "feat: add pipeline worker with Failed status on error"
```

---

## Task 13: Pipeline — Recovery & Spawn

**Files:**
- Create: `src/pipeline/recovery.rs`
- Modify: `src/pipeline/mod.rs`

Workers use `async-channel` — a multi-producer, multi-consumer channel — so multiple workers can drain the queue concurrently without serializing on a mutex.

- [ ] **Step 1: Write `src/pipeline/recovery.rs` with a test**

```rust
// src/pipeline/recovery.rs
use crate::{db::DbClient, error::AppError};
use uuid::Uuid;

pub async fn requeue_stale(
    db: &DbClient,
    tx: &async_channel::Sender<Uuid>,
) -> Result<usize, AppError> {
    let ids = db.list_non_ready_ids().await?;
    let count = ids.len();
    for id in ids {
        tx.send(id).await.map_err(|e| AppError::Internal(e.to_string()))?;
    }
    tracing::info!("requeued {count} stale blobs on startup");
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::DbClient,
        models::{BlobRecord, BlobStatus},
    };
    use chrono::Utc;
    use tempfile::TempDir;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_requeue_sends_pending_ids_only() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        let now = Utc::now();

        let pending = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "c1".into(),
            name: "f.txt".into(), mime_type: "text/plain".into(), size: 1,
            status: BlobStatus::Pending, error: None, summary: None,
            tags: vec![], embedding: None, created_at: now, updated_at: now,
        };
        let ready = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum: "c2".into(),
            name: "g.txt".into(), mime_type: "text/plain".into(), size: 1,
            status: BlobStatus::Ready, error: None, summary: None,
            tags: vec![], embedding: None, created_at: now, updated_at: now,
        };
        db.insert(&pending).await.unwrap();
        db.insert(&ready).await.unwrap();

        let (tx, rx) = async_channel::bounded(10);
        let count = requeue_stale(&db, &tx).await.unwrap();
        assert_eq!(count, 1);
        let received = rx.recv().await.unwrap();
        assert_eq!(received, pending.id);
    }
}
```

- [ ] **Step 2: Write `src/pipeline/mod.rs`**

```rust
// src/pipeline/mod.rs
pub mod recovery;
pub mod worker;

use crate::{ai::OllamaClient, db::DbClient, storage::BlobStore};
use std::sync::Arc;
use uuid::Uuid;

pub fn spawn_pipeline(
    workers: usize,
    db: Arc<DbClient>,
    store: Arc<BlobStore>,
    ai: Arc<OllamaClient>,
) -> async_channel::Sender<Uuid> {
    let workers = workers.max(1);
    let (tx, rx) = async_channel::bounded::<Uuid>(256);

    for i in 0..workers {
        let rx = rx.clone();
        let db = db.clone();
        let store = store.clone();
        let ai = ai.clone();
        tokio::spawn(async move {
            tracing::info!("pipeline worker {i} started");
            while let Ok(id) = rx.recv().await {
                if let Err(e) = worker::process_blob(id, &db, &store, &ai).await {
                    tracing::error!("worker {i} error for {id}: {e}");
                }
            }
            tracing::info!("pipeline worker {i} stopped");
        });
    }
    tx
}
```

- [ ] **Step 3: Run tests to verify they pass**

```bash
cargo test pipeline
```

Expected: 1 passed.

- [ ] **Step 4: Commit**

```bash
git add src/pipeline/
git commit -m "feat: add pipeline recovery and async-channel worker spawning"
```

---

## Task 14: Auth Middleware

**Files:**
- Modify: `src/api/auth.rs`
- Modify: `src/api/mod.rs`

- [ ] **Step 1: Write the auth middleware with tests**

```rust
// src/api/auth.rs
use crate::error::AppError;
use axum::{extract::Request, http::header::AUTHORIZATION, middleware::Next, response::Response};

pub async fn require_bearer(
    expected_key: String,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) if t == expected_key => Ok(next.run(request).await),
        _ => Err(AppError::Unauthorized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, middleware::from_fn, routing::get, Router};
    use axum_test::TestServer;

    fn test_app(key: &'static str) -> Router {
        let key = key.to_string();
        Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(from_fn(move |req, next| {
                let k = key.clone();
                async move { require_bearer(k, req, next).await }
            }))
    }

    #[tokio::test]
    async fn test_valid_bearer_passes() {
        let server = TestServer::new(test_app("secret")).unwrap();
        let resp = server.get("/test")
            .add_header(axum::http::header::AUTHORIZATION, "Bearer secret")
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_missing_bearer_returns_401() {
        let server = TestServer::new(test_app("secret")).unwrap();
        let resp = server.get("/test").await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_wrong_key_returns_401() {
        let server = TestServer::new(test_app("secret")).unwrap();
        let resp = server.get("/test")
            .add_header(axum::http::header::AUTHORIZATION, "Bearer wrong")
            .await;
        assert_eq!(resp.status_code(), StatusCode::UNAUTHORIZED);
    }
}
```

- [ ] **Step 2: Stub `src/api/mod.rs`**

```rust
// src/api/mod.rs
pub mod auth;
pub mod blob;
pub mod blobs;
```

- [ ] **Step 3: Run tests**

```bash
cargo test api::auth
```

Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add src/api/
git commit -m "feat: add bearer token auth middleware"
```

---

## Task 15: Handler — POST /api/v1/blobs

**Files:**
- Modify: `src/api/blobs.rs`

- [ ] **Step 1: Write the upload handler**

```rust
// src/api/blobs.rs
use crate::{
    error::AppError,
    models::{BlobListResponse, BlobRecord, BlobStatus},
    AppState,
};
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use axum_extra::extract::Query;
use bytes::Bytes;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub s: Option<String>,
    pub name: Option<String>,
    // axum_extra::Query handles repeated ?tag=a&tag=b correctly
    #[serde(default)]
    pub tag: Vec<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn upload_blob(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    let mut file_bytes: Option<Bytes> = None;
    let mut filename: Option<String> = None;
    let mut display_name: Option<String> = None;
    let mut tags: Vec<String> = vec![];

    while let Some(field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                file_bytes = Some(field.bytes().await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?);
            }
            "name" => {
                display_name = Some(field.text().await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?);
            }
            "tags" => {
                let raw = field.text().await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
                tags = serde_json::from_str(&raw)
                    .map_err(|_| AppError::BadRequest("tags must be a JSON array of strings".into()))?;
            }
            _ => {}
        }
    }

    let data = file_bytes.ok_or_else(|| AppError::BadRequest("missing 'file' field".into()))?;
    let name = display_name.or(filename).unwrap_or_else(|| "unnamed".to_string());
    let mime_type = mime_guess::from_path(&name).first_or_octet_stream().to_string();
    let checksum = crate::storage::compute_checksum(&data);
    let now = Utc::now();

    let (status_code, record) = if state.store.exists(&checksum).await {
        let existing = state.db.get_one_by_checksum(&checksum).await?;
        let (summary, embedding, status) = match existing {
            Some(ref r) => (r.summary.clone(), r.embedding.clone(), BlobStatus::Ready),
            None => (None, None, BlobStatus::Pending),
        };
        let record = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum, name, mime_type,
            size: data.len() as i64, status, error: None, summary, tags,
            embedding, created_at: now, updated_at: now,
        };
        state.db.insert(&record).await?;
        if matches!(record.status, BlobStatus::Pending) {
            state.pipeline_tx.send(record.id).await
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }
        (StatusCode::CREATED, record)
    } else {
        state.store.write(&data).await?;
        let record = BlobRecord {
            id: Uuid::new_v4(), owner_id: 0, checksum, name, mime_type,
            size: data.len() as i64, status: BlobStatus::Pending, error: None,
            summary: None, tags, embedding: None, created_at: now, updated_at: now,
        };
        state.db.insert(&record).await?;
        state.pipeline_tx.send(record.id).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        (StatusCode::ACCEPTED, record)
    };

    Ok((status_code, Json(record)))
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo build
```

- [ ] **Step 3: Commit**

```bash
git add src/api/blobs.rs
git commit -m "feat: add POST /api/v1/blobs upload handler with dedup"
```

---

## Task 16: Handler — GET /api/v1/blobs

**Files:**
- Modify: `src/api/blobs.rs`

- [ ] **Step 1: Add the list/search handler**

```rust
// src/api/blobs.rs — add below upload_blob:

pub async fn list_blobs(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0);

    if let Some(query_text) = q.s {
        let embedding = crate::ai::embed::embed_text(&state.ai, &query_text).await?;
        let items = state.db.search(embedding, q.name.as_deref(), &q.tag, limit).await?;
        let total = items.len();
        return Ok(Json(BlobListResponse { total, items }));
    }

    let (total, items) = state.db.list(q.name.as_deref(), &q.tag, limit, offset).await?;
    Ok(Json(BlobListResponse { total, items }))
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo build
```

- [ ] **Step 3: Commit**

```bash
git add src/api/blobs.rs
git commit -m "feat: add GET /api/v1/blobs list and semantic search handler"
```

---

## Task 17: Handler — GET, PUT, DELETE /api/v1/blob/:id

**Files:**
- Modify: `src/api/blob.rs`

- [ ] **Step 1: Write all three handlers**

```rust
// src/api/blob.rs
use crate::{error::AppError, models::UpdateBlobRequest, AppState};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use uuid::Uuid;

pub async fn get_blob(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_by_id(id).await?;

    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("application/json"))
        .unwrap_or(false);

    if wants_json {
        return Ok(Json(record).into_response());
    }

    // File is always on disk once the blob record exists
    let data = state.store.read(&record.checksum).await?;
    let disposition = format!("attachment; filename=\"{}\"", record.name);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, record.mime_type),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from(data),
    )
        .into_response())
}

pub async fn update_blob(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateBlobRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.name.is_none() && body.tags.is_none() {
        return Err(AppError::BadRequest(
            "at least one of 'name' or 'tags' is required".into(),
        ));
    }
    let updated = state.db.update_metadata(id, body.name, body.tags).await?;
    Ok(Json(updated))
}

pub async fn delete_blob(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = state.db.get_by_id(id).await?;
    let ref_count = state.db.count_by_checksum(&record.checksum).await?;
    if ref_count <= 1 {
        state.store.delete(&record.checksum).await?;
    }
    state.db.delete_by_id(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo build
```

- [ ] **Step 3: Commit**

```bash
git add src/api/blob.rs
git commit -m "feat: add GET/PUT/DELETE /api/v1/blob/:id handlers"
```

---

## Task 18: Router & `main.rs`

**Files:**
- Modify: `src/api/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write the router in `src/api/mod.rs`**

```rust
// src/api/mod.rs
pub mod auth;
pub mod blob;
pub mod blobs;

use crate::{api::auth::require_bearer, AppState};
use axum::{
    middleware::from_fn,
    routing::{delete, get, post, put},
    Router,
};

pub fn router(state: AppState) -> Router {
    let key = state.config.admin_api_key.clone();
    Router::new()
        .route("/api/v1/blobs", post(blobs::upload_blob))
        .route("/api/v1/blobs", get(blobs::list_blobs))
        .route("/api/v1/blob/:id", get(blob::get_blob))
        .route("/api/v1/blob/:id", put(blob::update_blob))
        .route("/api/v1/blob/:id", delete(blob::delete_blob))
        .layer(from_fn(move |req, next| {
            let k = key.clone();
            async move { require_bearer(k, req, next).await }
        }))
        .with_state(state)
}
```

- [ ] **Step 2: Write `src/main.rs`**

```rust
// src/main.rs
use ollie::{
    ai::OllamaClient, api, AppState, config::Config, db::DbClient,
    pipeline::{recovery::requeue_stale, spawn_pipeline}, storage::BlobStore,
};
use std::{net::SocketAddr, sync::Arc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ollie=info".into()),
        )
        .init();

    let config = Arc::new(Config::from_env().map_err(|e| anyhow::anyhow!(e))?);

    let db = Arc::new(DbClient::new(&config.lancedb_path, config.ollama_embed_dim).await?);
    let store = Arc::new(BlobStore::new(&config.blob_store_path));
    let ai = Arc::new(OllamaClient::new(
        &config.ollama_base_url,
        &config.ollama_embed_model,
        &config.ollama_summary_model,
        &config.ollama_vision_model,
    ));

    let pipeline_tx = spawn_pipeline(config.pipeline_workers, db.clone(), store.clone(), ai.clone());
    requeue_stale(&db, &pipeline_tx).await?;

    // Build vector index (no-op if table is empty or index already exists)
    if let Err(e) = db.create_vector_index().await {
        tracing::warn!("vector index not created: {e} (needs data first)");
    }

    let state = AppState { db, store, ai, pipeline_tx, config: config.clone() };
    let app = api::router(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 3: Run full build and all tests**

```bash
cargo build && cargo test
```

Expected: clean compile, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs src/api/mod.rs
git commit -m "feat: wire AppState, router, and main server"
```

---

## Task 19: Docker

**Files:**
- Create: `Dockerfile`
- Create: `docker-compose.yml`
- Create: `.dockerignore`

- [ ] **Step 1: Write the multi-stage `Dockerfile`**

```dockerfile
# Dockerfile
FROM rust:1.78-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
# Warm the dependency cache
RUN mkdir src && echo 'fn main(){}' > src/main.rs && \
    echo 'pub fn lib(){}' > src/lib.rs && \
    cargo build --release && rm -rf src

COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/ollie /usr/local/bin/ollie

RUN useradd -m -u 1001 ollie
USER ollie

EXPOSE 3000
CMD ["ollie"]
```

- [ ] **Step 2: Write `docker-compose.yml`**

```yaml
# docker-compose.yml
services:
  blob-store:
    build: .
    ports:
      - "3000:3000"
    environment:
      ADMIN_API_KEY: ${ADMIN_API_KEY:?ADMIN_API_KEY is required}
      OLLAMA_BASE_URL: http://ollama:11434
      BLOB_STORE_PATH: /data/blobs
      LANCEDB_PATH: /data/lancedb
      OLLAMA_EMBED_MODEL: ${OLLAMA_EMBED_MODEL:-nomic-embed-text}
      OLLAMA_SUMMARY_MODEL: ${OLLAMA_SUMMARY_MODEL:-llama3.2}
      OLLAMA_VISION_MODEL: ${OLLAMA_VISION_MODEL:-llava}
      OLLAMA_EMBED_DIM: ${OLLAMA_EMBED_DIM:-768}
      PIPELINE_WORKERS: ${PIPELINE_WORKERS:-1}
      RUST_LOG: ollie=info
    volumes:
      - ./data:/data
    depends_on:
      - ollama
    restart: unless-stopped

  ollama:
    image: ollama/ollama:latest
    volumes:
      - ollama-models:/root/.ollama
    ports:
      - "11434:11434"
    restart: unless-stopped

volumes:
  ollama-models:
```

- [ ] **Step 3: Write `.dockerignore`**

```
target/
.git/
data/
.env
*.md
docs/
.superpowers/
```

- [ ] **Step 4: Verify Docker build**

```bash
docker build -t ollie:latest .
```

Expected: builds successfully.

- [ ] **Step 5: Commit**

```bash
git add Dockerfile docker-compose.yml .dockerignore
git commit -m "feat: add multi-stage Dockerfile and docker-compose with Ollama"
```

---

## Task 20: Integration Smoke Tests

**Files:**
- Modify: `tests/integration_test.rs`

- [ ] **Step 1: Write integration tests**

```rust
// tests/integration_test.rs
use axum::http::header;
use axum_test::TestServer;
use ollie::{
    ai::OllamaClient, api, AppState, config::Config, db::DbClient,
    storage::BlobStore,
};
use std::sync::Arc;
use tempfile::TempDir;

async fn test_server() -> (TestServer, TempDir, TempDir) {
    let blob_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    std::env::set_var("ADMIN_API_KEY", "test-secret");

    let config = Arc::new(Config::from_env().unwrap());
    let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
    let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
    let ai = Arc::new(OllamaClient::new(
        "http://localhost:11434", "nomic-embed-text", "llama3.2", "llava",
    ));
    // Channel intentionally not drained — no Ollama in these tests
    let (tx, _rx) = async_channel::bounded(100);

    let state = AppState { db, store, ai, pipeline_tx: tx, config };
    let server = TestServer::new(api::router(state)).unwrap();
    (server, blob_dir, db_dir)
}

fn auth() -> (&'static str, &'static str) {
    ("Authorization", "Bearer test-secret")
}

#[tokio::test]
async fn test_upload_returns_202_with_uuid() {
    let (server, _b, _d) = test_server().await;
    let resp = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"hello world".to_vec())
                .file_name("hello.txt").mime_type("text/plain")))
        .await;
    assert_eq!(resp.status_code(), 202);
    let body: serde_json::Value = resp.json();
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["status"], "pending");
    assert_eq!(body["name"], "hello.txt");
}

#[tokio::test]
async fn test_upload_without_auth_returns_401() {
    let (server, _b, _d) = test_server().await;
    let resp = server.post("/api/v1/blobs").await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn test_get_metadata_after_upload() {
    let (server, _b, _d) = test_server().await;
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"content".to_vec())
                .file_name("doc.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let meta = server.get(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(meta.status_code(), 200);
    assert_eq!(meta.json::<serde_json::Value>()["id"], id);
}

#[tokio::test]
async fn test_get_raw_bytes_regardless_of_status() {
    let (server, _b, _d) = test_server().await;
    let content = b"raw content bytes";
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("raw.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let raw = server.get(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(raw.status_code(), 200);
    // File is available even though status is still "pending" (no Ollama in test)
    assert_eq!(raw.bytes(), content.as_ref());
}

#[tokio::test]
async fn test_list_blobs() {
    let (server, _b, _d) = test_server().await;
    server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"file1".to_vec())
                .file_name("a.txt").mime_type("text/plain")))
        .await;

    let list = server.get("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(list.status_code(), 200);
    assert!(list.json::<serde_json::Value>()["total"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn test_dedup_second_upload_returns_201() {
    let (server, _b, _d) = test_server().await;
    let content = b"duplicate content";

    let r1 = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("dup1.txt").mime_type("text/plain")))
        .await;
    assert_eq!(r1.status_code(), 202);

    let r2 = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(content.to_vec())
                .file_name("dup2.txt").mime_type("text/plain")))
        .await;
    assert_eq!(r2.status_code(), 201);

    let id1 = r1.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    let id2 = r2.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();
    assert_ne!(id1, id2);
}

#[tokio::test]
async fn test_delete_removes_record_returns_404() {
    let (server, _b, _d) = test_server().await;
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"to delete".to_vec())
                .file_name("del.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let del = server.delete(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .await;
    assert_eq!(del.status_code(), 204);

    let get = server.get(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .add_header(header::ACCEPT, "application/json")
        .await;
    assert_eq!(get.status_code(), 404);
}

#[tokio::test]
async fn test_put_updates_name_and_tags() {
    let (server, _b, _d) = test_server().await;
    let upload = server.post("/api/v1/blobs")
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .multipart(axum_test::multipart::MultipartForm::new()
            .add_part("file", axum_test::multipart::Part::bytes(b"data".to_vec())
                .file_name("orig.txt").mime_type("text/plain")))
        .await;
    let id = upload.json::<serde_json::Value>()["id"].as_str().unwrap().to_string();

    let updated = server.put(&format!("/api/v1/blob/{id}"))
        .add_header(header::AUTHORIZATION, "Bearer test-secret")
        .json(&serde_json::json!({ "name": "renamed.txt", "tags": ["finance"] }))
        .await;
    assert_eq!(updated.status_code(), 200);
    let body = updated.json::<serde_json::Value>();
    assert_eq!(body["name"], "renamed.txt");
    assert_eq!(body["tags"], serde_json::json!(["finance"]));
}
```

- [ ] **Step 2: Run integration tests**

```bash
cargo test --test integration_test
```

Expected: all 8 tests pass.

- [ ] **Step 3: Run full test suite**

```bash
cargo test
```

Expected: all tests pass.

- [ ] **Step 4: Final commit**

```bash
git add tests/integration_test.rs
git commit -m "feat: add integration tests covering full API lifecycle"
```

---

## Spec Coverage Check

| Spec Requirement | Covered By |
|---|---|
| `POST /api/v1/blobs` multipart upload | Task 15 |
| `GET /api/v1/blobs` list + `?s=` semantic search | Task 16 |
| `GET /api/v1/blobs?name=&tag=` filters (repeatable tag) | Task 16 (axum_extra Query) |
| `GET /api/v1/blob/:id` content negotiation | Task 17 |
| `PUT /api/v1/blob/:id` update name/tags | Task 17 |
| `DELETE /api/v1/blob/:id` dedup-aware delete | Task 17 |
| SHA-256 sharded filesystem (`ab/cd/{hash}`) | Tasks 5–6 |
| LanceDB schema with FixedSizeList embedding + all fields | Task 7 |
| LanceDB CRUD + ANN search | Task 8 |
| Vector index creation | Task 8 (`create_vector_index`) + Task 18 (main.rs) |
| Bearer token auth middleware | Task 14 |
| Ollama embed, summary, vision models | Tasks 9–11 |
| Content extraction (text/PDF/image/binary) | Task 10 |
| PDF vision fallback (<50 words) | Task 10 |
| Async pipeline worker with status transitions (pending→processing→ready/failed) | Task 12 |
| `Failed` status written with error message | Task 12 |
| `mark_processing` preserves existing fields | Task 8 (ops), Task 12 (worker) |
| PIPELINE_WORKERS env var (default 1) | Tasks 2, 13 |
| Multi-worker async-channel (no mutex serialization) | Task 13 |
| Startup recovery of pending/processing blobs | Task 13 |
| Content-addressed deduplication on upload | Task 15 |
| Dedup-aware delete (ref count check) | Task 17 |
| Dedup copies embedding/summary on re-upload | Task 15 |
| `202 Accepted` / `201 Created` on upload | Task 15 |
| Raw bytes always available regardless of processing status | Task 17, integration test |
| Config from env with sensible defaults + OLLAMA_EMBED_DIM | Task 2 |
| `AppError` → HTTP status mapping | Task 4 |
| Multi-stage Dockerfile | Task 19 |
| docker-compose with Ollama service + volumes | Task 19 |
| Version `1.0.0` in Cargo.toml | Task 1 |
