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
