# Fleet UI CRUD — Design

**Date:** 2026-06-04
**Status:** Approved (pending final user review)

## Problem

The fleet backend already exposes full write support for every core entity
(`src/api/fleet_portal/*_writes.rs` plus loads/trips lifecycle handlers in
`data.rs`), but the fleet SPA (`static/fleet/`) is almost entirely read-only.
Only three things have write functionality wired up today:

- **Terminals** — inline create/edit form (the established CRUD pattern)
- **Documents** — upload
- **Account** — API key create/delete

Everything else (Loads, Trips, Drivers, and Trucks/Trailers/Facilities — which
have no views at all) is view-only. This project closes the gap: full create/
update/delete in the fleet UI for Drivers, Trucks, Trailers, Loads, Trips, and
Facilities, plus a structural rewrite of the fleet frontend into modules and
real URLs.

## Goals

- Full CRUD in the UI for **Drivers, Trucks, Trailers, Loads, Trips,
  Facilities** — including create forms (not just edit/lifecycle).
- A **modular rewrite** of the fleet SPA mirroring the driver portal's
  `pages/` / `components/` / `utils/` layout.
- **Path-based (pushState) routing** so URLs reflect the entity and item being
  viewed/edited, applied to *every* entity including Terminals.
- **Scope-aware UI**: write controls are gated by the signed-in user's
  effective scopes, fetched from a new `/me` endpoint and kept fresh.
- **Override safety**: inherited values (e.g. driver pay cascading from a
  terminal) are *displayed* but never *saved* as an override unless the user
  changes them intentionally — and existing overrides can be reverted to
  inherited.
- **Two-tier delete**: everyday "Delete" is a reversible soft delete (archive/
  inactive) and is the preferred path; a separate, deliberately difficult
  permanent hard delete is refused (with an enumerated explanation) whenever
  anything references the object.
- A real **JS test toolchain** (Vitest + happy-dom) for the growing frontend
  logic, plus Playwright for E2E.

## Non-goals

- Optimistic UI. Mutations re-fetch on success, matching current behavior.
- Migrating the driver portal or any non-fleet surface.

## Decisions (resolved during brainstorming)

| Topic | Decision |
|---|---|
| Entities in scope | Drivers, Trucks, Trailers, Loads, Trips, Facilities — full CRUD incl. create |
| Facilities | Brought in-scope (was the one referenced-but-unmanaged entity); needs new backend delete routes (soft archive + guarded permanent) and an `archived` flag |
| Work structure | One spec, phased implementation plan |
| Form pattern | Inline panel (list swaps to form in `#main-content`) |
| Routing | pushState path routing; URLs reflect entity + item; all entities incl. Terminals |
| Terminals | Gains a detail page for consistency (was inline-edit-only) |
| Sidebar | New "Equipment" heading grouping Trucks + Trailers |
| Delete (default) | **Soft delete** = archive / mark `Inactive`. The preferred, everyday action: reversible (reactivate), preserves references, hides from active lists. Existing `Inactive` status where present; new `archived` flag for facilities + terminals |
| Hard delete | A separate, deliberately **difficult** permanent purge. Refused with `409` + **enumerated referrers** if **any** reference exists (active or historical) — clear the chain leaf-first. Extra confirmation (type the name/number) in the UI |
| Referential integrity | The **hard-delete** guard: every permanent-delete handler runs a referrer check across the reference graph and refuses (409, naming the referrers) if anything points at the object. Composes with existing status guards |
| Authority | New `GET /fleet/api/v1/me`; UI gates controls by effective scopes |
| Scope freshness | Refresh `/me` on boot, browser refresh, token refresh, window focus, and any 403 |
| Code org | Full modular rewrite mirroring the driver portal |
| Inherited values | `inheritable` field type: show inherited as ghost placeholder, persist only on intentional change |
| Override revert | Backend gains `Option<Option<f64>>` clear support (driver + trip rates) so UI can send explicit `null` |
| Facility entry (stops) | Typeahead over existing + inline disambiguation on near-matches |
| Trip creation | Both load-linked and free-standing |
| JS testing | Vitest + happy-dom (units) + Playwright (E2E); adds `package.json` + CI job |

## Architecture

### Module layout

The fleet SPA keeps its persistent shell (sidebar + topbar + `#main-content`)
but moves all view logic into ES modules. `index.html` loads `app.js` as
`type="module"`.

```
static/fleet/
  index.html            # <script type="module" src="app.js">
  app.js                # entry: boot shell, pushState router, login gate, nav wiring
  utils/
    auth.js             # token storage, JWT decode, isAuthenticated
    api.js              # apiFetch, tryRefresh, scope store (/me cache + refresh)
    format.js           # fmtDate, fmtUSD, fmtMiles, badge, escHtml, shortId, ...
  components/
    form.js             # declarative inline CRUD panel (incl. inheritable fields)
    confirm.js          # confirmDelete(message) -> native confirm wrapper
    table.js            # shared list/table render + row-click helpers
    scope-gate.js       # show/hide controls by effective scope
  pages/
    home.js
    loads.js  load-detail.js  load-form.js
    trips.js  trip-detail.js  trip-form.js
    drivers.js  driver-detail.js  driver-form.js
    trucks.js  truck-detail.js  truck-form.js
    trailers.js  trailer-detail.js  trailer-form.js
    facilities.js  facility-detail.js  facility-form.js
    terminals.js  terminal-detail.js  terminal-form.js
    events.js  documents.js  document-detail.js  account.js  login.js
```

### Routing

`app.js` renders the shell once, then runs a pushState router (mirroring the
driver portal's `route()` with `path.match()` + lazy `import()`), rendering the
matched page into `#main-content` and syncing the active sidebar link. The
fleet static mount already has an SPA fallback to `index.html`
(`src/api/mod.rs`), so deep links serve the shell with no backend change.

Hitting any `/fleet/*` route while unauthenticated renders the login pane and
remembers the target path to redirect back after sign-in.

**URL scheme (uniform across entities):**

| Entity | List | Create | Detail | Edit |
|---|---|---|---|---|
| Loads | `/fleet/loads` | `/fleet/loads/new` | `/fleet/loads/{id}` | `/fleet/loads/{id}/edit` |
| Trips | `/fleet/trips` | `/fleet/trips/new` | `/fleet/trips/{id}` | `/fleet/trips/{id}/edit` |
| Drivers | `/fleet/drivers` | `/fleet/drivers/new` | `/fleet/drivers/{id}` | `/fleet/drivers/{id}/edit` |
| Trucks | `/fleet/trucks` | `/fleet/trucks/new` | `/fleet/trucks/{id}` | `/fleet/trucks/{id}/edit` |
| Trailers | `/fleet/trailers` | `/fleet/trailers/new` | `/fleet/trailers/{id}` | `/fleet/trailers/{id}/edit` |
| Facilities | `/fleet/facilities` | `/fleet/facilities/new` | `/fleet/facilities/{id}` | `/fleet/facilities/{id}/edit` |
| Terminals | `/fleet/terminals` | `/fleet/terminals/new` | `/fleet/terminals/{id}` | `/fleet/terminals/{id}/edit` |

Plus `/fleet/home`, `/fleet/events`, `/fleet/documents`,
`/fleet/documents/{id}`, `/fleet/account`. Bare `/fleet` redirects to
`/fleet/home`.

### Sidebar

Existing flat links (Home, Loads, Trips, Drivers, Facilities, Events,
Documents, Terminals, Account) plus a new non-clickable **Equipment** heading
nesting **Trucks** and **Trailers**.

## Components

### `components/form.js` — declarative inline form panel

Each entity describes its form as a field list; the component handles
rendering, validation, type coercion, payload building, and submit/error state.

```js
renderForm(container, {
  title: 'Edit truck',
  fields: [
    { key: 'unit_number', label: 'Unit #', type: 'text', required: true },
    { key: 'make',        label: 'Make',   type: 'text' },
    { key: 'year',        label: 'Year',   type: 'int' },
    { key: 'status',      label: 'Status', type: 'select', options: [...] },
  ],
  values,                      // {} for create, record for edit
  submitLabel: 'Save',
  onSubmit: async (payload) => { /* page does POST/PUT, returns Response */ },
})
```

- **Coercion by `type`:** `number`→`parseFloat`, `int`→`parseInt`,
  `checkbox`→bool, blank→omitted (so create payloads stay clean and PUT/PATCH
  treats absent as "leave unchanged" — preserving today's Terminals semantics).
- **Validation:** required fields checked before submit.
- **Errors:** backend `{error}` surfaced in an inline `alert--error`; submit
  button disabled during the request.
- **Repeatable sub-sections** (load stops, rate items) and **typeahead**
  (facility, load, driver/truck/trailer pickers) are provided as form
  primitives that the bespoke `load-form.js` / `trip-form.js` compose.

#### Inheritable fields (override safety)

`form.js` supports an `inheritable` field type for cascading values (driver pay
from terminal; trip rate overrides from driver→terminal). Each is declared
with:

- `value` — the entity's own stored override (`null` when inherited)
- `inheritedValue` — the effective value if not overridden (e.g. terminal rate)
- `inheritedFrom` — label, e.g. `"Terminal: Dallas"`

**Rendering:**

- *Inherited (no override):* input left **empty**; inherited value shown as a
  **ghost placeholder** with an "Inherited from {source}" tag. The number is
  displayed, not held in the input.
- *Overridden:* input holds the actual value, with an "Overridden" tag and a
  **"Revert to inherited"** control.

**Submit rule** — a rate field is included in the payload only when it
represents an intentional override:

| Starting state | User action | Sent? |
|---|---|---|
| Inherited | left blank | **omitted** — never persists the inherited number |
| Inherited | typed a value | sent (new override) |
| Overridden | kept / edited | sent (value) |
| Overridden | clicked "Revert to inherited" | sent as explicit `null` (clear) |

### `components/scope-gate.js`

`hasScope('drivers:write')` consults the scopes cached from `/me`, honoring the
same `resource:*` / global-`*` rules as the backend's `scope_granted`. Pages
call it to decide whether to render Create/Edit/Delete/lifecycle controls.
**Fail-safe:** if `/me` hasn't loaded, controls are hidden, not shown.

### `components/confirm.js`

Thin wrapper over native `confirm()` with a standard destructive-action
message; returns bool. Used for all deletes. When the backend refuses a delete
with `409`, the enumerated referrer message is surfaced in a follow-up
`alert--error` (see Delete policy).

## Backend changes

The frontend rewrite needs four backend additions.

### 1. `GET /fleet/api/v1/me`

Returns the authenticated user's identity + authority:

```json
{ "fleet_user_id": "...", "name": "...", "email": "...", "role": "dispatcher",
  "effective_scopes": ["loads:read", "loads:write", "drivers:write", ...] }
```

Serializes `claims.effective_scopes` (already computed per-request by the auth
middleware) plus a DB lookup for name/email/role. Lives in `fleet_portal`
behind the existing auth middleware.

### 2. Explicit override-clear for rate fields

Driver and trip rate-override fields change from `Option<f64>` to
`Option<Option<f64>>` (mirroring the existing `Option<Option<String>>` pattern
on terminal `address`), giving three-state semantics:

- absent (`None`) → leave unchanged
- explicit `null` (`Some(None)`) → clear the override back to inherited
- value (`Some(Some(v))`) → set the override

Applies to `UpdateDriverRequest` and `UpdateTripRequest` rate fields and their
`apply_*_patch` / `update_*_rate_overrides` merge logic. The "rate changed"
gate triggers on *presence* (`is_some()`), so a clear is treated as a change.
Setting one rate must not clobber the others.

### 3. Soft delete (default) + reactivate

The everyday "Delete" is a reversible soft delete that hides the record from
active lists/pickers while preserving it as a reference target:

- **Drivers / Trucks / Trailers** — set `status = Inactive` (the existing
  `soft_delete_*` ops already do this). The current `DELETE` handlers already
  call these, so this tier mostly stays as-is.
- **Facilities / Terminals** — have no status enum; add a lightweight
  `archived: bool` field. Add `DELETE /fleet/api/v1/facilities/{id}` (new
  `facilities:delete` scope) and reuse terminal delete to set `archived`.
- **Loads / Trips** — the existing `cancel` / `Cancelled` lifecycle is the soft
  archive; no new soft path needed.

All soft deletes are **reversible** via a reactivate/unarchive action
(`POST .../{id}/reactivate`), gated by the entity's `:write` scope. Soft delete
does **not** run a referrer check — it's always safe because the row persists.

### 4. Hard delete (permanent) with referential-integrity guard

A separate, deliberately difficult permanent purge — exposed as a distinct
route (e.g. `DELETE .../{id}/permanent` or `?permanent=true`, gated by a
stricter `:delete` scope) and requiring extra confirmation in the UI. The
handler:

1. Runs a **referrer check** across the reference graph (matrix in Delete
   policy below). If any referrer exists — active *or* historical — returns
   `AppError::Conflict` (HTTP `409`) with a message that **enumerates the
   referrers** (type + count, ids where cheap, e.g. `"cannot permanently
   delete: referenced by 3 trips (#1024, #1025, #1031)"`).
2. Composes with existing **status guards** (e.g. an in-transit/delivered/
   completed trip cannot be deleted regardless of references).
3. Only if **unreferenced** and status-eligible, removes the row (the codebase
   already has `hard_delete_trip` and the `AppError::Conflict` → 409 precedent
   in blob/trip/load deletes).

Implementation: a `count_referrers` / `find_referrers` db op per entity.

## Delete policy (soft + hard)

Two tiers, surfaced on each entity's detail page.

**Tier 1 — Delete (soft, default, preferred).** Archives / marks the record
`Inactive`. Reversible via **Reactivate**. Preserves all references; just hides
the record from active lists and pickers. Single native `confirm()`. This is
the button users reach for normally.

**Tier 2 — Permanently delete (hard, deliberately difficult).** Reached from a
secondary/advanced control (typically only on an already-archived record),
behind a stronger confirmation (type the entity's name/number). The backend
refuses with `409` + an **enumerated referrer list** if anything points at the
object; the UI surfaces that message and the user must delete the referrers
first (leaf-first down the chain).

**Reference graph** (what blocks a permanent delete):

| Permanently deleting… | …is blocked by referrers |
|---|---|
| Driver | Trips (`driver_id`); equipment it currently holds (`current_truck_id`/`current_trailer_ids` — detach first) |
| Truck | Trips (`truck_id`); Drivers holding it (`current_truck_id`) |
| Trailer | Trips (`trailer_ids`); Drivers holding it (`current_trailer_ids`) |
| Facility | Load stops (`facility_id`) |
| Terminal | Drivers (`terminal_id`) |
| Load | Trips (`load_id`) |
| Trip | Other trips (`previous_trip_id`); pay records |

The 409 message names the referrer types and counts (and ids where cheap) so
the user knows exactly what to clear. Status guards still apply on top (e.g. an
in-transit trip is undeletable regardless of references).

**Scope mapping:** soft delete / reactivate require the entity's `:write`
scope (it's a reversible state change); permanent hard delete requires the
stricter `:delete` scope (reserved for the irreversible purge). This re-homes
the existing `:delete` scope onto the permanent path.

## Per-entity field specs

Create forms **omit `status`** (backend defaults it); edit forms add a status
`select`. Each detail page exposes **Delete** (soft) + **Reactivate** and, on
archived records, **Permanently delete** (hard, guarded).

### Trucks (`trucks:write` / `trucks:delete`)

`unit_number`*, `year`, `make`, `model`, `vin`, `plate`, `plate_state`,
`notes`; edit adds `status` (Available/Assigned/Dispatched/OutOfService/Inactive).

### Trailers (`trailers:write` / `trailers:delete`)

`unit_number`*, `owner`* (Fleet/Carrier/Customer/Other), `owner_name`, `year`,
`make`, `trailer_type`, `length_ft`, `vin`, `plate`, `plate_state`, `notes`;
edit adds `status`.

### Drivers (`drivers:write` / `drivers:delete`)

`name`*, `phone`, `email`, `license_number`, `license_state`, `license_expiry`
(date), `notes`, `terminal_id` (select from `/terminals`), plus a collapsible
**rate-overrides** group (`loaded_rate_per_mile`, `deadhead_rate_per_mile`,
`extra_stop_fee`, `detention_rate_per_hour`, `free_dwell_minutes`) using the
**inheritable** field type (inherits from the selected terminal). Edit adds
`status` (Available/Assigned/Dispatched/Inactive).

Driver **detail page** carries gated action buttons: **Set PIN**,
**Attach/Detach equipment** (truck + trailers).

### Facilities (`facilities:write` / `facilities:delete`)

`name`*, `address`*, `notes`, `tags` (chip input), optional `lat`/`lng`
(advanced/optional — normally left blank for the backend to geocode), and a
repeatable **contacts** section (`name`*, `title`, `phone`, `email`, `notes`)
using the same repeatable primitive as load rate items. Update uses `PATCH`
(`UpdateFacilityRequest`, optional fields).

The **detail page** additionally surfaces read-only/derived fields:
`geocode_status` (+ `normalized_address`, geocode failure count),
`avg_dwell_minutes`, and `dwell_sample_count`. Creating/editing only sends the
editable fields; geocoding happens asynchronously server-side after save, so
the detail view should reflect a pending/failed/succeeded geocode state.

### Loads

Bespoke `load-form.js`. Top fields: `customer_name`*, `customer_ref`,
`load_number` (blank→auto), `commodity`, `weight_lbs`, `miles` (blank→backend
routes it), `notes`, `tags` (chip input). Two repeatable sections:

- **Stops** (≥1): `stop_type` (Pickup/Delivery), `service_type` (validated
  against stop_type), **facility** (typeahead to pick existing *or* enter
  `facility_name`+`address`), `scheduled_arrive`*, `scheduled_arrive_end`,
  `timezone`* (IANA), `expected_dwell_minutes`, detention fields, `notes`.
  Sequence is implicit by row order.
- **Rate items** (repeatable): `description` + `amount_usd`, with running total.

**Facility resolution:** when a stop is entered by name+address, the backend may
return fuzzy-match candidates. The form catches that response and shows an
inline disambiguation step — pick a candidate or **"Create new facility"**
(`force_new_facility: true`) — then resubmits.

**Detail actions** (gated): **Cancel** (reason prompt, `loads:write`),
**Invoice** (`loads:invoice`), **Settle** (`loads:settle`), **Delete**
(`loads:delete`).

### Trips

Bespoke `trip-form.js`, supporting **both** creation paths:

- *Load-linked:* pick an existing load (typeahead); stops/sequence derive from
  it.
- *Free-standing:* `load_id` optional; manually entered stops.

Fields: `trip_number` (blank→auto), `load_id`, `sequence`, `driver_id`,
`truck_id`, `trailer_ids` (selects), `notes`, `previous_trip_id`, plus
**inheritable** trip rate overrides (cascade driver→terminal).

**Detail actions** (gated by `trips:write` and current status): assign/unassign,
dispatch/undispatch, complete, cancel, recalculate-miles, plus per-stop
arrive/depart/late and check-call.

## Scope-store lifecycle

`utils/api.js` owns the cached scopes and keeps them fresh:

- **Boot / browser refresh** — `/me` is fetched before first render (a reload
  re-runs boot, so hard refresh is covered for free).
- **Token refresh** — whenever the access token is silently refreshed
  (`tryRefresh`), `/me` is refreshed too.
- **Window focus** — refresh on `visibilitychange`/focus after the tab was
  hidden.
- **On any 403** — `apiFetch` forces a `/me` refresh and re-renders the current
  view so controls re-gate. If the 403 persists, show the "no permission"
  message; if scopes changed, the UI silently corrects.

After any refresh, if the scope set changed, re-run scope-gating on the mounted
view so controls appear/disappear without a manual reload.

## Error handling

- `apiFetch` surfaces backend `{error}` JSON into the form's inline
  `alert--error`.
- **401** → existing silent-refresh-then-login flow.
- **403 / insufficient scope** → trigger `/me` refresh + re-gate (above);
  friendly inline message if still denied.
- **422 validation** → surfaced inline.
- **409 on permanent delete** → the enumerated referrer message is shown in the
  delete dialog's `alert--error`; the record is not deleted and the user is
  pointed at the referrers to clear.
- **Facility resolution** → inline disambiguation step, not an error.
- Mutations re-fetch on success (no optimistic UI).

## Testing

### Backend (Rust, existing `*_writes.rs` / `jwt.rs` test style)

- `GET /me` returns the correct `effective_scopes` for a user's role +
  extra_scopes.
- Three-state `Option<Option<f64>>` semantics for driver + trip rates: absent =
  leave, `null` = clear, value = set — including that setting one rate doesn't
  clobber the others, and that a clear triggers the rate-update path.
- Soft delete + reactivate per entity: archives/marks `Inactive` (or sets
  `archived`), drops out of active lists/pickers, preserves references, and is
  reversible. Facilities/terminals gain the `archived` flag.
- Permanent-delete referrer guard per entity: returns `409` with an enumerated
  referrer message when any referrer exists (active or historical), composes
  with status guards, and only removes the row when fully unreferenced. One
  end-to-end "clear the chain leaf-first, then purge" test.

### Frontend (Vitest + happy-dom for units; Playwright for E2E)

New `package.json` (dev-only) + a CI job. Unit tests target the bug-prone pure
logic:

- `form.js` payload builder: coercion, blank-omission, required validation,
  and repeatable sub-sections (contacts, rate items, stops).
- Inherited-value submit rule (the table above) — each starting-state ×
  user-action combination.
- `scope-gate.js` matching: exact, `resource:*`, global `*`, denial.

Playwright E2E covers representative flows per entity: create→detail,
edit→persist, soft delete→archived→reactivate, blocked permanent-delete
(referrer message shown), scope-gated controls hidden, and the inherited-rate
"ghost placeholder, not saved unless changed" behavior end-to-end.

## Phasing (one spec → phased implementation plan)

- **Phase 0 — Foundation:** backend `/me` + `Option<Option>` clear support
  (driver+trip) with tests; the **delete framework** — soft-delete/reactivate
  convention + the `count_referrers`/permanent-delete guard pattern + the
  `archived` flag for facilities/terminals — established as shared helpers so
  each entity phase just wires its own reference checks; JS toolchain
  (`package.json`, Vitest, happy-dom, CI job); UI scaffold (`utils/`,
  `components/{form,scope-gate,confirm,table}.js`), pushState router + login
  gate in `app.js`; migrate the read-only views (home, events, documents,
  account, login) onto the new router so the shell fully works.
- **Phase 1 — Terminals + Equipment:** migrate Terminals to modules + detail
  page + routing (proof-of-pattern), then add Trucks & Trailers
  (list/detail/CRUD) + the Equipment sidebar group.
- **Phase 2 — Drivers:** driver pages + CRUD, inheritable rate-override fields,
  set-PIN, attach/detach equipment.
- **Phase 3 — Facilities:** `archived` flag + delete routes (`facilities:write`
  soft / `facilities:delete` permanent); facility pages + CRUD (repeatable
  contacts, geocode-status display). Lands before Loads so the load stop form
  has a real facilities data source + management surface.
- **Phase 4 — Loads:** `load-form` (stops w/ facility typeahead +
  disambiguation, rate items) + detail lifecycle actions
  (cancel/invoice/settle) + soft/permanent delete.
- **Phase 5 — Trips:** `trip-form` (load-linked + free-standing) w/ inheritable
  rate overrides + detail lifecycle actions + stop events.

Each phase ships independently behind the same unified design.
