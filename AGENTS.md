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

## UI / Frontend

UI changes to `static/driver/` or `static/dispatch/` must be consistent with the design system defined in [`docs/DESIGN.md`](docs/DESIGN.md). Reuse the existing CSS tokens in `static/driver/css/base.css`; add a new token there (and document it in `DESIGN.md`) rather than inlining hex values or one-off styles.

## Release Workflow

All releases use a **single session**: backlog review → sprint planning (with PM) → plan doc + Opus plan review → branch creation → implementation via subagents → Opus code review → AGENTS.md lessons → issue closure → merge → tag → GitHub release.

### Version increment
- **Patch (x.y.Z):** bug fixes only, no new API surface or features
- **Minor (x.Y.0):** any new feature, endpoint, or UI capability

### Steps (in order — do not skip any)

1. **Backlog review:** fetch open issues, group by theme, present prioritised shortlist to PM. Wait for scope confirmation before proceeding.
2. **Plan:** use `superpowers:writing-plans` skill. Save to `docs/superpowers/plans/YYYY-MM-DD-vX.Y.Z.md`.
3. **Opus plan review:** spawn Opus as a subagent to verify file paths, method signatures, error variants, and completeness. Fix BLOCK findings inline; re-run before proceeding.
4. **Create release branch:** `git checkout main && git pull && git checkout -b vX.Y.Z && git push -u origin vX.Y.Z`. Commit the plan doc.
5. **Implement:** use `superpowers:subagent-driven-development` skill. After every subagent: `git diff --stat HEAD` (commit manually if skipped) and `cargo test && cargo clippy -- -D warnings` (never proceed red).
6. **Opus code review:** spawn Opus to review key changed files. Report PASS or BLOCK with file:line. Do not merge until PASS.
7. **AGENTS.md lessons:** add 2–4 concise lessons (rule + why + how to apply). Commit to the release branch.
8. **Close issues:** for each in-scope issue, leave a verification comment then `gh issue close N --reason completed`.
9. **Merge to main (ff-only):**
   ```bash
   git log --oneline main..vX.Y.Z        # must show commits
   git checkout main && git merge --ff-only vX.Y.Z && git push origin main
   git log --oneline -1 main && git log --oneline -1 vX.Y.Z   # must match
   ```
10. **Tag** (refs/tags/ prefix avoids branch/tag ambiguity):
    ```bash
    git tag vX.Y.Z && git push origin refs/tags/vX.Y.Z
    ```
11. **GitHub release:** `gh release create vX.Y.Z --title "..." --notes "..." --target main`. Group notes by Bug Fixes / Enhancements / Infrastructure; reference issue numbers; note any deferred Opus findings as "tracked in #N".
12. **Hand back to PM:** output what shipped, what's still open, any issues filed for deferred Opus findings.

### Opus triage rules (applies to both plan review and code review)
- **BLOCK:** fix inline, retest, re-run Opus before proceeding
- **Non-blocking, < 30 min fix:** fix inline
- **Non-blocking, needs more work:** file a GitHub issue (Opus finding as body); reference in release notes
- **Non-blocking, out of scope / needs design decision:** file a GitHub issue; do not fix this sprint; note as "tracked in #N"

### Key rules
- Parallelism lives **inside** one session via subagents — never across sessions owning the same worktree
- Opus plan review and Opus code review are both subagent calls, never separate sessions
- **Merge to main before tagging.** The tag must point to a commit on main. Tagging the branch tip instead of main leaves main stale and breaks the next release's branch point.
- **Before starting:** verify main is current: `git log --oneline -1 main` should match the previous release tag.
- **Subagent commit discipline:** after each subagent completes, run `git diff --stat HEAD` to confirm it committed. If the diff is non-empty, commit manually before proceeding.
- **Close issues with verification comments** — don't leave in-scope issues open at release.
- **Config::from_env() test flakiness is broader than one test:** any unit test that calls `Config::from_env()` can race when the full lib suite runs in parallel (env var mutation). Not just `test_config_from_env` — facility and other integration tests that call config init are also affected. Ignore intermittent failures in isolation; they pass when run alone.
- **Action endpoints that chain multiple mutations must re-fetch before returning.** When an action endpoint (assign, unassign, etc.) calls `transition_*` and then one or more additional mutations (e.g. `update_trip_resources`), the record returned by the first call is stale — it predates the later mutations. Always re-fetch via `get_*(id)` after all mutations are applied and before `Ok(Json(...))`. Tests that verify state via a subsequent GET will pass either way; only the response body reveals the bug.
- **Integration test assertions on action response bodies, not just subsequent GETs.** When testing multi-mutation actions (assign, unassign), assert on the response body of the action itself (e.g. `assign_resp.json()["driver_id"]`) in addition to a follow-up GET. A subsequent GET catches DB correctness but masks a stale-return bug in the handler.
- **Stop sequence vs. array index are different things.** `Stop.sequence` is 1-based (user-facing order); the array index in `trip.stops[i]` is 0-based. Never use `s.sequence` as an array index in arrive/depart cascade logic — it will write to the wrong stop and silently skip the last stop. Always derive the array index as `(s.sequence - 1)` or iterate with `enumerate()`.
- **Stop scheduled times must be naive datetime strings when a timezone is stored.** Store as `"2026-05-10T08:00:00"` (no Z, no offset), not UTC-normalized. A UTC-normalized string paired with a timezone field produces inconsistent data: `parse_stop_time` will misread the offset. Reject any string with a trailing `Z` or `+HH:MM` when `timezone` is `Some`.
- **Validate scheduled_arrive at input, not only at query time.** Run `validate_stop_time_str` on all `scheduled_arrive` and `scheduled_arrive_end` inputs in `resolve_stops` — not just in the arrive/depart endpoints. Inconsistent (timezone, UTC-string) pairs that reach the DB cannot be corrected later without a migration.
- **Making a required API field optional is a semver minor change; accepting a new required field is a breaking change.** For internal, controlled consumers that can be updated atomically with the server, this can be shipped as a patch — but call it out explicitly in the release notes so any future external consumers are warned.
- **Integration test fixtures must satisfy the API's own validation.** Test stop time strings like `"2026-05-10"` that predate validation rules will silently pass until the rule is added — then break en masse. Keep a canonical test constant for the current required format (e.g. `TEST_STOP_TIME = "2026-05-10T08:00:00"`) and reference it everywhere rather than inlining strings.
- **Use non-zero stop sequences in cascade tests to catch off-by-one bugs.** A stop with `sequence: 1` has `load_stop_index = 0`, which is the same value as `sequence - 1 = 0`. Using `sequence: 0` masks index errors. Always write cascade tests with at least one stop at `sequence ≥ 2` so a 1-based vs 0-based mixup is immediately visible.
- **Bump `CACHE_NAME` in `static/driver/sw.js` on every Driver PWA release.** The constant controls which cache version returning users load. If you change any file listed in `STATIC_ASSETS` (JS, CSS, icons, manifest), increment the version string (e.g. `driver-v2` → `driver-v3`). Forgetting this silently serves stale assets until the browser evicts the old cache.
- **`next_stop_name` means "not yet arrived", not "not yet departed".** The correct predicate is `actual_arrive.is_none()` — the first stop the driver hasn't reached yet. `actual_depart.is_none()` returns the stop they're currently at (arrived but not left), which is the current stop, not the next one.
- **Opus non-blocking findings go to the backlog, not the floor.** Any non-critical finding from an Opus review that doesn't need to be fixed before merge should be filed as a GitHub issue immediately. Don't let them get lost in session transcripts.
- **Two parallel execution tracks sharing one test file will conflict at merge.** When two orchestrator sessions both add tests to `tests/integration_test.rs`, the merge will conflict at the same insertion point. Resolve by keeping both blocks in full; don't drop either track's tests.
- **Dispatcher JWT secret is a required production env var.** `DISPATCHER_JWT_SECRET` (min 32 bytes) must be set on the server before deploying any release that includes the dispatcher portal. Include this in release notes whenever the dispatcher feature ships for the first time.
- **WebAuthn challenge response must include driver_id.** The passkey auth finish endpoint (`POST /auth/verify`) requires a `driver_id` to look up the stored challenge. Return `{ driver_id, challenge }` from the challenge endpoint so the frontend can pass it back — without it, the finish call has no way to identify which driver is authenticating.
- **CSS polish passes need a hex-value scan.** After any CSS edit session, grep for raw hex literals (`#[0-9a-fA-F]`). Two Opus BLOCK findings in v1.5.0 were raw hex values (`#fff`, `#f59e0b` as a fallback) that should have been design tokens. The ban on inline hex applies to every property — color, background, border, box-shadow, etc.
- **Check ALL instances of a UI pattern when polishing.** When migrating a CSS pattern (e.g. back buttons to `.btn-ghost-back`), grep for every selector variant before closing the task. The login "← Different number" button used a different class name than the trip/stop detail back buttons and was missed in the initial pass — Opus caught it as a BLOCK finding.

## Commit Style

- Use `feat:`, `fix:`, `refactor:`, `test:`, `chore:` prefixes
- Co-author with the current model name:
  ```
  Co-Authored-By: Claude with <model-name>
  ```
