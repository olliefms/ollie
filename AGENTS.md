# ollie — Agent Guide

This file is for AI coding agents working on this codebase. Read it before making changes.

## Project Overview

ollie is a RAG-enabled blob store written in Rust. It accepts file uploads, stores them content-addressed on disk, and uses Ollama to generate summaries and embeddings. Blobs are indexed in LanceDB for semantic search.

**Stack:** Axum 0.7, LanceDB 0.27, Arrow 57, async-channel 2, reqwest 0.12, lopdf 0.32

## Codebase Layout

```
src/
  lib.rs          — module declarations + AppState
  main.rs         — startup: config, db, store, ai, pipeline, HTTP server
  config.rs       — Config::from_env()
  models.rs       — BlobRecord, BlobStatus, BlobListItem, BlobListResponse, UpdateBlobRequest
  error.rs        — AppError enum + IntoResponse impl
  storage/
    mod.rs        — BlobStore: write/read/delete/exists, compute_checksum
    shard.rs      — shard_path(base, checksum) → 2-level shard: ab/cd/fullhash
  db/
    mod.rs        — DbClient { table, embed_dim }, blob_schema(), creates/opens "blobs" table
    ops.rs        — all DB operations: insert, get, list, update, delete, search, mark_*
  ai/
    mod.rs        — OllamaClient: embed(), generate()
    extract.rs    — Extractable enum, extract_content(), bytes_to_base64()
    embed.rs      — embed_text()
    summarize.rs  — summarize_text(), describe_image()
  pipeline/
    mod.rs        — spawn_pipeline() → (Sender<Uuid>, JoinHandles)
    worker.rs     — process_blob(): mark_processing → extract → summarize/embed → mark_ready/failed
    recovery.rs   — requeue_stale(): re-queues pending/processing blobs at startup
  api/
    mod.rs        — router() with all routes + bearer auth middleware
    auth.rs       — require_bearer() middleware
    blobs.rs      — POST /blobs (upload), GET /blobs (list/search)
    blob.rs       — GET /blobs/:id, PUT /blobs/:id, DELETE /blobs/:id
```

## Critical Version Notes

### LanceDB + Arrow

**Do not change these versions without reading this section.**

- `lancedb = "0.27"` — lancedb 0.9 (originally in the design plan) had an arrow/chrono incompatibility and was abandoned
- `arrow-array = "57"` and `arrow-schema = "57"` — lancedb 0.27.2 bundles arrow 57.2 internally; using arrow 52 or 53 causes `RecordBatchReader` trait incompatibility at compile time
- If you upgrade lancedb, check its bundled arrow version first: `cargo tree -p lancedb | grep arrow`

### LanceDB 0.27 API Differences

The plan was written for an older API. The actual 0.27 API differs:

- Use `.only_if("condition")` not `.filter("condition")` for row filtering
- Queries require traits in scope: `use lancedb::query::{ExecutableQuery, QueryBase};`
- `table.query().execute()` returns a `RecordBatchStream`, not a stream of batches — call `.await` then iterate
- Vector index: `lancedb::index::Index::IvfPq(Default::default())`

## DB Operations Patterns

### Update Pattern — Use merge_insert, never delete+insert

All update operations (status transitions, metadata updates, field mutations) must use LanceDB's `merge_insert` rather than `delete` followed by `insert`. The delete+insert pattern is non-atomic: a crash between the two operations permanently deletes the record.

The canonical pattern is a private `upsert_*` method on `DbClient` in each ops module:

```rust
async fn upsert_blob(&self, record: &BlobRecord) -> Result<(), AppError> {
    let batch = record_to_batch(record, self.embed_dim)?;
    let schema = blob_schema(self.embed_dim);
    let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
    let mut op = self.blob_table.merge_insert(&["id"]);
    op.when_matched_update_all(None).when_not_matched_insert_all();
    op.execute(reader).await
        .map(|_| ())
        .map_err(|e| AppError::Internal(e.to_string()))
}
```

Key points:
- Match on `&["id"]` — the primary key column
- `when_matched_update_all(None)` — replace the row on match (no filter condition)
- `when_not_matched_insert_all()` — insert if somehow absent (safe fallback, rarely triggered)
- `initial insert` (first-time record creation) still uses `table.add(...).execute()` directly
- All new tables in v1.2.0+ must follow this pattern from day one — do not copy the old delete+insert pattern

This applies to every ops module: `blob_ops.rs`, `load_ops.rs`, `facility_ops.rs`, and all future `*_ops.rs` files.

### Dangling &str — Always bind to a local variable

This is wrong and will fail to compile:
```rust
// WRONG — .as_str() borrows a temporary
StringArray::from(vec![record.id.to_string().as_str()])
```

Always bind first:
```rust
// CORRECT
let id_str = record.id.to_string();
StringArray::from(vec![id_str.as_str()])
```

This pattern appears in `record_to_batch` for every field. Follow it.

### mark_processing preserves existing fields

`mark_processing` uses fetch → modify status only → `upsert_blob`. It does NOT touch `summary` or `embedding`. If you need to change this, be careful not to break recovery: a blob that was mid-processing when the server crashed must be re-processable.

### Status transitions

```
pending → processing → ready
                     → failed
```

- `mark_processing`: fetch record, set status to "processing", upsert
- `mark_ready(id, summary, embedding)`: sets all AI output fields, clears error, upsert
- `mark_failed(id, error)`: sets error string, leaves AI fields as-is, upsert

## Pipeline Architecture

`spawn_pipeline()` creates a bounded `async_channel::bounded(256)` channel and spawns N worker tasks, each holding a clone of the receiver. Blobs are sent by UUID after DB insert and storage write.

**Why async-channel instead of tokio mpsc?** tokio's mpsc receiver is not Clone; sharing it across workers requires `Arc<Mutex<Receiver>>`. async-channel receivers are Clone, so each worker gets its own clone — no locking needed.

## Test Patterns

### async_channel receiver must stay alive

This is the most common test failure:

```rust
// WRONG — receiver is dropped, channel closes, uploads return 500
let (server, _db_dir, _blob_dir) = test_server().await;

// CORRECT — receiver kept alive for duration of test
let (server, _db_dir, _blob_dir, _rx) = test_server().await;
```

If uploads return 500 in tests, check this first.

### axum-test response bytes

`axum-test 15.7.4` uses `as_bytes()`, not `bytes()`:

```rust
// WRONG
let body = resp.bytes();

// CORRECT
let body = resp.as_bytes();
```

### Config test flakiness

`test_config_from_env` in `config.rs` is known to be flaky when the full test suite runs in parallel. The test mutates env vars (`set_var`/`remove_var`) and can race with other tests. If it fails intermittently, that's expected — it passes in isolation.

### Integration tests require no Ollama

The integration tests in `tests/integration_test.rs` do not call Ollama. The pipeline receiver is kept alive but workers are never polled. All upload tests check HTTP response codes only, not AI output.

## OpenAPI / Agent Discoverability

The API is documented via `utoipa` (v4). Two unauthenticated endpoints serve the spec:

- `GET /openapi.json` — full OpenAPI 3.0 spec (auto-generated)
- `GET /llms.txt` — plain-text summary written for LLM consumption

### Required for every new endpoint

1. **Handler**: Add `#[utoipa::path(...)]` directly above the handler function.
   - Use OpenAPI path syntax (`{id}` not `:id`)
   - Include `security(("BearerAuth" = []))` for authenticated endpoints
   - Set `tag` to `"blobs"`, `"facilities"`, or `"loads"` (or a new tag for new resource groups)
   - Document all responses including 400/401/404/409 where applicable

2. **Request/response types**: Add `#[derive(utoipa::ToSchema)]` to every struct or enum used
   in request bodies or responses. Fields with `#[serde(skip)]` also need `#[schema(skip)]`
   to be excluded from the generated schema.

3. **Query parameter structs**: Add `#[derive(utoipa::IntoParams)]` and
   `#[into_params(parameter_in = Query)]`. Reference the struct with `params(MyQuery)` in
   the path annotation. Add `#[param(description = "...")]` per field.

4. **Register in ApiDoc**: Add the handler path to the `paths(...)` list in the `ApiDoc`
   struct in `src/api/mod.rs`. Add any new schema types to the `schemas(...)` list.

5. **llms.txt**: Review and update `LLMS_TXT` in `src/api/mod.rs` when new endpoint groups
   are added or auth behaviour changes. The text is hand-written; keep it concise.

## API Auth

All endpoints require `Authorization: Bearer <ADMIN_API_KEY>`. The key is set via the `ADMIN_API_KEY` environment variable (required — no default). Missing or wrong key → 401.

`ADMIN_API_KEY` is arbitrary — any non-empty string works. For local dev or testing, `ADMIN_API_KEY=test-key` is fine. There is no provisioning or secret management required.

## Deduplication Logic

On upload, the server computes SHA-256 of the file bytes:
- If checksum exists in DB: return **201 Created** (copies summary/embedding from existing record to new record)
- If checksum is new: return **202 Accepted** (queues for processing)

Both paths write to the DB. The filesystem file is only written once per unique checksum.

## Content Extraction

`extract_content()` in `src/ai/extract.rs` returns an `Extractable` enum:

- `Text(String)` — for `text/*`, `application/json`, `application/xml`, and PDFs with ≥50 words
- `ImageBytes(Vec<u8>)` — for `image/*` and PDFs with <50 extractable words
- `Unsupported` — everything else

PDFs use `lopdf::Document::load_mem()`. If lopdf can't extract ≥50 words, the PDF is treated as an image and sent to the vision model.

## Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `ADMIN_API_KEY` | Yes | — | Bearer token for all API requests |
| `PORT` | No | `3000` | HTTP listen port |
| `DATA_DIR` | No | `./data` | Root for blob storage and LanceDB |
| `OLLAMA_BASE_URL` | No | `http://localhost:11434` | Ollama API base URL |
| `OLLAMA_EMBED_MODEL` | No | `nomic-embed-text` | Embedding model |
| `OLLAMA_SUMMARY_MODEL` | No | `llama3.2` | Text summarization model |
| `OLLAMA_VISION_MODEL` | No | `moondream` | Vision/image description model |
| `OLLAMA_EMBED_DIM` | No | `768` | Embedding dimension (must match model) |
| `PIPELINE_WORKERS` | No | `1` | Concurrent pipeline workers |

## Running Tests

```bash
# All tests (unit + integration)
cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml

# Integration tests only
cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml --test integration_test

# Single test
cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml test_upload_returns_202
```

After any change: run `cargo test`, `cargo clippy`, `cargo build` before committing.

## Commit Style

- Use `feat:`, `fix:`, `refactor:`, `test:`, `chore:` prefixes
- Co-author with the current model name:
  ```
  Co-Authored-By: Claude with <model-name>
  ```
