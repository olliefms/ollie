# ollie — Agent Guide

This file is for AI coding agents working on this codebase. Read it before making changes.

## Project Overview

ollie is a self-hosted freight **Transportation Management System (TMS)** written in Rust. It manages the operational core of a trucking dispatch operation — loads, trips, drivers, trucks, trailers, and facilities — alongside an AI-enabled document store (the original "blob store": files content-addressed on disk, summarized and embedded by Ollama, indexed in LanceDB for semantic search).

The domain is exposed through four API surfaces: a dispatcher **MCP** server (`POST /dispatch/mcp`, preferred for AI agents), a dispatcher **REST** API (`/dispatch/api/v1`), a **driver portal** (`/driver/api/v1`, JWT via passkey/PIN), and a **deprecated admin REST** API (`/api/v1`, `ADMIN_API_KEY`). Two static web apps ship with it: a dispatcher SPA at `/dispatch` and a driver PWA at `/driver`. `GET /llms.txt` is the hand-written, agent-oriented tour of every surface and is the best high-level map of the running system.

**Stack:** Axum 0.8, LanceDB 0.29, Arrow 58, async-channel 2, reqwest 0.12, pdf-extract 0.7, jsonwebtoken 10, webauthn-rs 0.5, utoipa 4. Facility geocoding uses the US Census geocoder; trip/load mileage uses OpenRouteService (HGV).

## Codebase Layout

The codebase follows a per-entity convention: each domain resource has a model in `models/`, DB operations in `db/<entity>_ops.rs`, and HTTP handlers in `api/`. When adding a resource, follow the existing trio.

```
src/
  lib.rs          — module declarations + AppState (db, store, ai, geocoding, ors, pipelines, webauthn, …)
  main.rs         — startup: config, db, store, ai, geocoding/routing clients, pipelines, indices, HTTP server
  config.rs       — Config::from_env()
  error.rs        — AppError enum + IntoResponse impl
  models/         — one module per resource: blob, facility, load, trip, driver, truck, trailer,
                    dispatcher(+credentials/api_key), event, oauth_client, authorization_code, refresh_token
  storage/        — BlobStore (content-addressed sharding), extract_store
  db/             — DbClient + one *_ops.rs per resource; merge_insert upsert pattern (see below)
  ai/             — OllamaClient: embed(), generate(); extract.rs, embed.rs, summarize.rs
  geocoding/      — US Census geocoder client
  routing/        — OpenRouteService HGV routing client (deadhead/loaded miles)
  pipeline/       — spawn_pipeline (doc AI), spawn_geocoding_pipeline, spawn_routing_pipeline, recovery
  events/         — append-only event journal helpers
  services/       — trip_stops, doctors/ (trip, load, facility data-integrity repair)
  api/
    mod.rs              — router() wiring all surfaces; ApiDoc (utoipa) + LLMS_TXT
    auth.rs             — require_bearer() (admin API_KEY) middleware
    blobs.rs / blob.rs  — admin blob upload/list and per-blob get/update/delete/query
    facilities.rs, loads.rs, trips.rs, trip_actions.rs, drivers.rs, trucks.rs,
      trailers.rs, dispatchers.rs, events.rs, mileage_summary.rs, version.rs — admin REST handlers
    oauth/              — OAuth 2.1 (authorize, token, register, metadata) for MCP
    dispatcher_portal/  — JWT auth, data (read) + *_writes handlers, mcp.rs (MCP server), blobs + presigned, api_keys
    driver_portal/      — passkey/PIN auth (jwt.rs, middleware.rs), data, equipment, documents
```

## Critical Version Notes

### LanceDB + Arrow

**Do not change these versions without reading this section.**

- `lancedb = "0.29"` — lancedb 0.9 (originally in the design plan) had an arrow/chrono incompatibility and was abandoned. Bumped 0.27→0.29 in v1.19.x to drop the `tantivy`→`lru` transitive dependency flagged by Dependabot (GHSA-rhfx-m35p-ff5j); lancedb 0.29 removed `tantivy` entirely.
- `arrow-array = "58"` and `arrow-schema = "58"` — **must match the arrow version lancedb bundles internally.** lancedb 0.29 uses arrow 58; a mismatch (e.g. our crate on 57 while lancedb is on 58) puts two copies of `arrow_array` in the tree and causes `RecordBatch` / `RecordBatchReader` trait incompatibility at compile time. This was the entire content of the 0.27→0.29 migration: no lancedb API call we use changed — only the arrow pin had to move in lockstep.
- If you upgrade lancedb, check its bundled arrow version first (`cargo tree -p lancedb | grep arrow`) and bump `arrow-array`/`arrow-schema` to match in the same change.

### LanceDB API Notes

The plan was written for an older API. The actual API (0.27 through 0.29, unchanged across that bump) differs:

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

Auth depends on the surface (see `/llms.txt` for the authoritative description):

- **Dispatcher MCP/REST** (`/dispatch/*`) — `Authorization: Bearer <JWT>` from `POST /dispatch/auth/login` (email+password), or a dispatcher API key. JWTs are signed with `DISPATCHER_JWT_SECRET`.
- **Driver portal** (`/driver/api/v1/*`) — `Authorization: Bearer <JWT>` from passkey/PIN auth. JWTs are signed with `DRIVER_JWT_SECRET`.
- **Admin REST** (`/api/v1/*`, deprecated) — `Authorization: Bearer <ADMIN_API_KEY>`. The key is arbitrary — any non-empty string works (`ADMIN_API_KEY=test-key` is fine for local dev/tests).
- **Public, no auth:** `GET /version`, `GET /openapi.json`, `GET /llms.txt`.

Missing or wrong credentials → 401. All three secrets (`ADMIN_API_KEY`, `DRIVER_JWT_SECRET`, `DISPATCHER_JWT_SECRET`) are required at startup — the server refuses to boot without them.

## Deduplication Logic

On upload, the server computes SHA-256 of the file bytes:
- If checksum exists in DB: return **201 Created** (copies summary/embedding from existing record to new record)
- If checksum is new: return **202 Accepted** (queues for processing)

Both paths write to the DB. The filesystem file is only written once per unique checksum.

## Geocoding

Facility addresses are geocoded asynchronously after create or address-change update — the API returns immediately with `geocode_status: pending` and a background worker fills `lat`/`lng`/`normalized_address`. On failure, `geocode_status` transitions to `failed` (and after 3 attempts, `permanently_failed`).

**Manual override:** `POST /api/v1/facilities` and `PATCH /api/v1/facilities/{id}` both accept optional `lat` + `lng`. When both are supplied, the geocoder is skipped, coords are persisted as-is, `geocode_status` is set to `ready`, and `geocode_failure_count` resets to `0`. On UPDATE, explicit coords win even when `address` is also being changed. Partial coords or out-of-range values (lat ∉ [-90, 90], lng ∉ [-180, 180]) → 422. This is the supported repair path for facilities the geocoder can't resolve (e.g. industrial warehouses with BOL-derived addresses).

## Content Extraction

`extract_content()` in `src/ai/extract.rs` returns an `Extractable` enum:

- `Text(String)` — for `text/*`, `application/json`, `application/xml`, and PDFs with ≥50 words
- `ImageBytes(Vec<u8>)` — for `image/*` and PDFs with <50 extractable words
- `Unsupported` — everything else

PDFs use `pdf_extract::extract_text_from_mem()`. If it can't extract ≥50 words, the PDF is treated as an image and sent to the vision model.

## Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `ADMIN_API_KEY` | Yes | — | Bearer token for the deprecated admin REST API (`/api/v1`) |
| `DRIVER_JWT_SECRET` | Yes | — | Signing secret for driver-portal JWTs. Min 32 bytes |
| `DISPATCHER_JWT_SECRET` | Yes | — | Signing secret for dispatcher JWTs and API keys. Min 32 bytes |
| `DRIVER_RP_ID` | Yes | — | WebAuthn relying-party ID for driver passkeys (e.g. `localhost`) |
| `DRIVER_RP_ORIGIN` | Yes | — | WebAuthn relying-party origin (e.g. `http://localhost:3000`) |
| `ORS_API_KEY` | No | `` (empty) | OpenRouteService key for trip/load mileage. Empty disables mileage calc |
| `PORT` | No | `3000` | HTTP listen port |
| `DATA_DIR` | No | `./data` | Root for blob storage and LanceDB |
| `GEOCODING_WORKERS` | No | `1` | Concurrent facility-geocoding workers |
| `OLLAMA_BASE_URL` | No | `http://localhost:11434` | Ollama API base URL |
| `OLLAMA_EMBED_MODEL` | No | `nomic-embed-text` | Embedding model |
| `OLLAMA_SUMMARY_MODEL` | No | `llama3.2` | Text summarization model |
| `OLLAMA_VISION_MODEL` | No | `moondream` | Vision/image description model |
| `OLLAMA_EMBED_DIM` | No | `768` | Embedding dimension (must match model) |
| `PIPELINE_WORKERS` | No | `1` | Concurrent pipeline workers |
| `TERMINAL_TIMEZONE` | No (deprecated) | America/New_York | **Seed-only.** First-boot timezone for the Default terminal row. No longer read by `Config`; terminals own timezone (#185). |
| `OLLIE_FREE_DWELL_MINUTES` | No (deprecated) | `120` | **Seed-only.** First-boot free-dwell for the Default terminal row. No longer read by `Config`; terminals own free-dwell (#185, #258). |
| `OLLIE_PUBLIC_BASE_URL` | No | `` (empty) | Externally-reachable base URL (no trailing slash), e.g. `https://ollie.example.com`. Used to build absolute presigned blob upload/download URLs for the dispatcher MCP blob tools. When empty, the presigned-URL tools error; inline `create_blob` still works (#277). |
| `OLLIE_MCP_INLINE_BLOB_MAX_BYTES` | No | `262144` | Max decoded size for the inline-base64 `create_blob` MCP tool. Larger files must use a presigned upload URL. Also sizes the `/dispatch/mcp` request body limit (#277). |
| `OLLIE_BLOB_PRESIGN_TTL_SECS` | No | `300` | Default TTL (seconds) for presigned blob URLs when the caller omits `expires_in_seconds` (#277). |
| `OLLIE_BLOB_PRESIGN_MAX_TTL_SECS` | No | `3600` | Hard cap (seconds) on presigned blob URL TTL (#277). |

### Terminals & driver pay

The `terminals` table owns each terminal's timezone and the mandatory rate floor
(loaded/deadhead rates, extra-stop fee, detention rate, free-dwell minutes). Drivers
attach to a terminal via `terminal_id` and may carry optional per-field rate overrides;
trips may carry the same overrides. Pay resolves per-field trip → driver → terminal
(`resolve_rates` in `src/models/pay.rs`). The `TERMINAL_TIMEZONE` and
`OLLIE_FREE_DWELL_MINUTES` env vars are now ONLY first-boot seed values for the Default
terminal (read directly in `open_or_create_terminal`); they are otherwise deprecated and
no longer read by `Config`.

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

**Skill source of truth:** The project-local copies of `work-issue`, `sprint-plan`, and `cut-release` live in `.claude/skills/` and are the authoritative versions for this repo. When invoking any of these skills, always use the project-local version, never the globally installed one of the same name. The project copies are tailored to Ollie's conventions (trunk-based releases, Shippability Bar, triage rules above) and may diverge from the global skills over time.

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
- **Stop scheduled times must be naive datetime strings when a timezone is stored.** Store as `"2026-05-10T08:00:00"` (no Z, no offset), not UTC-normalized. A UTC-normalized string paired with a timezone field produces inconsistent data: `parse_stop_time` will misread the offset. Reject any string with a trailing `Z` or `+HH:MM` when `timezone` is `Some`. — Same rule applies to `actual_arrive` and `actual_depart`: when the stop has a `timezone` set, new writes with a Z/offset suffix get 422 via `validate_stop_time_str`. Existing UTC-suffixed rows with `timezone: null` stay readable (backwards-compat — confirmed scope decision; no rewrite). Responses include `actual_arrive_utc` / `actual_depart_utc` (RFC 3339 UTC) derived from naive + tz, or from the embedded Z for legacy rows. These UTC companion fields are response-only — never persisted.
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
- **Existing-DB migration safety net lives in `tests/migration_test.rs`.** The test seeds a pre-v1.16.0 trips table (current schema minus the v1.16.0-added columns), populates one row, then opens the DB with current `DbClient::new` and asserts the migration completes and the new columns round-trip via `insert_trip` / `get_trip`. Any future migration that adds a column must extend this test: drop the new column from the seed schema and add an assertion that a fresh post-migration row carrying the new column round-trips through the ops layer. Without this, CAST-type regressions and other migration bugs only surface in production. Documented under the "recurring AI-agent failure" lesson.
- **SPA `goBack()` must not call `navigate()` — use a shared render function instead.** `navigate()` pushes to history before rendering. If `goBack()` calls `navigate()`, pressing Back twice re-adds the current view to history, creating an infinite loop. Extract the render-and-update logic into `_renderView(view, params)` and have both `navigate()` (which pushes first) and `goBack()` (which pops first) call `_renderView` directly.
- **KPI count endpoints on LanceDB should access `state.db.*_table.count_rows()` directly rather than going through list helpers.** The list helpers fetch + deserialize rows just to return the total, which is wasteful for a pure count. LanceDB's `count_rows(filter)` is a cheap metadata operation. Add count methods or call the table directly in the handler.
- **Sort by `created_at DESC` happens after `collect_stream`, not in the query.** LanceDB (0.27–0.29) does not have reliable `order_by` support. Follow the trip_ops pattern: call `batches_to_*`, then `records.sort_by_key(|r| std::cmp::Reverse(r.created_at))`, then skip/map. Do not rely on insertion order for user-facing list views.
- **LanceDB migration CAST expressions require DataFusion *SQL* type names, not Arrow type names.** The bundled SQL parser accepts `string`, `double`, `int64`, `bigint` — and rejects every Arrow spelling (`Utf8`, `utf8`, `Float64`, `float64`, `Double`). Applies to all `add_columns(NewColumnTransform::SqlExpressions(...))` calls in migration helpers. (Earlier versions of this lesson incorrectly listed `utf8`/`float64` as acceptable — they are not, and shipped releases v1.13.0 and v1.16.0 each crash-looped on this exact mistake. See the "recurring AI-agent failure" lesson below.)
- **Removing a DB-level `.limit()` from a list query requires adding `.take(limit)` to the iterator.** When you remove `.limit(limit + offset)` from a LanceDB query (e.g. to fix a sort-window bug), the in-memory iterator no longer has a page cap. Always add `.skip(offset).take(limit)` to the iterator after `sort_by_key` — without `.take(limit)`, the endpoint returns all remaining rows.
- **CSS class attribute injection needs the same allowlist as inner-text injection.** Before interpolating any API-derived value into a CSS class string (e.g. `badge--${entityType}`), apply `.replace(/[^a-z0-9_]/g, '_')` — the same pattern used in the `badge()` utility. A raw `entity_type` containing `"` or whitespace becomes an XSS vector via class attribute breakout.
- **Driver PWA: `position: fixed; height: 100dvh` is the correct scroll-containment pattern for full-viewport list views.** `height: 100vh` with a flex parent using `min-height` leaks scroll to the body. Use `position: fixed; top: 0; left: 0; width: 100%; height: 100dvh` to take the full viewport; `100dvh` avoids the iOS Safari browser-chrome clip. Other views that use body scroll are unaffected.
- **`enrich_trip` and similar enrichment helpers must be synchronous and accept pre-fetched maps, not call the DB per item.** An async `enrich_trip` that fires `get_driver`, `get_truck`, and `get_trailer` per trip is O(trips × entities) DB roundtrips. The correct pattern: collect all unique IDs in one pass, call `tokio::try_join!(batch_get_drivers, batch_get_trucks, batch_get_trailers)`, build `HashMap<Uuid, String>` name maps, then map synchronously. Apply this pattern to any list enrichment function. — Why: 20 trips × 3 trailers = 60 DB queries collapsed to 3. — How to apply: whenever you see `join_all` over individual `get_*` calls in a list handler, refactor to batch.
- **`batch_get_*` methods belong on `DbClient`, not in handlers.** Any entity type that appears in list views needs a `batch_get_*` method in its `*_ops.rs` file. Pattern: early-return empty map on empty input; build `id IN (...)` filter string; call `.only_if(filter)` (not `.filter()`); return `HashMap<Uuid, Record>`. — Why: consistency and reuse — handlers and tests can rely on a single well-tested batch path. — How to apply: add `batch_get_*` when writing any new entity type, not only when an N+1 regression is discovered.
- **A scan cap with in-memory sort diverges from `count_rows` total at deep offsets.** `list_loads` caps the LanceDB scan at 2000 rows then paginates in memory, but `count_rows` returns the full unfiltered total. A client requesting offset > 2000 gets an empty page while the total still shows more records. Document any scan cap with a constant and comment; if load volume grows, raise the cap or switch to cursor pagination. — Why: silent empty pages are confusing and hard to debug. — How to apply: always pair a scan cap with a comment explaining the divergence; add a UI-side offset clamp when building pagination UI.
- **Apply `escHtml()` to ALL fields that flow from API responses into innerHTML, not just the ones that seem dangerous.** The v1.11.0 review found pre-existing unescaped `load_number` and `customer_name` in the load detail view. When adding `escHtml` to one part of a component, grep for all adjacent interpolations and escape them in the same commit. — Why: partial escaping leaves XSS surface area that reviewers and linters won't catch. — How to apply: after any `escHtml` fix, grep for `\${` patterns in the same template and verify each one is wrapped.
- **Document preview must MIME-branch, not sandbox-everything-in-one-iframe.** v1.14.x doc preview originally used a single `<iframe sandbox="">` for every previewable MIME. Empirical Playwright testing in v1.14.x (#184) showed Chrome's PDF viewer fails under EVERY sandbox value — `''`, `'allow-same-origin'`, `'allow-scripts'`, and combinations all break blob-PDF rendering. The fix is to branch by type and use the right element per type: `application/pdf` → `<iframe>` with no sandbox attribute (PDFs cannot execute scripts, so origin inheritance is harmless); `image/*` → `<img>` (no iframe at all); `text/plain` → `<pre>` with `.textContent` (XSS-safe by definition); `text/html` → drop from preview, force download (the only previewable type that can execute scripts). This supersedes the earlier "always carry `sandbox=''`" rule, which assumed sandbox was free — it isn't, it breaks the PDF viewer. — Why: any single-iframe approach picks between "broken PDFs" and "XSS surface"; MIME-branching gets both right. — How to apply: in any new doc preview path, switch on MIME at the top and choose the element; reserve sandboxed iframes for types that can execute scripts AND that we actually want to render inline (currently: none).
- **`history.replaceState` (not `pushState`) belongs inside `_renderView`; `pushState` belongs in `navigate`.** `navigate()` records history and calls `_renderView`. If `_renderView` used `pushState`, every internal re-render would double-push, bloating history and breaking Back. Use `replaceState` inside `_renderView` to mirror state without adding history entries. — Why: the distinction prevents the double-push trap when the same view is re-rendered. — How to apply: any time you mirror router state into the URL inside a render helper, use `replaceState`.
- **CSS design token fallbacks inside `var()` must use the same token from `base.css`, not an arbitrary hex.** `var(--color-warning,#f59e0b)` is wrong when `--color-warning` is defined as `#d97706` in base.css — the fallback and the token disagree. Use `var(--color-warning)` with no fallback (the token is always defined by the time the SPA runs), or use an existing token alias like `--color-warning-soft` when a lighter variant is needed. — Why: disagreeing fallbacks produce inconsistent colors in environments where custom properties fail. — How to apply: before writing any `var(--token,#hex)`, grep base.css to confirm the token exists and check its value.
- **Driver app convention: no innerHTML in new files.** All files added under `static/driver/components/` and `static/driver/pages/` in v1.13.0 use DOM construction (`document.createElement` + `.textContent`) exclusively. SVG icons are parsed once via `DOMParser` and reused via `cloneNode(true)` (see `static/driver/components/icons.js`). Legacy `login.js` retains its static-string template (no interpolation, safe by inspection) but new code must not adopt the pattern. — Why: avoids the entire class of XSS bugs that #179 caught in the dispatch app. — How to apply: when writing any new driver-app component, use `el.textContent = value` for user-derived fields. Use `el.appendChild(svgFactory())` for icons. Reach for `innerHTML` only when the source is a hardcoded string with no template interpolation.
- **Week-window upper bounds must come from the next-period local midnight, not `lo + Duration::days(7)`.** A 168-hour UTC delta misbuckets deliveries on the two DST-transition weeks per year. Compute `hi` via `tz.from_local_datetime(next_period_start_naive)` and convert to UTC (see `parse_week_start` in `src/api/driver_portal/data.rs`). — Why: a delivery at 23:30 Saturday local on a transition week is one hour from the boundary; a 168h delta puts it in the wrong week, which silently drops it from the driver's pay-period view. — How to apply: any pay-period/billing-period/reporting window that operates in a local timezone must compute both endpoints from the local calendar, never from UTC arithmetic.
- **`pub(crate)` is the right visibility for cross-module helpers within the same crate.** When `parse_stop_time` (in `src/models/load.rs`) needed to be callable from `src/api/driver_portal/data.rs`, the fix was `pub(crate) fn`, not `pub fn`. — Why: `pub` advertises the symbol on the public API surface (visible to doc generators, breakable by external consumers); `pub(crate)` is internal-only. — How to apply: prefer `pub(crate)` for helpers shared across modules; reserve bare `pub` for items that genuinely cross the crate boundary (handlers, models exposed in OpenAPI, etc).
- **DataFusion CAST type names are SQL keywords, not Arrow types — `string`/`double` work, `utf8`/`float64`/`Utf8`/`Float64` crash-loop at migration time.** This is the canonical list of acceptable types in a LanceDB migration CAST: `string`, `double`, `int`/`bigint`, `boolean`, `date`, `timestamp`. Anything spelled like an Arrow `DataType` variant (`Utf8`, `Float64`, `Int64`, `utf8`, `float64`) compiles fine and is rejected at runtime — the server fails on startup and retries the migration forever. Three releases have shipped this same bug: v1.10.0 (`Utf8`→`utf8`, #02ee4ff), v1.13.0 (`utf8`→`string`, #4f367b3 / #191), v1.16.0 (`float64`→`double`, #f1ced74). — Why: Arrow's Rust enum (`DataType::Utf8`, `DataType::Float64`) is the natural mental model when writing Rust against `arrow_schema`; the bundled DataFusion SQL parser does not accept those spellings, only SQL keywords. The trap is reinforced every time the migration code is read, because the surrounding Rust references `DataType::Utf8` constants that look identical. — How to apply: any `CAST(NULL AS …)` in a LanceDB migration must use the SQL keyword, never the Arrow name. Copy from an existing working migration in `src/db/mod.rs`. Treat any `CAST(NULL AS Utf8|utf8|Float64|float64|Double)` in a diff as a crash — reject it on review even if the code "looks fine" against the surrounding `DataType::*` constants.
- **WebAuthn registration: every `*.id` field returned by the server needs base64url→ArrayBuffer decoding, including `excludeCredentials[].id`.** v1.13.0 decoded `challenge` and `user.id` for the register-passkey path but missed every entry in `excludeCredentials`, so any driver who already had a passkey hit `Failed to read the 'id' property … not of type ArrayBuffer` on Add Passkey. — Why: the WebAuthn JS API enforces `ArrayBuffer` strictly across every credential-descriptor field; partial decoding looks correct in review but fails at runtime only for users with existing credentials, so the bug hides until release. — How to apply: when wiring any new passkey flow, walk the full options object — `challenge`, `user.id`, AND every entry in `excludeCredentials` / `allowCredentials` must each pass through `base64urlToBuffer`. Mirror the existing pattern in `static/driver/pages/login.js` for `allowCredentials`.
- **Define every `var(--token)` referenced by the stylesheet in `base.css` — undefined custom properties silently invalidate the whole shorthand.** v1.13.0 referenced `var(--space-1..6)` throughout `components.css`, but the spacing scale lived only in `docs/DESIGN.md`, never in `base.css`. Every `padding: 0 var(--space-4)` declaration was invalid and fell back to `0`, leaving the driver app-bar title flush against the viewport edge (#192). — Why: CSS does not error on unknown custom properties — it silently treats the declaration as invalid and uses the property's initial value. Visual review can miss the breakage if the surrounding layout already has its own padding. — How to apply: when introducing any `var(--name)` reference, grep `base.css` to confirm the token exists. When porting tokens from `docs/DESIGN.md`, port the whole scale at once rather than picking individual values.
- **Capture the HTTP response body on non-2xx from external services — `error_for_status()` discards the most useful diagnostic.** v1.13.0 used `reqwest::Response::error_for_status()` on Ollama 500s, leaving the operator with `status: 500 Internal Server Error` and no further information; the actual cause (vision-model context overflow) was only discoverable by hand-replaying the request. — Why: `error_for_status()` consumes the response and surfaces only the status line; the body — where the upstream service typically explains what went wrong — is thrown away. — How to apply: when calling an external HTTP service from Rust, branch on `resp.status().is_success()` and read `resp.text().await` into the error message for the failure path. Reserve `error_for_status()` for cases where you genuinely do not care which non-2xx happened.

- **Service worker `STATIC_ASSETS` precache list must stay in sync with `index.html` on every release.** Bumping `CACHE_NAME` causes the SW to install fresh and pre-cache `STATIC_ASSETS`; if the list still references deleted or renamed modules (e.g., `settings.js` after it was replaced by `account.js`), the install fails or the precache is incomplete and runtime requests fall through to network on first load. — Why: stale precache + new `CACHE_NAME` is the worst of both worlds: install succeeds but seeds outdated URLs, and the version-stamp mismatch defeats the precache entirely on first visit. — How to apply: on every release that adds/removes files under `static/driver/{pages,components,utils}/`, audit `static/driver/sw.js` `STATIC_ASSETS` in the same commit as the `CACHE_NAME` bump. Bump every `?v=` stamp in that array to the new release version.

- **Hardcoded version strings in the SPA drift on every release — fetch from `GET /version` instead.** v1.13.1 shipped with the driver Account page still showing `v1.13.0` because `static/driver/pages/account.js` carried a hand-maintained `APP_VERSION` constant disconnected from `Cargo.toml`. v1.13.2 added an unauthenticated `GET /version` endpoint returning `{"version": CARGO_PKG_VERSION}` and a small `utils/version.js` helper that fetches it once per session. — Why: every patch release will silently drift unless the version is sourced from `env!("CARGO_PKG_VERSION")` at runtime. The release checklist is too easy to forget for a non-functional constant. — How to apply: when adding any new "what version am I running" UI affordance, build it on `/version` (or extend `/version` if more build metadata is needed). Never reintroduce a hardcoded version constant in the SPA.

- **`iframe.src=` cannot carry an `Authorization` header — `fetch` + `URL.createObjectURL` is the JWT-friendly preview pattern.** The driver doc preview always returned 401 because the iframe could not authenticate (#202). The fix: `fetch()` the content with the Bearer header, build a blob URL via `URL.createObjectURL`, and assign it to an `<img>` (images) or sandboxed `<iframe>` (PDFs). Always call `URL.revokeObjectURL` on close to avoid leaks. (Earlier guidance said iframes loading `blob:` URLs must carry `iframe.sandbox = ''`; this was superseded in v1.14.x — see the MIME-branch lesson above. Sandboxing the iframe breaks PDF rendering in current Chrome.) — Why: this is the same fundamental constraint that defeated the dispatcher preview in #184; Brave's blob-PDF bug is a separate problem still tracked there. — How to apply: any JWT-protected binary endpoint that needs in-page preview must fetch with the header and convert to a blob URL — never set `iframe.src` directly to the API URL.

- **Appending children directly to a `.app-bar` with `justify-content: space-between` pushes the right slot to center.** v1.13.1's trip-detail header passed a Back button via `renderAppBar({ right: backBtn })`, then appended a status badge directly to the bar afterward with `appBar.appendChild(badge)`. The result: three flex children spaced apart — title left, badge center, Back right — making Back look like a centered control (#197). Always insert additional elements into the existing `.app-bar__right` slot via `appBar.querySelector('.app-bar__right')`, never directly on the bar. — Why: mixing slotted and freely-appended children in the same flex container produces ad-hoc layouts that break on small screens. — How to apply: when adding a badge or any control to an app-bar after the initial render, target the right slot and `insertBefore` to keep the trailing control (typically Back) on the far right.

- **Pages with a fixed bottom nav need a `--bottom-nav-height` token applied as `padding-bottom`.** Detail-view containers (`.trip-detail-page`, `.stop-detail-page`) with `min-height: 100vh` and no padding had their final section clipped by the fixed bottom nav (#199). Defining the height as a token in `base.css` (`--bottom-nav-height: calc(64px + env(safe-area-inset-bottom))`) lets every detail-view container reuse it AND lets the `.bottom-nav` rule reference the same token, eliminating a duplicated `calc()`. — Why: a hand-maintained inline `calc()` in two places drifts; one token is one source of truth. — How to apply: any new page-level container that should scroll independently and sits under the bottom nav must declare `padding-bottom: var(--bottom-nav-height)`.

- **Display a derived label for user-uploaded files in the SPA, not the raw filename.** Camera-captured filenames are 30+ random digits and unreadable (#201). Render `DOCTYPE — Mon DD, HH:MM AM/PM` derived from `doc.tags['doctype:']` and `doc.created_at`; keep the original filename as a `title=` tooltip. Always CSS-truncate the title with `overflow:hidden;text-overflow:ellipsis;white-space:nowrap` as defense in depth — and give sibling action buttons (e.g. `+ Upload`) `flex-shrink: 0` so they never get squeezed by a long title. — Why: future doc sources (scanner uploads, email attachments) may also produce ugly names; the doctype + capture time is always meaningful. — How to apply: any list of user-uploaded files in the SPA should render a derived label, not `doc.name`.

- **Guard awaited render functions against re-entry with a Symbol token on the container.** If `route()` fires twice (e.g. popstate during initial nav), two `renderTripDetail` calls race — each clears the container, awaits its fetches, then appends DOM. The result is duplicate content (#198). Stamp `container.__renderToken = Symbol(...)` at the start of the render, then after every `await`, bail with `if (container.__renderToken !== renderToken) return;` before mutating the DOM. — Why: SPAs with `replaceChildren()` + async fetches are vulnerable to interleaved renders; a Symbol identity check is cheaper and clearer than an abortable fetch. — How to apply: any render function that has at least one `await` between `container.replaceChildren()` and the final `appendChild` needs this guard.

- **Verify iframe / sandbox / blob URL changes empirically in a real browser — issue bodies and prior research routinely make confident claims that are wrong.** #184 went through two written analyses (the original issue body and a 2026-05-17 research comment) that confidently prescribed `sandbox='allow-same-origin'` as the fix. A five-minute Playwright MCP test in current Chrome falsified both — every sandbox value fails. The empirical check fits in one short HTML page (iframe variants pointing at a blob PDF URL, screenshot), can be served via `python3 -m http.server` from /tmp, and takes one Playwright navigation. — Why: browser sandboxing + plugin behavior changes between releases and across vendors; static reasoning from MDN or vendor docs is high-confidence-but-wrong far more often than other web APIs. — How to apply: any PR that touches `iframe.sandbox`, `URL.createObjectURL`, or the interaction between them must include a Playwright MCP run that visually confirms the target browser renders the content. Don't trust the issue body, don't trust prior comments, don't trust the previous code — run the test.

- **Recurring AI-agent failure: writing Arrow type names where DataFusion SQL types are required.** This codebase has now shipped the same bug in three separate releases (v1.10.0, v1.13.0, v1.16.0), each authored by a different AI-agent session, each catching a different one of the Arrow spellings (`Utf8` → `utf8` → `float64`). The pattern is robust: an agent writing migration code reads `arrow_schema::DataType::Float64` in the surrounding Rust, mentally pattern-matches it to "this field is a 64-bit float", and writes `CAST(NULL AS float64)` in the SQL string. It compiles. Tests pass on a fresh DB because no migration runs. Production crash-loops on startup because the SQL parser doesn't speak Arrow. The most recent occurrence (v1.16.0, #f1ced74) cost ~hours of production downtime and required a panic-mode rollback to v1.15.0. — Why: the field type (`Option<f64>`) in the Rust struct, the Arrow `DataType` constant nearby, the column's Arrow representation in LanceDB, and the SQL CAST string look like they should all use the same name — but only the last one is parsed by a different parser with a different vocabulary. The cognitive frame "Arrow type" carries through every other reference without breaking, then silently breaks on the SQL boundary. — How to apply: (1) when writing or reviewing any `CAST(NULL AS …)` in `src/db/mod.rs`, stop and check it against the SQL-keyword list (`string`, `double`, `bigint`, `boolean`, `date`, `timestamp`) — never against the Arrow `DataType` enum. (2) Treat any diff that adds a new migration CAST as a high-risk change that needs an existing-DB integration test, not only a fresh-DB unit test. (3) If you find yourself reaching for an Arrow-style name in a SQL string anywhere in the codebase, treat it as a signal to re-read this lesson before committing. — Meta: this is a recurring AI-agent failure mode, not a one-off mistake. The fact that three independent sessions have reproduced it confirms the trap is in the surrounding code, not the agent — adjust the code (or the review checklist) accordingly.

## Commit Style

- Use `feat:`, `fix:`, `refactor:`, `test:`, `chore:` prefixes
- Co-author with the current model name:
  ```
  Co-Authored-By: Claude with <model-name>
  ```
