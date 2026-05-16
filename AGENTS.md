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

Releases use a **3-session model** with the PM as a checkpoint between planning and release. This keeps a human in the loop at the two highest-risk moments (scope lock and merge/tag) while letting the orchestrator run autonomously in between.

### Version increment
- **Patch (x.y.Z):** bug fixes only, no new API surface or features
- **Minor (x.Y.0):** any new feature, endpoint, or UI capability

---

### Session 1 — PM: Triage & Plan

1. **Backlog review:** `gh issue list --state open --limit 50 --json number,title,labels,body`. Group by theme, note effort/risk/dependencies, present prioritised shortlist to PM. Wait for scope confirmation before proceeding.
2. **Plan:** use `superpowers:writing-plans` skill. Save to `docs/superpowers/plans/YYYY-MM-DD-vX.Y.Z.md`.
3. **Opus plan review:** spawn Opus as a subagent to verify file paths, method signatures, error variants, and completeness. Fix BLOCK findings inline; re-run before proceeding. File non-blocking out-of-scope findings as GitHub issues ("tracked in #N").
4. **Create release branch:**
   ```bash
   git checkout main && git pull
   git checkout -b vX.Y.Z && git push -u origin vX.Y.Z
   ```
5. Commit the plan doc to the branch.
6. **Hand off to orchestrator** — output the orchestrator briefing prompt (see template below).

---

### Session 2 — Orchestrator: Implement & Review

1. Read `AGENTS.md` and the plan doc in full before spawning any subagents.
2. **Implement:** use `superpowers:subagent-driven-development` skill, task by task. After every subagent: `git diff --stat HEAD` (commit manually if skipped) and `cargo test && cargo clippy -- -D warnings`. Never proceed with a red build.
3. **Opus code review:** spawn Opus as a subagent to review key changed files. Brief: "Report PASS or BLOCK with specific file:line." Apply triage rules below. Do not open the PR until PASS.
4. **AGENTS.md lessons:** add 2–4 concise lessons (rule + why + how to apply). Commit to the release branch.
5. **Open PR:** `gh pr create --base main --head vX.Y.Z --title "..." --body "..."`. List any GitHub issues filed for non-blocking Opus findings in the PR body as "tracked in #N".
6. **Hand back to PM** — output the PM release prompt (see template below).

---

### Session 3 — PM: Verify & Release

1. Review the open PR.
2. **Verify build:** confirm `cargo test && cargo clippy -- -D warnings` are green on the branch.
3. **Close in-scope issues:** for each, leave a verification comment then `gh issue close N --reason completed`.
4. **Merge to main (ff-only):**
   ```bash
   git log --oneline main..vX.Y.Z          # must show commits
   git checkout main && git merge --ff-only vX.Y.Z && git push origin main
   git log --oneline -1 main && git log --oneline -1 vX.Y.Z   # must match
   ```
5. **Tag** (refs/tags/ prefix avoids branch/tag ambiguity):
   ```bash
   git tag vX.Y.Z && git push origin refs/tags/vX.Y.Z
   ```
6. **GitHub release:**
   ```bash
   gh release create vX.Y.Z --title "vX.Y.Z — <headline>" --target main --notes "..."
   ```
   Notes format: group by Bug Fixes / Enhancements / Infrastructure. Reference issue numbers. Include "tracked in #N" for any deferred Opus findings.

---

### Opus triage rules (applies to both plan review and code review)
- **BLOCK:** fix inline, retest, re-run Opus before proceeding
- **Non-blocking, < 30 min fix:** fix inline
- **Non-blocking, needs more work:** file a GitHub issue (Opus finding as body); reference in PR/release notes
- **Non-blocking, out of scope / needs design decision:** file a GitHub issue; do not fix this sprint; note as "tracked in #N"

### Key rules
- Parallelism lives **inside** the orchestrator session via subagents — never across sessions owning the same worktree
- Opus plan review and Opus code review are subagent calls, never separate sessions
- **Merge to main before tagging.** The tag must point to a commit on main. Tagging the branch tip instead of main leaves main stale and breaks the next release's branch point.
- **Before starting Session 1:** verify main is current: `git log --oneline -1 main` should match the previous release tag.
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

## Commit Style

- Use `feat:`, `fix:`, `refactor:`, `test:`, `chore:` prefixes
- Co-author with the current model name:
  ```
  Co-Authored-By: Claude with <model-name>
  ```
