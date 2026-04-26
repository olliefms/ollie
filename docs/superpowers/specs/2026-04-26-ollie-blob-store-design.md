# Ollie — RAG-enabled Blob Store Design

**Version:** v1.0.0  
**Date:** 2026-04-26  
**Stack:** Rust · Axum · LanceDB · Ollama · Docker

---

## Versioning Policy

- **Patch** (1.0.x): bug fixes only
- **Minor** (1.x.0): new functionality, backwards compatible
- **Major** (x.0.0): breaking API change that warrants a new `/api/v2` route tree

---

## Architecture Overview

Single Rust binary, single Axum crate, organized into focused modules. All shared state is carried in an `Arc<AppState>` injected via Axum's `State` extractor.

```
Client
  └─ HTTP ──► BearerAuth middleware
                └─ Router
                    ├─ POST   /api/v1/blobs       (api/blobs.rs)
                    ├─ GET    /api/v1/blobs        (api/blobs.rs)
                    ├─ GET    /api/v1/blob/:id     (api/blob.rs)
                    ├─ PUT    /api/v1/blob/:id     (api/blob.rs)
                    └─ DELETE /api/v1/blob/:id     (api/blob.rs)

AppState
  ├─ db:           Arc<DbClient>         (LanceDB)
  ├─ store:        Arc<BlobStore>        (sharded filesystem)
  ├─ ai:           Arc<OllamaClient>     (Ollama HTTP client)
  ├─ pipeline_tx:  mpsc::Sender<Uuid>   (background job channel)
  └─ config:       Arc<Config>
```

---

## Module Structure

```
src/
  main.rs              startup, config load, server bind, pipeline spawn
  config.rs            Config struct from env vars
  error.rs             AppError enum → axum IntoResponse
  models.rs            BlobRecord, Status, Tags

  api/
    mod.rs             Router assembly, middleware wiring
    auth.rs            Bearer token middleware (tower Layer)
    blobs.rs           POST /api/v1/blobs, GET /api/v1/blobs
    blob.rs            GET/PUT/DELETE /api/v1/blob/:id

  storage/
    mod.rs             BlobStore: write, read, delete, exists
    shard.rs           Path derivation: checksum → blobs/ab/cd/{sha256}

  db/
    mod.rs             LanceDB connection, table creation/migration
    ops.rs             insert, get_by_id, get_by_checksum, update, delete,
                       count_by_checksum, list, search

  ai/
    mod.rs             OllamaClient, model config, HTTP calls
    extract.rs         Text extraction by file type (text / PDF / image / binary)
    embed.rs           Generate embedding vector via embed model
    summarize.rs       Generate summary text via summary or vision model

  pipeline/
    mod.rs             Spawn N workers, return mpsc::Sender<Uuid>
    worker.rs          Job loop: pending → processing → extract → summarize → embed → ready/failed
    recovery.rs        On startup: re-queue all blobs with status pending or processing
```

---

## Configuration (Environment Variables)

| Variable | Default | Required |
|---|---|---|
| `ADMIN_API_KEY` | — | yes |
| `PORT` | `3000` | no |
| `BLOB_STORE_PATH` | `./data/blobs` | no |
| `LANCEDB_PATH` | `./data/lancedb` | no |
| `OLLAMA_BASE_URL` | `http://localhost:11434` | no |
| `OLLAMA_EMBED_MODEL` | `nomic-embed-text` | no |
| `OLLAMA_SUMMARY_MODEL` | `llama3.2` | no |
| `OLLAMA_VISION_MODEL` | `llava` | no |
| `PIPELINE_WORKERS` | `1` | no |

`PIPELINE_WORKERS=1` is the recommended default for low-end hardware — one Ollama request in flight at a time.

---

## Authentication

All endpoints require:

```
Authorization: Bearer <ADMIN_API_KEY>
```

Implemented as a tower `Layer` in `api/auth.rs` applied to the entire router. Returns `401 Unauthorized` on missing or invalid token. `owner_id` is always `0` (admin) in v1; the field is present in the schema for future multi-user support.

---

## Storage Layer

### Filesystem Sharding

Files are stored by their SHA-256 checksum using a 2-level, 1-byte-per-level directory structure:

```
{BLOB_STORE_PATH}/
  ab/
    cd/
      abcdef1234...  (full SHA-256 hex, no extension)
```

`shard.rs` derives the path: `bytes[0]` → level 1 (2 hex chars), `bytes[1]` → level 2 (2 hex chars). This yields 256 × 256 = 65,536 possible leaf directories.

### Content-Addressed De-duplication

On upload, the SHA-256 checksum is computed from the incoming bytes before any write. If a file with that checksum already exists on disk:

1. Skip the filesystem write
2. Copy `embedding`, `summary`, and `status: ready` from any existing LanceDB record with the same checksum
3. Insert a new LanceDB record immediately as `ready`
4. Return `201 Created` with the new UUID

If the checksum is new, write the file, insert a `pending` record, enqueue the UUID, and return `202 Accepted`.

---

## LanceDB Schema

Table name: `blobs`

| Field | Type | Notes |
|---|---|---|
| `id` | `String` (UUID) | Primary key |
| `owner_id` | `Int64` | Default `0` (admin); multi-user ready |
| `checksum` | `String` | SHA-256 hex; used for de-dup and shard path |
| `name` | `String` | Display name; mutable |
| `mime_type` | `String` | Detected at upload time |
| `size` | `Int64` | Bytes |
| `status` | `String` | `pending` / `processing` / `ready` / `failed` |
| `error` | `String?` | Set on `failed`; null otherwise |
| `summary` | `String?` | Generated by summary/vision model |
| `tags` | `String` | JSON array; mutable |
| `embedding` | `FixedSizeList<Float32>` | Dimension matches embed model output |
| `created_at` | `String` | ISO 8601 |
| `updated_at` | `String` | ISO 8601; updated on PUT |

Indexes: `checksum` (for de-dup count queries), `owner_id` (for future multi-user filtering), vector index on `embedding` (for ANN search).

---

## API Contracts

### `POST /api/v1/blobs`

Upload a blob. Accepts `multipart/form-data`.

**Request fields:**
- `file` (binary, required)
- `name` (string, optional) — overrides the filename from `Content-Disposition`
- `tags` (JSON array string, optional)

**Response:**
- `202 Accepted` — new file, queued for processing
- `201 Created` — duplicate checksum, record immediately ready

```json
{
  "id": "uuid",
  "name": "report.pdf",
  "status": "pending",
  "checksum": "sha256hex",
  "size": 204800,
  "mime_type": "application/pdf",
  "owner_id": 0,
  "created_at": "2026-04-26T12:00:00Z"
}
```

---

### `GET /api/v1/blobs`

List blobs or perform semantic search. All params are optional and combinable.

**Query params:**
- `s=<text>` — semantic search (embeds the query, runs ANN against LanceDB)
- `name=<string>` — partial match on `name`
- `tag=<string>` — exact tag match; repeatable (`?tag=finance&tag=2024`)
- `limit=<int>` — default `20`, max `100`
- `offset=<int>` — default `0`; ignored when `s=` is present (results ranked by score)

When `s=` is present, `total` reflects the number of items returned (up to `limit`), not a database count — ANN search returns top-N by score with no natural total.

**Response `200 OK`:**
```json
{
  "total": 42,
  "items": [
    {
      "id": "uuid",
      "name": "report.pdf",
      "mime_type": "application/pdf",
      "size": 204800,
      "status": "ready",
      "summary": "Quarterly financial report...",
      "tags": ["finance", "2024"],
      "owner_id": 0,
      "created_at": "2026-04-26T12:00:00Z",
      "score": 0.91
    }
  ]
}
```

`score` is present only when `s=` is provided.

---

### `GET /api/v1/blob/:id`

Content negotiation on `Accept` header.

- `Accept: application/json` → full metadata JSON (all fields including current `status`, `summary`, `updated_at`)
- `Accept: */*` or `Accept: application/octet-stream` → raw file bytes with correct `Content-Type` and `Content-Disposition` headers

The file is written to disk synchronously during upload before the `202` is returned, so raw bytes are always available regardless of processing status. Embedding and summary enrichment does not gate data access. Returns `404` if the ID is not found.

---

### `PUT /api/v1/blob/:id`

Update mutable metadata. Request body is `application/json`. At least one field required.

```json
{ "name": "new-name.pdf", "tags": ["finance", "revised"] }
```

Returns `200 OK` with the full metadata object (same shape as `GET` JSON response).

---

### `DELETE /api/v1/blob/:id`

De-duplication-aware delete:

1. Count LanceDB records sharing the same `checksum`
2. If count == 1: delete file from filesystem, then delete DB record
3. If count > 1: delete DB record only (file and other IDs remain valid)

Returns `204 No Content`. Returns `404` if ID not found.

---

## AI Processing Pipeline

### File Type Handling

| Type | Detection | Processing |
|---|---|---|
| Text (`.txt`, `.md`, `.json`, `.csv`, code, etc.) | MIME type / extension | Read content → summarize → embed |
| PDF | `application/pdf` | Extract text with pdfium; if extracted text is fewer than 50 words, fall back to vision model per page |
| Image (`.jpg`, `.png`, `.gif`, `.webp`, etc.) | MIME `image/*` | Send to vision model → description → embed description |
| Binary / unknown | Everything else | Skip AI; set `status: ready`, `summary: null`, `embedding: null` |

### Async Worker

Upload handlers send only the `Uuid` into an `mpsc` channel. Workers consume from the channel:

```
pending → processing → [extract text]
                     → [summarize]     (summary model or vision model)
                     → [embed]         (embed model on extracted/described text)
                     → ready
                     ↘ failed          (error stored in record)
```

`PIPELINE_WORKERS` workers drain the channel. Default is `1` — serial processing, ideal for single-GPU or CPU-only Ollama.

### Startup Recovery

On startup, before the HTTP server begins accepting requests, `pipeline/recovery.rs` queries LanceDB for all records with `status IN ('pending', 'processing')` and sends their UUIDs into the pipeline channel. This handles crashes mid-processing.

---

## Error Handling

`error.rs` defines `AppError` variants that implement Axum's `IntoResponse`:

| Variant | HTTP Status |
|---|---|
| `NotFound` | 404 |
| `Unauthorized` | 401 |
| `BadRequest(msg)` | 400 |
| `Conflict(msg)` | 409 |
| `Internal(msg)` | 500 |

All error responses return JSON: `{ "error": "<message>" }`.

---

## Docker

### `Dockerfile`

Multi-stage build:
1. `builder` — `rust:latest`, compile release binary
2. `runtime` — `debian:bookworm-slim`, copy binary + runtime deps (pdfium)

### `docker-compose.yml`

Two services:

```yaml
services:
  blob-store:
    build: .
    ports: ["3000:3000"]
    environment:
      OLLAMA_BASE_URL: http://ollama:11434
      BLOB_STORE_PATH: /data/blobs
      LANCEDB_PATH: /data/lancedb
    volumes:
      - ./data:/data
    depends_on: [ollama]

  ollama:
    image: ollama/ollama
    volumes:
      - ollama-models:/root/.ollama
    ports: ["11434:11434"]

volumes:
  ollama-models:
```

The standalone `Dockerfile` connects to Ollama via `OLLAMA_BASE_URL` — suitable for any deployment where Ollama runs separately.

---

## Key Design Decisions

| Decision | Rationale |
|---|---|
| UUID as primary ID, checksum for storage path | Decouples identity from content; enables de-dup without collisions |
| Single pipeline worker default | Prevents GPU contention on low-end hardware; bumping `PIPELINE_WORKERS` scales up |
| Startup recovery pass | Handles mid-processing crashes without a separate job queue |
| Copy embedding on de-dup upload | Avoids redundant Ollama round-trip for identical content |
| DELETE filesystem-last | Orphaned file with no record is preferable to orphaned record with no file |
| `owner_id` = 0 for all v1 records | Schema is multi-user ready; no migration needed when auth is extended |
