# Overnight progress — Fleet UI CRUD (Task 5 + Phase 1)

Run date: 2026-06-05 (unattended). Branch: `claude/thirsty-black-57145e`.

## What was completed

### (a) Deferred Phase 0b-ii Task 5 — read-only view extraction (PURE refactor)
Moved every read-only view out of the 1765-line `app.js` into `static/fleet/pages/` ES modules, behaviour-identical:
- `pages/home.js`, `pages/events.js` (owns its refresh timer + `clearEventsRefresh`), `pages/documents.js`, `pages/document-detail.js` (owns the object-URL lifecycle; exports `revokeActiveObjectUrl()`), `pages/account.js` (owns `API_KEYS_BASE`), `pages/login.js` (showLogin/showSetup/showLoginOrSetup/showApp + login/setup forms; `enterApp` is injected by app.js to avoid a cycle).
- New `utils/dom.js` holds the shared `setContent` / `setRefreshIndicator` / `navigate` / `goBack` + the `VIEW_PATHS` map, so pages navigate via `router.js` without importing from `app.js` (breaks the app-internal-helper cycle, per the Task 5 plan).
- `app.js` now imports these and retains only the still-legacy entity views (loads/trips/drivers + their details). Commit `81146d6`.

### (b) Phase 1 — Terminals + Equipment
- **Terminals** migrated off the inline `app.js` view into modules: `pages/terminals.js` (list via `renderTable`), **new** `pages/terminal-detail.js`, `pages/terminal-form.js` (via `renderForm`). Routes `/fleet/terminals/{new,{id},{id}/edit}` added.
- **Trucks** and **Trailers** built as new entity surfaces: `pages/{trucks,truck-detail,truck-form}.js` and `pages/{trailers,trailer-detail,trailer-form}.js`, with `/fleet/{trucks,trailers}/{new,{id},{id}/edit}` routes (placeholder routes removed).
- Shared page scaffolds added: `pages/_list.js`, `pages/_detail.js`, `pages/_form.js`.
- `hasScope` gates Create/Edit/Delete (`{trucks,trailers,terminals}:{write,delete}`).
- Delete wired to the existing endpoints; 409 messages surfaced inline.
- Styled the component-library `.table` and `.form-panel` classes in `components.css` with design tokens — they were built+tested in 0b-i but never given fleet CSS, so `renderTable`/`renderForm` output would otherwise be unstyled. No raw hex; tokens only.
- Router precedence tests added for the new routes. Commit `c495563`.

## Verification
- `npm test`: **66 passing** (incl. 6 new router tests). No backend code was touched, so `cargo clippy`/`cargo test` were not required; `cargo build` succeeds (used to run the dev server).
- **Browser-verified** with Playwright against a local `cargo run` (owner `owner@dev.local`, scopes `*`):
  - Trucks: list renders, create (via API) + row→detail (status badge), Edit form prefilled and **save persisted** (Make→Volvo, Plate→ABC123).
  - Terminals: list + detail render; Delete on the default terminal correctly **surfaced the 409** "cannot delete the default terminal" in the inline alert.
  - Trailers: create form (owner `<select>`) → create → detail; soft Delete succeeded and the row returned to the list as **status `inactive`**.
  - Deep links (`/fleet/trucks/{id}`, `/fleet/terminals/{id}`, `/fleet/trailers/new`) render via SPA fallback; sidebar nav + Back button work; only benign console errors (`/fleet/auth/refresh` 401 when logged out, favicon 404, the expected 409).

## Decisions / things needing morning review

1. **Trucks/Trailers forms intentionally omit `status`.** The task field spec said "status on edit", but the backend (`truck_writes.rs` / `trailer_writes.rs`) uses `#[serde(deny_unknown_fields)]` and explicitly does **not** accept `status` — trucks/trailers transition status only via the trip lifecycle. Sending `status` would 400. Status is shown read-only (badge) on the detail page. **This is a spec↔backend discrepancy; confirm the backend stance is intended (it appears deliberate).**

2. **No permanent hard-delete or reactivate UI for Trucks/Trailers.** The fleet-portal backend only exposes soft delete (`DELETE` → status `Inactive`); there is no `…/reactivate` or permanent-purge route for trucks/trailers (only Terminals' `DELETE` is a guarded permanent delete). So the two-tier delete UI from the spec is reduced to: soft delete for trucks/trailers, guarded permanent delete for terminals. **Permanent hard-delete + reactivate for equipment is deferred pending those backend routes** (out of Phase 1 scope; matches the task's "minimal/deferred if backend lacks a permanent route" allowance).

3. **Soft-deleted trucks/trailers still appear in the list** (as `inactive`) — the backend `list_trucks`/`list_trailers` default query does not filter out `Inactive`. Spec language is "hides from active lists"; that filtering would be a backend change, left out of this UI-only phase. Worth a follow-up if active-only lists are desired.

4. **`confirmDelete()` copy** ("can be undone by reactivating") is accurate-ish for trucks/trailers (soft delete) but no reactivate UI exists yet (see #2). Terminals use a custom permanent-delete confirm instead.

5. Dev data: the verification created truck `T-100` (edited to Volvo/ABC123) and a now-`inactive` trailer `TR-1` in `./data`. Harmless local dev artifacts.

## HEAD
`c495563664f1b71af66722dc5498b016b4367410` (before this progress-doc commit).

## Stopped after Phase 1 as instructed. Did not start Phase 2+. No pushes, no merges.

---

# Overnight progress — Phases 2 & 3 (Drivers + Facilities)

Run date: 2026-06-05 (second unattended run). Branch: `claude/thirsty-black-57145e`.
Resumed from HEAD `78f3e30` (after Phase 1). Stopped after Phase 3 as instructed.

## Phase 2 — Drivers (frontend only; backend driver CRUD already existed)
- Replaced the legacy inline drivers view with `pages/drivers.js` (list),
  `pages/driver-detail.js`, `pages/driver-form.js`. Routes
  `/fleet/drivers/{new,{id},{id}/edit}` wired; legacy
  `renderDriversView`/`renderDriverDetailView` removed from `app.js`.
- `components/form.js` extended: **inheritable "Revert to inherited" affordance**
  (clears the input + sends explicit `null` via `buildPayload`'s reverted set),
  a **`date`** field type, and **object-valued `select` options** (`{value,label}`)
  for the terminal picker.
- `components/table.js`: opt-in **`html: true`** column flag (for status badges).
- Driver form: name*, phone, email, license #/state/expiry(date), notes,
  terminal select, and the 5 **inheritable** rate overrides whose ghost
  placeholder shows the selected terminal's floor and **updates live** on
  terminal change.
- Driver detail: effective-rate display (override vs inherited), gated **Set PIN**
  (prompt → POST /pin), **Manage Equipment** inline panel (attach/detach
  truck+trailers via the existing endpoints), soft **Delete**.
- Tests: object-select, date input, revert affordance, table html flag, driver
  route precedence.

### Phase 2 decisions / morning review
- **Driver form omits `status`** — `UpdateDriverRequest` has no `status` field
  (drivers transition via the trip lifecycle / soft delete). Same spec↔backend
  discrepancy already flagged for trucks/trailers in Phase 1. Status is shown
  read-only (badge) on the detail page.
- **No driver Reactivate** — there is no driver reactivate route and PATCH can't
  set status, so soft Delete is one-way in the UI (matches trucks/trailers).
- **Driver detail dropped the legacy "Trips" sub-table** (the old inline view
  listed the driver's trips). Not in the spec's driver-detail field list; trips
  remain viewable from `/fleet/trips`. Minor regression — restore if desired.
- **Rate-overrides group is not collapsible** (spec says "collapsible"); rendered
  inline. `form.js` has no group/collapse primitive yet.
- Equipment attach/detach uses single-select truck + single trailer per action
  (additive); detach offers "Detach truck" / "Detach all trailers".

## Phase 3 — Facilities (BACKEND + FRONTEND)
### Backend (carefully review the migration)
- Added `archived: bool` to `FacilityRecord`/`FacilityListItem`; persisted in
  `facility_ops` (schema column, `facility_to_batch`, `row_to_facility`,
  `empty_facility_batch`) with an `open_or_create_facility` migration adding the
  column via **`CAST(false AS boolean)`** (SQL keyword, not Arrow `Boolean`).
- `build_facility_filter` now excludes archived rows → archived facilities drop
  out of active lists **and the stop typeahead** (both go through it), while
  staying fetchable by id (detail/reactivate) and as reference targets.
- New `set_facility_archived` db op; `count_loads_referencing_facility` in
  `load_ops` backs the referrer guard.
- Routes: `DELETE /facilities/{id}` = soft archive (`facilities:write`);
  `POST /facilities/{id}/reactivate` (`facilities:write`);
  `DELETE /facilities/{id}/permanent` = guarded hard delete (`facilities:delete`),
  refused **409 + `referrer_conflict_message("facility", &[("loads", n)])`** when
  any load stop references it. Registered all three in the OpenAPI `paths()`.
- **NOTE / deviation:** the task text said "a permanent `DELETE /facilities/{id}`
  route", but the spec's two-tier model makes the *default* delete a soft
  archive. I followed the **spec**: `DELETE /{id}` = soft, `DELETE /{id}/permanent`
  = hard. Flagging in case the literal task wording was intended.
- Tests: migration round-trip (pre-archived fixture → column added, defaults
  false, archive drops from active list, reactivate restores); integration
  soft-archive/reactivate + active-list filter; permanent-delete 409 referrer
  guard then leaf-first purge once unreferenced.

### Frontend
- `pages/facilities.js` (list + geocode badge), `facility-detail.js`
  (geocode_status, normalized_address, coords, avg dwell, contacts; two-tier
  delete actions gated by state), `facility-form.js` (name*, address*, notes,
  tags, optional lat/lng with paired-coords validation, **repeatable contacts**
  sub-section). Routes wired; placeholder removed.

### Phase 3 decisions / morning review
- **tags is a comma-separated text input**, not a chip UI (spec says "chip
  input"). Functional; upgrade later.
- Repeatable **contacts** built bespoke in `facility-form.js` (the general
  repeatable primitive lands with Loads in Phase 4, per the spec).
- Permanent-delete confirmation requires typing the facility name (UI guard) on
  top of the backend 409 referrer check.

## Cross-cutting FIX (caught by browser verification)
- `utils/dom.js` `VIEW_PATHS` was **missing** `driver-new`, `driver-edit`, and
  ALL facility entries, so `navigate()` silently fell through to `/fleet/home` —
  this broke driver create/edit (Phase 2) and every facility navigation. Added
  the entries (commit `1e50a8f`). Verified the fix via a cache-busted module
  import returning the correct paths.

## Verification
- `cargo clippy`: clean. `cargo test`: **lib 293**, **migration 9**,
  **integration 237** — all pass. `npm test`: **72** pass.
- **Browser-verified** (Playwright, local `cargo run`, owner `owner@dev.local`):
  - Facilities list renders with geocode badge; **create form** renders with the
    repeatable contacts section; **create persisted** tags `["dock","reefer"]`
    + contact "Sam Receiver" (confirmed via API).
  - Facility **detail** renders all derived fields + contacts; **active** state
    shows Edit/Delete; after archiving, **archived** state shows
    Edit/Reactivate/Permanently delete (all scope-gated).
  - Driver **create form** renders; terminal select populated; the 5 inheritable
    rate inputs show ghost placeholders from the Default terminal
    (`Inherited: 120 (Terminal: Default)` etc.). No console errors.
  - **Caveat:** live click-through navigation could not be exercised in the
    browser because the dev server sends **no `Cache-Control`** and the
    pre-edit `dom.js` was heuristically disk-cached (its on-disk last-modified
    predated the edit). `browser_close` does not clear that cache. The
    VIEW_PATHS fix is correct (proven via fresh import) and matches the
    already-working Terminals/Trucks pattern; it will work on any cold client /
    after a release `?v=` bump. **Worth a quick human click-through after a hard
    refresh.**

## HEAD
`1e50a8fc383247d92837acb89d6cdc32235f40d9`

## Stopped after Phase 3 as instructed. Did NOT start Phase 4 (Loads) or Phase 5
(Trips). No pushes, no merges, no version/`?v=` bumps.

---

# Progress — Phase 4 (Loads)

Run date: 2026-06-09 (supervised). Branch: `claude/thirsty-black-57145e`.
Resumed from HEAD `f04fde9` (after Phase 3). Executed subagent-driven (fresh
implementer + spec review + code-quality review per task). Scoped after a
backend scout. Stopped after Phase 4; did NOT start Phase 5 (Trips).

## What was completed
Full Loads CRUD, migrating the legacy inline list/detail out of `app.js`:
- **`pages/load-form-payload.js`** (new, pure + Vitest-tested, 13 tests): `buildLoadPayload`
  (coercion, blank-omission, required validation, 1-based stop sequencing, naive-datetime
  normalization, rate-item filtering), `applyResolutionChoices` (facility-resolution merge),
  `serviceTypesFor`, `toNaiveDateTime`.
- **`pages/loads.js`** (new): list migrated verbatim (status filter, `LOAD_SCAN_CAP=2000`
  notice, sort, row→detail, states) + scope-gated **+ New Load** (`loads:write`).
- **`pages/load-form.js`** (new, bespoke): top fields + repeatable **stops** (≥1; `stop_type`-driven
  `service_type` options that repopulate on change; facility `<select>` from `/facilities` with
  a "— Facility not listed —" option revealing name+address; `datetime-local` arrivals; **timezone
  select with alias labels / IANA values**, default Central; detention fields) + repeatable
  **rate items** with a live running total. Submit funnels through a single `submitPayload` that
  also handles the **inline facility-disambiguation picker** (HTTP-200 candidate response →
  per-`stop_index` radio of candidates + "Create new facility" → `applyResolutionChoices` → resubmit;
  staged multi-stop re-resolution supported).
- **`pages/load-detail.js`** (new): detail migrated verbatim (Load Details grid, Rate table,
  Stops table, Trips sub-table, Documents) + gated action bar — **Edit** (`loads:write`),
  **Cancel** (`loads:write`, pre-delivery only), **Invoice** (`loads:invoice`, status `delivered`
  only), **Settle** (`loads:settle`, status `invoiced` only), **Delete** (`loads:delete`, permanent,
  409 surfaced in-page). Non-delete actions re-fetch/re-render.
- Routing/nav: `load-new` (registered before `load-detail`) + `load-edit` in `router.js`,
  `VIEW_PATHS`, `app.js` titles/cases; inline `renderLoadsView`/`renderLoadDetailView` removed.

## Backend reality that shaped the build (verified, differs from spec)
- **No `deny_unknown_fields`** on load structs (so the trucks/drivers status-field pain doesn't apply).
- **Facility disambiguation = HTTP 200** with a per-stop candidate array, not a 409.
- **Delete = single hard delete** (409 if active trips). No soft-delete/reactivate for loads →
  the spec's two-tier delete UI does NOT apply here.
- Stop `scheduled_arrive` is **naive local** (Z/offset rejected); `sequence` is required (set by row order);
  `timezone` required IANA. `service_type` validity: Pickup→{pre_loaded,live_load,relay},
  Delivery→{live_unload,drop_and_hook,relay}. Invoice requires status `delivered`; settle requires `invoiced`.

## Verification
- `npm test`: **86 pass** (10 files) — was 72; +13 payload, +1 router precedence. `cargo build`: clean (no Rust changed).
- Every task passed independent **spec-compliance** + **code-quality** review; real bugs caught & fixed:
  stale `errEl`/`submitBtn` after the picker innerHTML swap (→ live DOM lookup); settle reusing the
  soft-delete confirm copy (→ native confirm); Invoice/Settle missing status gates (→ added).
- **Browser-verified** (Playwright) against an **isolated fresh instance** (`PORT=3100`, temp
  LanceDB; owner `owner@dev.local`): loads list (filter + gated Create + empty state); create form
  renders all fields; `stop_type` Pickup↔Delivery **repopulates** service types; tz alias dropdown
  (Central default); facility dropdown populated from API; **rate running total** updates live
  ($1500.00); **create persisted → detail** `LD-2026-0001` (status planned); detail correctly shows
  Edit/Cancel/Delete and **hides Invoice/Settle on a planned load** (status gating works); **edit
  prefill** round-trips customer/load#/tags and the stop (facility selected, datetime trimmed to
  `2026-06-15T10:30`, tz Central). Only benign console errors (favicon 404, pre-login refresh 401).
- **API-verified** the form's exact payload shapes: create-with-`facility_id` (auto `LD-2026-0001`,
  `total_rate_usd` computed), cancel (`planned`→`cancelled`, reason persisted), delete (204).
- **Facility dedup / disambiguation — investigated, partially reproduced:** the embed model
  (`nomic-embed-text`, dim 768) was not pulled initially → `/api/embeddings` 404 → 500 on the
  name+address path. After `ollama pull nomic-embed-text`, embeddings work and the **dedup auto-use
  path is verified end-to-end** (an exact name+address re-entry resolved to the *existing* facility_id,
  proving embed→vector-search→score→threshold). The **HTTP-200 candidate picker** specifically did not
  trigger locally: (1) `nomic-embed-text` scores are near-binary for these short facility strings
  (exact≈1.0 → auto-use ≥0.92; paraphrases fall well below `FACILITY_DEDUP_LOW_THRESHOLD` 0.75 →
  auto-create), so the 0.75–0.92 band is rarely hit; and (2) the facility **vector index isn't built on
  a near-empty DB** (startup "KMeans cannot train … 0 vectors"), so sub-threshold candidate retrieval is
  unreliable until enough facilities accumulate. Both create paths DO embed best-effort at creation
  (`facility_writes.rs:146`, `facilities.rs:66`) — there is NO POST-vs-`create_new_facility` asymmetry
  (an earlier note wrongly claimed one). The real backend-dedup reliability defects (silent `embedding:
  None` with no backfill; IVF-PQ index unbuilt on small tables) are filed as **olliefms/ollie#346**.
  The frontend picker (200-resolution branch + `applyResolutionChoices`) is code-reviewed and
  unit-tested; worth a human pass on a populated DB to see the picker render.

## HEAD
`683a648` (before this progress-doc commit).

## Stopped after Phase 4. Did NOT start Phase 5 (Trips). No pushes, no merges, no version/`?v=` bumps.
