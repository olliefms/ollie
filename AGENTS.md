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
| TERMINAL_TIMEZONE | No | America/New_York | IANA timezone for pay-period weeks (driver-portal Past tab). Replaced by per-terminal data in a future release (#185). |

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

## Shippability Bar

"Done" means **all** of:
- No correctness, security, or data-loss bugs
- Critical paths covered by tests
- No broken contracts (API, schema, public types)

Style, taste, micro-optimizations, and refactor opportunities are **not** shippability blockers. They get noted in the PR description under `## Notes` if worth mentioning, then discarded. They do **not** become GitHub issues unless the user explicitly files one. The backlog is not a landfill for robot homework.

## Triage Rules

Every review finding (self-review or Opus subagent) is classified:

- **blocker** — violates the Shippability Bar. Must fix before merge.
- **significant** — meaningful issue that affects maintainability or correctness in edge cases. Fix in-PR if < 30 min; otherwise stop and discuss with the user. Never defer with a "tracked elsewhere" handwave.
- **nit** — style, taste, micro-opt, refactor opportunity. Note in PR `## Notes` if a pattern, otherwise discard. Never file as an issue.

**Hard cap: 2 Opus review iterations.** If iteration 2 still finds blockers, stop and escalate to the user. Looping further is a sign the change needs human eyes.

## Release Workflow

Trunk-based. Three skills cover the workflow:

- **`/work-issue <N>`** — default unit of work. One issue → branch off main → PR back to main → self-merge if Shippability Bar is met.
- **`/sprint-plan`** — exception, for cross-cutting work that must land atomically. Plans + executes on a feature branch, one PR to main.
- **`/cut-release`** — when main has accumulated enough work to ship. Bumps version, tags, generates release notes. No release branch involved.

### Version increment
- **Patch (x.y.Z):** bug fixes only, no new API surface or features
- **Minor (x.Y.0):** any new feature, endpoint, or UI capability

### Key rules
- **Bump version, then tag.** The tag points to the bump commit on main.
- **Use `refs/tags/vX.Y.Z` when pushing tags** to avoid branch/tag ambiguity.
- **Subagent commit discipline:** after each subagent completes, run `git diff --stat HEAD` to confirm it committed. If the diff is non-empty, commit manually before proceeding — one subagent silently skipped its commit in v1.3.1.
- **Close issues with verification comments** — don't leave in-scope issues open after release.
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
- **`transition_trip_status` enforces the state machine at the DB layer — verify before adding handler guards.** `trip_ops.rs::transition_trip_status` checks `can_transition_to` and returns `AppError::Conflict` for any illegal transition. Handlers only need explicit status checks for cases the machine intentionally allows but business rules forbid (e.g. blocking dispatch when the driver is already dispatched on another trip). When Opus flags a missing guard, check the transition table first — the BLOCK may be a false positive.
- **Batch-fetch-then-resolve is the correct N+1 fix for list endpoints.** Collect all foreign-key IDs across the page into a `HashSet`, call `batch_get_*(&ids) -> HashMap<Uuid, Record>` once, then resolve names synchronously from the map. Using `join_all` for N individual fetches is still O(N) round-trips even if parallelized. Add `batch_get_*` methods to `DbClient` for every entity type that appears in list views.
- **Frontend stop-name fallbacks must never expose UUIDs to users.** The pattern `stop.facility_name || stop.facility_id || '—'` leaks a raw UUID when the name is absent. Always prefer `stop.facility_name || stop.name || '—'` — or just `'—'`. Audit every `|| uuid` and `|| shortId(uuid)` chain in list and detail views after a UUID-display audit pass.
- **Dispatcher DTO enrichment belongs in a single `enrich_trip` async fn.** When a response type needs denormalized fields (driver_name, truck_unit, trailer_units), put all async resolution in one `enrich_trip(&state, item) -> Result<EnrichedItem>` helper. Scattering individual `get_driver / get_truck / get_trailer` calls across handlers is hard to maintain and hard to audit for N+1 regressions.
- **Mirror-API handlers inherit OpenAPI security schemes from the existing API, not a new name.** When creating dispatcher portal handlers that mirror admin blob handlers, use `security(("BearerAuth" = []))` — the scheme registered in `SecurityAddon`. Using an unregistered name like `"DispatcherJwt"` produces a spec that references an undefined scheme, breaking validators and tooling even though runtime behavior is unaffected. Always verify the registered scheme name in `src/api/mod.rs::SecurityAddon` before writing utoipa annotations.
- **Escape server-controlled string data before injecting into innerHTML in the SPA.** Blob names, tags, and AI-generated summaries are the most likely fields to carry adversary-influenced content. Use `escHtml(s)` on every field that flows from API responses into template literals. Add the utility once (already present as `escHtml` in `app.js`) and apply it wherever blob data appears — both in list views and in per-record detail panels.
- **Pre-validate all resources in a multi-step assign handler before any DB mutation.** Fetch and check ALL trailers (or any other multi-item list) in a loop before calling `transition_trip_status` or any other mutation. A validation failure after mutations leave the system in an inconsistent state that requires manual correction. Collect validated records into a `Vec` and reuse them in the mutation loop — this also eliminates redundant DB round-trips.
- **Use `API_BASE` for all apiFetch calls in the SPA; never hard-code path prefixes.** Hard-coded `/dispatch/api/v1/...` paths diverge silently when `API_BASE` changes. Every `apiFetch` call must use the `API_BASE` constant: `apiFetch(\`\${API_BASE}/blobs?...\`)`. Check for bare `/dispatch/api/v1` strings after every frontend feature addition.
- **Fetch a shared resource once and reuse across multiple derivations in the same handler.** When a handler needs a record for both stop derivation AND computed fields (e.g. loaded_miles, load_number), fetch it once at the top and pass it by reference to helpers. The v1.8.0 `create_trip` rewrite eliminated a redundant `get_load_by_id` call that previously happened inside the stops block.
- **Enrich denormalized fields at write time, not read time.** Fields like `stop.address`, `trip.load_number`, and `trip.previous_trip_id` are resolved once at creation and stored. This keeps read paths fast and avoids N+1 lookups in list views — even though it means the denormalized value may drift from source if the source changes. Use this pattern for data that won't change often and is expensive to recompute on every read.
- **Header injection from user-controlled strings is a real risk in Rust HTTP handlers.** Any field written into a header value (e.g., `Content-Disposition: attachment; filename="..."`) must be sanitized before use. Strip `\r` and `\n` at minimum — a blob name with a newline can inject additional HTTP headers. Do not assume the DB layer validates this.
- **`classify_trip` fallback logic: safer to default to pre-departure than active.** When an Assigned trip has no stops or an unparseable schedule, routing it to `TripTab::Current` (active) is wrong — the trip hasn't started. The safe default for any pre-departure status is the Upcoming tab. Write the guard as `Some(dt) if dt <= now => Current` so the `_` arm catches both missing and future schedules.
- **LanceDB schema migration must use `open_or_create_*` per table, not the generic `open_or_create`.** The generic helper opens an existing table without schema checks. Any table that gains new columns needs its own migration function (see `open_or_create_trip`, `open_or_create_facility`) that calls `table.add_columns(NewColumnTransform::SqlExpressions(...))` for each missing field. One missed migration causes all writes to fail on existing deployments.
- **SPA `goBack()` must not call `navigate()` — use a shared render function instead.** `navigate()` pushes to history before rendering. If `goBack()` calls `navigate()`, pressing Back twice re-adds the current view to history, creating an infinite loop. Extract the render-and-update logic into `_renderView(view, params)` and have both `navigate()` (which pushes first) and `goBack()` (which pops first) call `_renderView` directly.
- **KPI count endpoints on LanceDB should access `state.db.*_table.count_rows()` directly rather than going through list helpers.** The list helpers fetch + deserialize rows just to return the total, which is wasteful for a pure count. LanceDB's `count_rows(filter)` is a cheap metadata operation. Add count methods or call the table directly in the handler.
- **Sort by `created_at DESC` happens after `collect_stream`, not in the query.** LanceDB 0.27 does not have reliable `order_by` support. Follow the trip_ops pattern: call `batches_to_*`, then `records.sort_by_key(|r| std::cmp::Reverse(r.created_at))`, then skip/map. Do not rely on insertion order for user-facing list views.
- **LanceDB migration CAST expressions require lowercase DataFusion type names.** LanceDB 0.27's bundled DataFusion SQL parser rejects `Utf8` and `Double` — use `utf8` and `float64`. Applies to all `add_columns(NewColumnTransform::SqlExpressions(...))` calls in migration helpers.
- **Removing a DB-level `.limit()` from a list query requires adding `.take(limit)` to the iterator.** When you remove `.limit(limit + offset)` from a LanceDB query (e.g. to fix a sort-window bug), the in-memory iterator no longer has a page cap. Always add `.skip(offset).take(limit)` to the iterator after `sort_by_key` — without `.take(limit)`, the endpoint returns all remaining rows.
- **CSS class attribute injection needs the same allowlist as inner-text injection.** Before interpolating any API-derived value into a CSS class string (e.g. `badge--${entityType}`), apply `.replace(/[^a-z0-9_]/g, '_')` — the same pattern used in the `badge()` utility. A raw `entity_type` containing `"` or whitespace becomes an XSS vector via class attribute breakout.
- **Driver PWA: `position: fixed; height: 100dvh` is the correct scroll-containment pattern for full-viewport list views.** `height: 100vh` with a flex parent using `min-height` leaks scroll to the body. Use `position: fixed; top: 0; left: 0; width: 100%; height: 100dvh` to take the full viewport; `100dvh` avoids the iOS Safari browser-chrome clip. Other views that use body scroll are unaffected.
- **`enrich_trip` and similar enrichment helpers must be synchronous and accept pre-fetched maps, not call the DB per item.** An async `enrich_trip` that fires `get_driver`, `get_truck`, and `get_trailer` per trip is O(trips × entities) DB roundtrips. The correct pattern: collect all unique IDs in one pass, call `tokio::try_join!(batch_get_drivers, batch_get_trucks, batch_get_trailers)`, build `HashMap<Uuid, String>` name maps, then map synchronously. Apply this pattern to any list enrichment function. — Why: 20 trips × 3 trailers = 60 DB queries collapsed to 3. — How to apply: whenever you see `join_all` over individual `get_*` calls in a list handler, refactor to batch.
- **`batch_get_*` methods belong on `DbClient`, not in handlers.** Any entity type that appears in list views needs a `batch_get_*` method in its `*_ops.rs` file. Pattern: early-return empty map on empty input; build `id IN (...)` filter string; call `.only_if(filter)` (not `.filter()`); return `HashMap<Uuid, Record>`. — Why: consistency and reuse — handlers and tests can rely on a single well-tested batch path. — How to apply: add `batch_get_*` when writing any new entity type, not only when an N+1 regression is discovered.
- **A scan cap with in-memory sort diverges from `count_rows` total at deep offsets.** `list_loads` caps the LanceDB scan at 2000 rows then paginates in memory, but `count_rows` returns the full unfiltered total. A client requesting offset > 2000 gets an empty page while the total still shows more records. Document any scan cap with a constant and comment; if load volume grows, raise the cap or switch to cursor pagination. — Why: silent empty pages are confusing and hard to debug. — How to apply: always pair a scan cap with a comment explaining the divergence; add a UI-side offset clamp when building pagination UI.
- **Apply `escHtml()` to ALL fields that flow from API responses into innerHTML, not just the ones that seem dangerous.** The v1.11.0 review found pre-existing unescaped `load_number` and `customer_name` in the load detail view. When adding `escHtml` to one part of a component, grep for all adjacent interpolations and escape them in the same commit. — Why: partial escaping leaves XSS surface area that reviewers and linters won't catch. — How to apply: after any `escHtml` fix, grep for `\${` patterns in the same template and verify each one is wrapped.
- **Document preview iframes must carry `sandbox=""` (empty string) when the src is a blob URL.** A `blob:` URL inherits the creating document's origin — an HTML blob without a sandbox attribute can execute scripts in the dispatcher's session. `iframe.sandbox = ''` blocks scripts, same-origin access, and form submission while still rendering PDFs and images. — Why: any user who can upload a blob can plant an HTML file that steals the session token. — How to apply: whenever you create an iframe with a blob URL, add `iframe.sandbox = '';` immediately after setting the src.
- **`history.replaceState` (not `pushState`) belongs inside `_renderView`; `pushState` belongs in `navigate`.** `navigate()` records history and calls `_renderView`. If `_renderView` used `pushState`, every internal re-render would double-push, bloating history and breaking Back. Use `replaceState` inside `_renderView` to mirror state without adding history entries. — Why: the distinction prevents the double-push trap when the same view is re-rendered. — How to apply: any time you mirror router state into the URL inside a render helper, use `replaceState`.
- **CSS design token fallbacks inside `var()` must use the same token from `base.css`, not an arbitrary hex.** `var(--color-warning,#f59e0b)` is wrong when `--color-warning` is defined as `#d97706` in base.css — the fallback and the token disagree. Use `var(--color-warning)` with no fallback (the token is always defined by the time the SPA runs), or use an existing token alias like `--color-warning-soft` when a lighter variant is needed. — Why: disagreeing fallbacks produce inconsistent colors in environments where custom properties fail. — How to apply: before writing any `var(--token,#hex)`, grep base.css to confirm the token exists and check its value.
- **Driver app convention: no innerHTML in new files.** All files added under `static/driver/components/` and `static/driver/pages/` in v1.13.0 use DOM construction (`document.createElement` + `.textContent`) exclusively. SVG icons are parsed once via `DOMParser` and reused via `cloneNode(true)` (see `static/driver/components/icons.js`). Legacy `login.js` retains its static-string template (no interpolation, safe by inspection) but new code must not adopt the pattern. — Why: avoids the entire class of XSS bugs that #179 caught in the dispatch app. — How to apply: when writing any new driver-app component, use `el.textContent = value` for user-derived fields. Use `el.appendChild(svgFactory())` for icons. Reach for `innerHTML` only when the source is a hardcoded string with no template interpolation.
- **Week-window upper bounds must come from the next-period local midnight, not `lo + Duration::days(7)`.** A 168-hour UTC delta misbuckets deliveries on the two DST-transition weeks per year. Compute `hi` via `tz.from_local_datetime(next_period_start_naive)` and convert to UTC (see `parse_week_start` in `src/api/driver_portal/data.rs`). — Why: a delivery at 23:30 Saturday local on a transition week is one hour from the boundary; a 168h delta puts it in the wrong week, which silently drops it from the driver's pay-period view. — How to apply: any pay-period/billing-period/reporting window that operates in a local timezone must compute both endpoints from the local calendar, never from UTC arithmetic.
- **`pub(crate)` is the right visibility for cross-module helpers within the same crate.** When `parse_stop_time` (in `src/models/load.rs`) needed to be callable from `src/api/driver_portal/data.rs`, the fix was `pub(crate) fn`, not `pub fn`. — Why: `pub` advertises the symbol on the public API surface (visible to doc generators, breakable by external consumers); `pub(crate)` is internal-only. — How to apply: prefer `pub(crate)` for helpers shared across modules; reserve bare `pub` for items that genuinely cross the crate boundary (handlers, models exposed in OpenAPI, etc).
- **DataFusion CAST type names are lowercase — `utf8`/`string` work, `Utf8` crash-loops at migration time.** v1.13.0 shipped three `CAST(NULL AS utf8)` expressions in trip/blob migrations. LanceDB 0.27's bundled DataFusion SQL parser accepts only lowercase type names; mixed/upper case is rejected and the server fails on startup, retrying the migration forever. — Why: Arrow type names (`Utf8`, `Float64`) are how the type is spelled in Rust, but the SQL parser DataFusion ships uses lowercase keywords. Reaching for the Rust spelling produces a string that compiles fine but breaks at runtime on every existing deployment. — How to apply: any `CAST(NULL AS …)` in a LanceDB migration must use `string`, `float64`, `int64`, etc. — copy from an existing working migration in `src/db/mod.rs`, never from external docs.
- **WebAuthn registration: every `*.id` field returned by the server needs base64url→ArrayBuffer decoding, including `excludeCredentials[].id`.** v1.13.0 decoded `challenge` and `user.id` for the register-passkey path but missed every entry in `excludeCredentials`, so any driver who already had a passkey hit `Failed to read the 'id' property … not of type ArrayBuffer` on Add Passkey. — Why: the WebAuthn JS API enforces `ArrayBuffer` strictly across every credential-descriptor field; partial decoding looks correct in review but fails at runtime only for users with existing credentials, so the bug hides until release. — How to apply: when wiring any new passkey flow, walk the full options object — `challenge`, `user.id`, AND every entry in `excludeCredentials` / `allowCredentials` must each pass through `base64urlToBuffer`. Mirror the existing pattern in `static/driver/pages/login.js` for `allowCredentials`.
- **Define every `var(--token)` referenced by the stylesheet in `base.css` — undefined custom properties silently invalidate the whole shorthand.** v1.13.0 referenced `var(--space-1..6)` throughout `components.css`, but the spacing scale lived only in `docs/DESIGN.md`, never in `base.css`. Every `padding: 0 var(--space-4)` declaration was invalid and fell back to `0`, leaving the driver app-bar title flush against the viewport edge (#192). — Why: CSS does not error on unknown custom properties — it silently treats the declaration as invalid and uses the property's initial value. Visual review can miss the breakage if the surrounding layout already has its own padding. — How to apply: when introducing any `var(--name)` reference, grep `base.css` to confirm the token exists. When porting tokens from `docs/DESIGN.md`, port the whole scale at once rather than picking individual values.
- **Capture the HTTP response body on non-2xx from external services — `error_for_status()` discards the most useful diagnostic.** v1.13.0 used `reqwest::Response::error_for_status()` on Ollama 500s, leaving the operator with `status: 500 Internal Server Error` and no further information; the actual cause (vision-model context overflow) was only discoverable by hand-replaying the request. — Why: `error_for_status()` consumes the response and surfaces only the status line; the body — where the upstream service typically explains what went wrong — is thrown away. — How to apply: when calling an external HTTP service from Rust, branch on `resp.status().is_success()` and read `resp.text().await` into the error message for the failure path. Reserve `error_for_status()` for cases where you genuinely do not care which non-2xx happened.

- **Service worker `STATIC_ASSETS` precache list must stay in sync with `index.html` on every release.** Bumping `CACHE_NAME` causes the SW to install fresh and pre-cache `STATIC_ASSETS`; if the list still references deleted or renamed modules (e.g., `settings.js` after it was replaced by `account.js`), the install fails or the precache is incomplete and runtime requests fall through to network on first load. — Why: stale precache + new `CACHE_NAME` is the worst of both worlds: install succeeds but seeds outdated URLs, and the version-stamp mismatch defeats the precache entirely on first visit. — How to apply: on every release that adds/removes files under `static/driver/{pages,components,utils}/`, audit `static/driver/sw.js` `STATIC_ASSETS` in the same commit as the `CACHE_NAME` bump. Bump every `?v=` stamp in that array to the new release version.

- **Hardcoded version strings in the SPA drift on every release — fetch from `GET /version` instead.** v1.13.1 shipped with the driver Account page still showing `v1.13.0` because `static/driver/pages/account.js` carried a hand-maintained `APP_VERSION` constant disconnected from `Cargo.toml`. v1.13.2 added an unauthenticated `GET /version` endpoint returning `{"version": CARGO_PKG_VERSION}` and a small `utils/version.js` helper that fetches it once per session. — Why: every patch release will silently drift unless the version is sourced from `env!("CARGO_PKG_VERSION")` at runtime. The release checklist is too easy to forget for a non-functional constant. — How to apply: when adding any new "what version am I running" UI affordance, build it on `/version` (or extend `/version` if more build metadata is needed). Never reintroduce a hardcoded version constant in the SPA.

- **`iframe.src=` cannot carry an `Authorization` header — `fetch` + `URL.createObjectURL` is the JWT-friendly preview pattern.** The driver doc preview always returned 401 because the iframe could not authenticate (#202). The fix: `fetch()` the content with the Bearer header, build a blob URL via `URL.createObjectURL`, and assign it to an `<img>` (images) or sandboxed `<iframe>` (PDFs). Always call `URL.revokeObjectURL` on close to avoid leaks; iframes loading `blob:` URLs MUST carry `iframe.sandbox = ''`. — Why: this is the same fundamental constraint that defeated the dispatcher preview in #184; the workaround is acceptable for images (most driver uploads) but still hits Brave's blob-PDF bug, so the dispatcher fix per #184 remains separate. — How to apply: any JWT-protected binary endpoint that needs in-page preview must fetch with the header and convert to a blob URL — never set `iframe.src` directly to the API URL.

- **Appending children directly to a `.app-bar` with `justify-content: space-between` pushes the right slot to center.** v1.13.1's trip-detail header passed a Back button via `renderAppBar({ right: backBtn })`, then appended a status badge directly to the bar afterward with `appBar.appendChild(badge)`. The result: three flex children spaced apart — title left, badge center, Back right — making Back look like a centered control (#197). Always insert additional elements into the existing `.app-bar__right` slot via `appBar.querySelector('.app-bar__right')`, never directly on the bar. — Why: mixing slotted and freely-appended children in the same flex container produces ad-hoc layouts that break on small screens. — How to apply: when adding a badge or any control to an app-bar after the initial render, target the right slot and `insertBefore` to keep the trailing control (typically Back) on the far right.

- **Pages with a fixed bottom nav need a `--bottom-nav-height` token applied as `padding-bottom`.** Detail-view containers (`.trip-detail-page`, `.stop-detail-page`) with `min-height: 100vh` and no padding had their final section clipped by the fixed bottom nav (#199). Defining the height as a token in `base.css` (`--bottom-nav-height: calc(64px + env(safe-area-inset-bottom))`) lets every detail-view container reuse it AND lets the `.bottom-nav` rule reference the same token, eliminating a duplicated `calc()`. — Why: a hand-maintained inline `calc()` in two places drifts; one token is one source of truth. — How to apply: any new page-level container that should scroll independently and sits under the bottom nav must declare `padding-bottom: var(--bottom-nav-height)`.

- **Display a derived label for user-uploaded files in the SPA, not the raw filename.** Camera-captured filenames are 30+ random digits and unreadable (#201). Render `DOCTYPE — Mon DD, HH:MM AM/PM` derived from `doc.tags['doctype:']` and `doc.created_at`; keep the original filename as a `title=` tooltip. Always CSS-truncate the title with `overflow:hidden;text-overflow:ellipsis;white-space:nowrap` as defense in depth — and give sibling action buttons (e.g. `+ Upload`) `flex-shrink: 0` so they never get squeezed by a long title. — Why: future doc sources (scanner uploads, email attachments) may also produce ugly names; the doctype + capture time is always meaningful. — How to apply: any list of user-uploaded files in the SPA should render a derived label, not `doc.name`.

- **Guard awaited render functions against re-entry with a Symbol token on the container.** If `route()` fires twice (e.g. popstate during initial nav), two `renderTripDetail` calls race — each clears the container, awaits its fetches, then appends DOM. The result is duplicate content (#198). Stamp `container.__renderToken = Symbol(...)` at the start of the render, then after every `await`, bail with `if (container.__renderToken !== renderToken) return;` before mutating the DOM. — Why: SPAs with `replaceChildren()` + async fetches are vulnerable to interleaved renders; a Symbol identity check is cheaper and clearer than an abortable fetch. — How to apply: any render function that has at least one `await` between `container.replaceChildren()` and the final `appendChild` needs this guard.

## Commit Style

- Use `feat:`, `fix:`, `refactor:`, `test:`, `chore:` prefixes
- Co-author with the current model name:
  ```
  Co-Authored-By: Claude with <model-name>
  ```
