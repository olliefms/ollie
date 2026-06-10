# Fleet UI CRUD — Phase 5 (Trips) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Trips fully read/write in the fleet SPA — migrate the **last** legacy inline list/detail out of `app.js` into `pages/` modules, add a `trip-form.js` (load-linked **and** free-standing creation, inheritable rate overrides on edit) and a `trip-detail.js` with the full lifecycle (assign/unassign, dispatch/undispatch, complete, cancel, stop arrive/depart/late, check-call, recalculate-miles, delete). This is the final phase: after Task 7 `app.js` holds **zero** inline entity views.

**Architecture:** Mirror the Phase 1–4 module pattern (`pages/<entity>{,-detail,-form}.js`, router + `VIEW_PATHS` + `app.js` case wiring, scope-gated controls, no optimistic UI). The form's bug-prone payload logic (create vs free-standing, the inheritable-override "send `null` to clear / omit to keep / value to set" rule) is extracted into a **pure, unit-tested** `pages/trip-form-payload.js` so it is Vitest-covered with no DOM. The form UI imports it.

**One backend change (decided with the user):** fully implement the spec's "override revert" for trip rates. Today `PatchTripBody`'s five rate fields are plain `Option<f64>`/`Option<u32>` (omitted = no change; **no way to clear back to inherited**). Task 1 switches them — and the `update_trip_rate_overrides` DB method — to the **`double_option`** pattern already used in `driver_ops.rs`/`models/driver.rs`: omitted = no change, explicit `null` = clear to inherited, value = set. Backward-compatible with the MCP `update_trip` tool that shares `apply_trip_patch`.

**Tech Stack:** Vanilla ES modules (`static/fleet/`), existing `components/{form,table,scope-gate,confirm}.js` + `utils/{api,dom}.js`, Vitest + happy-dom for units, Playwright for E2E. Rust backend touched **only** in Task 1.

---

## Backend reality (verified — plan against THIS, not the spec's assumptions)

- **Routes** (`src/api/fleet_portal/mod.rs`, registered in `data.rs`/`trip_writes.rs`), all under `API_BASE = /fleet/api/v1`:
  `GET /trips` (`trips:read`) · `POST /trips` (`trips:write`) · `GET /trips/{id}` (`trips:read`) · `PATCH /trips/{id}` (`trips:write`) · `DELETE /trips/{id}` (`trips:delete`) · `POST /trips/{id}/recalculate-miles` (`trips:write`) · `POST /trips/{id}/assign` · `POST /trips/{id}/unassign` · `POST /trips/{id}/dispatch` · `POST /trips/{id}/undispatch` · `POST /trips/{id}/cancel` · `POST /trips/{id}/complete` · `POST /trips/{id}/stops/{seq}/arrive` · `POST /trips/{id}/stops/{seq}/depart` · `POST /trips/{id}/stops/{seq}/late` · `POST /trips/{id}/check-call` (all lifecycle = `trips:write`).
- **`CreateTripRequest`** (`src/models/trip.rs:234`) — **NO `deny_unknown_fields`**: `trip_number?`, `load_id?: Uuid` (None ⇒ free-standing), `sequence?: u32` (default 0), `driver_id?`, `truck_id?`, `trailer_ids: Vec<Uuid>` (default `[]`), `stops: Vec<TripStop>` (default `[]`), `notes?`, `previous_trip_id?` (omit ⇒ auto-resolves from driver's last trip), `blob_ids: Vec<Uuid>` (default `[]`).
- **Free-standing vs load-linked** (`src/api/trips.rs`): load-linked (`load_id: Some`) with **empty `stops`** derives stops from the load (copies stop_type/facility_id/schedule/dwell/detention/notes/timezone, sets `load_stop_index`) and denormalizes `load_number`. Free-standing (**`load_id: None`**) **requires explicit `stops`** — none are derived.
- **`PatchTripBody`** (`src/api/fleet_portal/trip_writes.rs:49`) — **HAS `deny_unknown_fields`**: `notes?`, `previous_trip_id?` (set only; no clear), `blob_ids?: Vec<Uuid>` (replaces list), `settlement_ref?`, `pay_period_start?`, `pay_period_end?`, and the five rate overrides `loaded_rate_per_mile?`, `deadhead_rate_per_mile?`, `extra_stop_fee?`, `detention_rate_per_hour?`, `free_dwell_minutes?`. **Stops, driver, truck, trailers are NOT patchable here** — they change via the lifecycle endpoints. Raw mileage fields (`deadhead_miles`/`loaded_miles`/`total_miles`/`segment_miles`) are explicitly 400'd.
- **Settlement freeze** (`trip_writes.rs:180-209`): once `settlement_ref` is set, pay-affecting edits (rate overrides, `previous_trip_id`) return **409** (`"trip is settled; pay-affecting fields are frozen"`); re-settling 409s; empty `settlement_ref` → 422.
- **Status enum + transitions** (`src/models/trip.rs:119-155`): `planned→{assigned,cancelled}`, `assigned→{planned,dispatched,cancelled}`, `dispatched→{assigned,in_transit,cancelled}`, `in_transit→delivered`, `delivered→completed`, `completed`/`cancelled` terminal.
- **Lifecycle bodies** (`src/services/trip_lifecycle.rs:17-46`): assign `{ driver_id, truck_id, trailer_ids }`; unassign/dispatch/undispatch/cancel **no body**; complete **no body** (→ 204); arrive `{ actual_arrive: String }`; depart `{ actual_depart: String }`; late `{ eta?, notes? }` (→ 204); check-call `{ location, notes?, eta_next_stop? }` (→ 204). Arrive/depart actuals are **naive local** strings (same `YYYY-MM-DDTHH:MM:SS` convention as load stop times).
- **`GET /trips/{id}`** returns `FleetTripListItem` (`data.rs:36`): `id`, `trip_number`, `status`, `driver_id/driver_name`, `truck_id/truck_unit`, `trailer_ids/trailer_units`, `load_id/load_number`, `stops: Vec<TripStop>`, flat `deadhead/loaded/total_miles`, `mileage_summary?` (origin + legs), `driver_pay?` (live or frozen snapshot), `notes`, `created_at/updated_at`, `origin_facility_name`.
- **DELETE = soft-then-hard** (`src/db/trip_ops.rs:179-198`): `planned|assigned|dispatched` → soft-cancel (status `cancelled`); already `cancelled` → physical delete; `in_transit|delivered|completed` → **409** `"cannot cancel trip with status '{status}'"`. So Delete is a single guarded action (like Loads), **not** the two-tier soft/reactivate UI.
- **Rate-override inheritance:** trip override → driver override → terminal floor. Trip values that are `None` are *inherited* (shown as ghost). Clearing-to-inherited is what Task 1 adds.

## Files

- **Modify (backend, Task 1):** `src/api/fleet_portal/trip_writes.rs` (`PatchTripBody` 5 rate fields → `double_option`; `touches_rate` + the apply call), `src/db/trip_ops.rs` (`update_trip_rate_overrides` → `Option<Option<…>>`), plus its other caller(s) if any. Add tests in the existing fleet-portal trip test module.
- **Create** `static/fleet/pages/trip-form-payload.js` — pure helpers: `buildCreateTripPayload(state)`, `buildTripPatch(state)` (override clear/keep/set), `tripStopTypes()`, `serviceTypesFor`-equivalent if stops are editable, `toNaiveDateTime` (reuse from load helper — import, don't duplicate).
- **Create** `static/fleet/pages/trip-form.js` — create/edit form UI; exports `renderTripForm(id|null)`.
- **Create** `static/fleet/pages/trips.js` — list page; exports `renderTripsView(params)`.
- **Create** `static/fleet/pages/trip-detail.js` — detail page + lifecycle actions; exports `renderTripDetail(id)`.
- **Create** `tests/fleet/trip-form-payload.test.js` — Vitest units for the pure helpers.
- **Modify** `static/fleet/router.js` — add `trip-new` (BEFORE `trip-detail`) + `trip-edit`.
- **Modify** `static/fleet/utils/dom.js` — add `trip-new` / `trip-edit` to `VIEW_PATHS`.
- **Modify** `static/fleet/app.js` — imports + `VIEW_TITLES` + `case` dispatch; **remove** inline `renderTripsView` + `renderTripDetailView` (lines ~158–314) and route to the new pages.

> Naming caution: the `trip-new` route regex MUST be registered before `trip-detail` (`/^\/fleet\/trips\/([^/]+)$/`), exactly as `load-new` precedes `load-detail`, or `/trips/new` matches the detail route.

---

## Task 1: Backend — `double_option` clear support for trip rate overrides (TDD, Rust)

**Files:**
- Modify: `src/api/fleet_portal/trip_writes.rs`
- Modify: `src/db/trip_ops.rs`
- Test: the existing fleet-portal trip write test module (find it: `grep -rn "patch_trip\|apply_trip_patch\|update_trip_rate_overrides" tests/ src/` — co-locate new tests with the current ones).

- [ ] **Step 1: Write failing tests.** Add cases asserting, via `apply_trip_patch` (or the HTTP handler in an integration test, matching the existing style):
  1. PATCH `{ "loaded_rate_per_mile": 2.5 }` **sets** the override to `Some(2.5)`.
  2. PATCH `{ "loaded_rate_per_mile": null }` **clears** it to `None` (inherited) on a trip that previously had `Some(...)`.
  3. PATCH `{}` (field omitted) leaves the existing override **unchanged**.
  4. Settlement freeze still 409s when a rate field is present as `null` **or** a value (clearing is pay-affecting too) — assert `touches_rate` treats `Some(None)` and `Some(Some(_))` alike.
  Run: `cargo test -p <crate> trip_rate_override` → FAIL.

- [ ] **Step 2: Switch `PatchTripBody` to `double_option`.** Use the same import the driver model uses (`grep -n "double_option" src/models/driver.rs src/models/mod.rs` for the helper path). For each of the five rate fields:
  ```rust
  #[serde(default, with = "crate::models::<the_double_option_mod>")]
  pub loaded_rate_per_mile: Option<Option<f64>>,
  // …deadhead_rate_per_mile, extra_stop_fee, detention_rate_per_hour: Option<Option<f64>>
  // …free_dwell_minutes: Option<Option<u32>>
  ```
  Keep `deny_unknown_fields`. Note the existing `update_trip_metadata`/`previous_trip_id`/settlement code is untouched.

- [ ] **Step 3: Update `touches_rate` + the apply call** in `apply_trip_patch`. `touches_rate` becomes `parsed.loaded_rate_per_mile.is_some() || …` (outer `Some` = "this field is being changed", covering both clear and set). Pass the `Option<Option<…>>` values straight through to the DB method.

- [ ] **Step 4: Update `update_trip_rate_overrides`** (`src/db/trip_ops.rs:226`) to take `Option<Option<f64>>` (×4) + `Option<Option<u32>>`, applying:
  ```rust
  if let Some(v) = loaded { t.loaded_rate_per_mile = v; }   // Some(None) clears, Some(Some) sets
  // …repeat for the other four
  ```
  Update **every** caller (the HTTP/MCP path via `apply_trip_patch`, plus any other — `grep -rn "update_trip_rate_overrides" src/`). For non-patch callers that previously passed `Some(x)`/`None`, wrap as `Some(Some(x))`/`None`.

- [ ] **Step 5:** `cargo test` (targeted then full) PASS; `cargo clippy --all-targets` clean (AGENTS.md requires clippy; do **not** run `cargo fmt --all` — repo is hand-formatted).

- [ ] **Step 6: Commit**
  ```bash
  git add src/api/fleet_portal/trip_writes.rs src/db/trip_ops.rs tests/...
  git commit -s -m "feat(fleet): clear trip rate overrides to inherited via PATCH null (Phase 5)"
  ```

---

## Task 2: Pure payload helpers (TDD, Vitest)

**Files:**
- Create: `static/fleet/pages/trip-form-payload.js`
- Test: `tests/fleet/trip-form-payload.test.js`

- [ ] **Step 1: Write failing tests** covering:
  - `buildCreateTripPayload`: **load-linked** mode (`load_id` set, stops left empty ⇒ payload omits `stops` so the backend derives them); **free-standing** mode (`load_id` blank ⇒ requires ≥1 explicit stop, else an error `"At least one stop is required for a free-standing trip"`); coerces `driver_id`/`truck_id`/`trailer_ids` (drops blanks); omits blank optional top fields; numbers stops by row order; normalizes stop datetimes via the shared `toNaiveDateTime`.
  - `buildTripPatch`: the **clear/keep/set** rule for each rate override — a field left at its inherited ghost (untouched) is **omitted**; a field explicitly cleared emits **`null`**; a field with a value emits the number. Asserts e.g. `{ loaded_rate_per_mile: null }` for an explicit clear and absence of the key when untouched. Also `notes` passthrough and that mileage/driver fields never appear.
  - `tripStopTypes()` returns the `TripStopType` set (`origin|fuel|pickup|delivery|relay|empty_move|maintenance|terminal` — confirm against `src/models/trip.rs:52`).
  - Reuse the load helper's `toNaiveDateTime` via import (`import { toNaiveDateTime } from './load-form-payload.js'`) rather than redefining — add a test asserting they agree.
  Run: `npm --prefix static/fleet test -- trip-form-payload` → FAIL.

- [ ] **Step 2: Implement the helpers** as pure functions (mirror `load-form-payload.js` style: `str`/`numOrUndef`/`intOrUndef`/`setIf`). The override tri-state is the crux — model each rate field in form state as `{ value: string, cleared: boolean, inherited: number|null }`; `buildTripPatch` emits the key only when `cleared` (→ `null`) or `value !== ''` (→ number).

- [ ] **Step 3:** `npm --prefix static/fleet test -- trip-form-payload` → PASS.

- [ ] **Step 4: Commit**
  ```bash
  git add static/fleet/pages/trip-form-payload.js tests/fleet/trip-form-payload.test.js
  git commit -s -m "feat(fleet-ui): pure trip payload builder + override clear/keep/set (Phase 5)"
  ```

---

## Task 3: Routing + nav wiring (no behavior change yet)

**Files:** `static/fleet/router.js`, `static/fleet/utils/dom.js`, `static/fleet/app.js`

- [ ] **Step 1:** In `router.js`, add `trip-new` BEFORE the existing `trip-detail`, plus `trip-edit`:
  ```javascript
  { name: 'trips',       re: /^\/fleet\/trips$/ },
  { name: 'trip-new',    re: /^\/fleet\/trips\/new$/ },
  { name: 'trip-edit',   re: /^\/fleet\/trips\/([^/]+)\/edit$/, id: true },
  { name: 'trip-detail', re: /^\/fleet\/trips\/([^/]+)$/, id: true },
  ```
- [ ] **Step 2:** In `utils/dom.js` `VIEW_PATHS`, beside the `trips`/`trip-detail` lines:
  ```javascript
  'trip-new': () => '/fleet/trips/new',
  'trip-edit': (p) => `/fleet/trips/${p.id}/edit`,
  ```
- [ ] **Step 3:** In `app.js`, add `import { renderTripForm } from './pages/trip-form.js';`, `VIEW_TITLES` entries `'trip-new': 'New Trip'`, `'trip-edit': 'Edit Trip'`, and the two new cases (`trip-new` → `renderTripForm(null)`, `trip-edit` → `renderTripForm(params.id)`). Leave the inline `trips`/`trip-detail` cases for now. Create `pages/trip-form.js` as a one-line spinner stub so the import resolves (pages `trips.js`/`trip-detail.js` are created in Tasks 4/6 when their import/case swap happens — never two `renderTripsView` symbols at once).
- [ ] **Step 4:** In `tests/fleet/router.test.js`, assert `/fleet/trips/new` → `trip-new` (not `trip-detail`) and `/fleet/trips/abc/edit` → `trip-edit`. `npm --prefix static/fleet test -- router` → PASS.
- [ ] **Step 5: Commit**
  ```bash
  git add static/fleet/router.js static/fleet/utils/dom.js static/fleet/app.js static/fleet/pages/trip-form.js tests/fleet/router.test.js
  git commit -s -m "feat(fleet-ui): register trip new/edit routes + form stub (Phase 5)"
  ```

---

## Task 4: Trips list page (migrate read-only + gated Create)

**Files:** `static/fleet/pages/trips.js`; reference inline `renderTripsView` (`app.js:160–222`).

- [ ] **Step 1:** Port the inline list verbatim into `renderTripsView(params)`: status filter (`['', planned, assigned, dispatched, in_transit, delivered, completed, cancelled]`), the stop-time sort, `Trip#/Load#/Status/Driver/Route/Pickup/Delivery` columns, row → `trip-detail`, `No trips found` empty state. Import `apiFetch, API_BASE, hasScope` from `../utils/api.js` and `setContent, navigate` from `../utils/dom.js`; bring `fmtArrivalWindow/badge/escHtml/shortId` from `../utils/format.js` (confirm their export home — `grep -n "fmtArrivalWindow\|export" static/fleet/utils/format.js`).
- [ ] **Step 2:** Add a scope-gated header control: `hasScope('trips:write')` → **+ New Trip** button → `navigate('trip-new')`.
- [ ] **Step 3:** Swap `app.js`: add `import { renderTripsView } from './pages/trips.js';`, delete the inline `renderTripsView` function, keep the `case 'trips'`. Browser-verify list + filter + row-nav + gated button (Task 7 harness).
- [ ] **Step 4: Commit**
  ```bash
  git add static/fleet/pages/trips.js static/fleet/app.js
  git commit -s -m "feat(fleet-ui): migrate trips list to pages/ + gated Create (Phase 5)"
  ```

---

## Task 5: Trip form — create (load-linked + free-standing) + edit (inheritable overrides)

**Files:** `static/fleet/pages/trip-form.js`; reuse repeatable-row pattern from `pages/load-form.js`, object-select from `pages/driver-form.js`, `buildCreateTripPayload`/`buildTripPatch` from Task 2.

- [ ] **Step 1 (create mode, `id == null`):**
  - **Mode toggle:** "From a load" (default) vs "Free-standing". From-a-load shows a load `<select>` (`GET ${API_BASE}/loads`) and, when chosen, lets stops auto-derive (omit `stops`); a **"customize stops"** affordance reveals the explicit stop editor. Free-standing hides the load select and **requires** the stop editor (≥1 row).
  - **Resources (optional at create):** driver `<select>` (`GET /drivers`), truck `<select>` (`GET /trucks`), trailers multi-select (`GET /trailers`). All optional — assignment can also happen on the detail page.
  - **Stops editor** (when shown): reuse the load-form stop row (stop_type, facility select + name/address, `scheduled_arrive` datetime-local, `scheduled_arrive_end`, timezone alias-select, dwell/detention, notes). Note trip `stop_type` set differs from load (`tripStopTypes()`); a free-standing trip needs an explicit `origin`-type first stop only if the user wants deadhead — keep it simple: default a single `pickup`+`delivery` pair like load-form and let the user adjust.
  - On submit: `buildCreateTripPayload(state)`; on errors show inline; else `POST ${API_BASE}/trips` → `navigate('trip-detail', { id: body.id })`.
- [ ] **Step 2 (edit mode, `id` set):** `GET /trips/${id}`. PATCH only exposes: `notes`; and the **inheritable rate overrides** (`loaded_rate_per_mile`, `deadhead_rate_per_mile`, `extra_stop_fee`, `detention_rate_per_hour`, `free_dwell_minutes`). For each override render the **ghost-placeholder** pattern (spec): show the inherited/effective value as a muted placeholder; an empty input = inherited (untouched); typing a value = set; a **"clear to inherited"** affordance on a currently-overridden field marks it `cleared` so `buildTripPatch` emits `null`. Driver/truck/trailers/stops are **not** edited here (a note directs the user to the detail page's assign action). On submit: `buildTripPatch(state)`; if the trip is settled, surface the backend 409 inline. `PATCH /trips/${id}` → back to detail.
- [ ] **Step 3:** Browser-verify: create-from-load persists & derives stops; create free-standing with explicit stops persists; edit sets an override; edit **clears** an override back to inherited (confirm the ghost returns and `driver_pay` reflects the inherited rate); settled-trip override edit shows the 409.
- [ ] **Step 4: Commit**
  ```bash
  git add static/fleet/pages/trip-form.js
  git commit -s -m "feat(fleet-ui): trip create (load-linked + free-standing) + edit overrides (Phase 5)"
  ```

---

## Task 6: Trip detail page + lifecycle actions + delete

**Files:** `static/fleet/pages/trip-detail.js`; reference inline `renderTripDetailView` (`app.js:226–314`). Use `confirm.js` for destructive/irreversible confirms.

- [ ] **Step 1:** Port the read-only detail into `renderTripDetail(id)`: `GET /trips/${id}`, the detail card, the **mileage-summary stops table** (origin row + per-stop inbound miles via the documented leg-index contract — port verbatim, it's subtle), driver/truck/trailer rows, `driver_pay` display, notes, status badge, **Back** link, and an **Edit** button gated on `hasScope('trips:write')` → `navigate('trip-edit', { id })`.
- [ ] **Step 2:** Add a gated, **status-aware** action bar (each control hidden unless its scope is granted *and* the status permits the transition — gate against the `can_transition_to` matrix in Backend reality):
  - **Assign** (`trips:write`, status `planned`): pick driver + truck + trailers → `POST .../assign { driver_id, truck_id, trailer_ids }`.
  - **Unassign** (`assigned`): `POST .../unassign`.
  - **Dispatch** (`assigned`) / **Undispatch** (`dispatched`): `POST .../dispatch` | `.../undispatch`.
  - **Stop arrive/depart** (`dispatched|in_transit`): per-stop, prompt a naive local datetime (default now) → `POST .../stops/{seq}/arrive {actual_arrive}` | `.../depart {actual_depart}`.
  - **Mark late** (active): `{ eta?, notes? }` → `POST .../stops/{seq}/late` (204).
  - **Check-call** (active): `{ location, notes?, eta_next_stop? }` → `POST .../check-call` (204).
  - **Complete** (`delivered`): confirm → `POST .../complete` (204).
  - **Recalculate miles** (`trips:write`, not settled): `POST .../recalculate-miles {force:true}`; surface the `mileage_recompute_warning`/409 (ORS down / settled-frozen) inline.
  - **Cancel / Delete** (`trips:delete` for delete; cancel is `trips:write`): a single guarded action mirroring the backend's soft-then-hard `DELETE`. Use `confirm.js`; on **409** show the backend message (`"cannot cancel trip with status '...'"`) and keep the trip; on success → `navigate('trips')`. (Document: this is **not** the two-tier soft-delete/reactivate UI — backend collapses it into one endpoint, like Loads.)
  - After any non-delete action, **re-fetch and re-render** (no optimistic UI).
- [ ] **Step 3:** Swap `app.js`: `import { renderTripDetail } from './pages/trip-detail.js';`, change `case 'trip-detail'` to call it, and **delete** the inline `renderTripDetailView` + any helpers only it used (e.g. the `milesForStop` closure now lives in the page). Browser-verify each transition against `cargo run`.
- [ ] **Step 4: Commit**
  ```bash
  git add static/fleet/pages/trip-detail.js static/fleet/app.js
  git commit -s -m "feat(fleet-ui): trip detail page + full lifecycle actions + delete (Phase 5)"
  ```

---

## Task 7: Remove the last legacy inline views + full verification

**Files:** `static/fleet/app.js`

- [ ] **Step 1:** Confirm `app.js` now has **no** inline entity render functions left: `grep -n "renderTripsView\|renderTripDetailView" static/fleet/app.js` → only import lines remain. Remove any now-dead helper imports/functions. This completes the spec's "modular rewrite" — `app.js` is shell + router + boot only.
- [ ] **Step 2: Full JS suite.** `npm --prefix static/fleet test` → PASS — prior 86 **plus** the new `trip-form-payload` + router precedence tests.
- [ ] **Step 3: Backend.** `cargo test` (full) + `cargo clippy --all-targets` clean (Task 1 is the only Rust change). `cargo build` for the dev server.
- [ ] **Step 4: Playwright E2E** against local `cargo run` (owner `owner@dev.local`, scopes `*`), mirroring Phase 1–4 verification. Cover: trips list + filter; **create from a load** → stops derived → detail; **create free-standing** with explicit stops → detail; **edit** sets then **clears** a rate override (ghost returns, pay recomputes); **assign → dispatch → arrive/depart → complete** happy path; **check-call** + **mark-late** (204s, no body errors); **recalculate-miles**; **cancel** an active trip then confirm **delete** of the cancelled trip; **delete blocked by 409** on an `in_transit`/`completed` trip; scope-gated controls hidden when a scope is removed. Capture console for only-benign errors (refresh 401 when logged out, favicon 404, expected 409).
- [ ] **Step 5: Update the progress doc + commit.** Append a "Phase 5 — Trips" section to `docs/superpowers/OVERNIGHT-PROGRESS.md` noting decisions (backend `double_option` override-clear added per spec; create supports load-linked **and** free-standing; PATCH covers notes+overrides only — driver/equipment/stops via lifecycle endpoints; single guarded delete not two-tier; settlement-freeze 409s surfaced) and verification results. Note that **Phase 5 completes the Fleet UI CRUD project** — `app.js` holds zero inline entity views.
  ```bash
  git add static/fleet/app.js docs/superpowers/OVERNIGHT-PROGRESS.md
  git commit -s -m "refactor(fleet-ui): drop last inline trip views; Phase 5 verified — project complete"
  ```

---

## Self-review notes (spec coverage)

- **Trip create — load-linked + free-standing** → Task 5 Step 1 (mode toggle; load-linked derives stops, free-standing requires them — matches `src/api/trips.rs`).
- **Inheritable rate overrides w/ revert** → Task 1 (backend `double_option` clear) + Task 2 (`buildTripPatch` tri-state) + Task 5 Step 2 (ghost placeholder + clear-to-inherited UI). This is the one spec item that needed a backend change; the user approved it.
- **Detail lifecycle + stop events** → Task 6 (assign/unassign, dispatch/undispatch, complete, arrive/depart/late, check-call, recalculate-miles), all status-gated against `can_transition_to`.
- **Scope gating** (`trips:{read,write,delete}`) → Tasks 4, 6.
- **Modular rewrite finished** → Task 7 (last inline views removed; `app.js` = shell only).
- **Known carry-overs accepted (consistent with Phases 1–4):** single guarded delete (soft-then-hard) not two-tier soft/reactivate UI — backend has one endpoint; tags/timezone affordances identical to load-form; miles shown pending while async routing completes.
- **Deviation logged:** PATCH cannot edit stops/driver/equipment (backend routes those through lifecycle endpoints), so the edit form is notes + overrides only and directs structural changes to the detail action bar.
