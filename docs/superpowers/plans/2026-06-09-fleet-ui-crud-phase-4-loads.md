# Fleet UI CRUD — Phase 4 (Loads) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Loads fully read/write in the fleet SPA — migrate the legacy inline list/detail out of `app.js` into `pages/` modules and add a bespoke `load-form.js` (repeatable stops + rate items + facility disambiguation) plus detail lifecycle actions (cancel / invoice / settle / delete).

**Architecture:** Mirror the Phase 1–3 module pattern (`pages/<entity>{,-detail,-form}.js`, router + `VIEW_PATHS` + `app.js` case wiring, scope-gated controls, no optimistic UI). The form's bug-prone payload logic (coerce top fields, collect repeatable stops/rate-items, merge facility-resolution choices) is extracted into a **pure, unit-tested** `pages/load-form-payload.js` so it can be Vitest-covered with no DOM. The form UI imports it.

**Tech Stack:** Vanilla ES modules (`static/fleet/`), existing `components/{form,table,scope-gate,confirm}.js` + `utils/{api,dom}.js`, Vitest + happy-dom for units, Playwright for E2E. Backend is unchanged (all routes/structs already exist).

---

## Backend reality (verified — plan against THIS, not the spec's assumptions)

- **Routes** (`src/api/fleet_portal/mod.rs:60-67`), all under `API_BASE = /fleet/api/v1`:
  `GET /loads` · `POST /loads` (`loads:write`) · `GET /loads/{id}` · `PUT /loads/{id}` (`loads:write`) · `DELETE /loads/{id}` (`loads:delete`) · `POST /loads/{id}/invoice` (`loads:invoice`) · `POST /loads/{id}/cancel` (`loads:write`) · `POST /loads/{id}/settle` (`loads:settle`).
- **No `deny_unknown_fields`** on any load request struct — extra keys are ignored, not 400'd (unlike trucks/drivers).
- **`CreateLoadRequest`** (`src/models/load.rs:302`): `customer_name`* , `stops: Vec<StopInput>`* (≥1), `rate_items` (default `[]`), `load_number?` (blank→auto `LD-{YEAR}-{NNNN}`), `customer_ref?`, `commodity?`, `weight_lbs?: f64`, `miles?: f64` (blank→async backend routing), `notes?`, `tags` (default `[]`).
- **`StopInput`** (`src/models/load.rs:214`): `sequence: u32`* (frontend sets from row order), `stop_type`* (`pickup`|`delivery`), `service_type`* , `facility_id?: Uuid` **OR** (`facility_name?` + `address?`), `force_new_facility` (default false), `scheduled_arrive: String`* (**naive local**, no `Z`/offset), `scheduled_arrive_end?`, `expected_dwell_minutes?: u32`, `detention_free_minutes?: u32`, `detention_grace_minutes?: u32`, `notes?`, `timezone: String`* (IANA).
- **`service_type` valid-for-`stop_type`** (`src/models/load.rs:42`): Pickup → `pre_loaded`,`live_load`,`relay`; Delivery → `live_unload`,`drop_and_hook`,`relay`.
- **Stop times are naive** (`src/models/load.rs:199`): a `Z` or `+offset` is **rejected** when a timezone is set. `<input type="datetime-local">` yields `YYYY-MM-DDTHH:MM` (naive) — append `:00` seconds.
- **Facility disambiguation** (`src/api/loads.rs`, `src/error.rs:31` → **HTTP 200**, not 409): POSTing a stop by name+address may return a body shaped
  `[{ "stop_index": 0, "facility_resolution_required": true, "candidates": [{ "id", "name", "address", "normalized_address?", "score" }] }]`.
  Frontend shows an inline picker, then re-POSTs with the chosen `facility_id` **or** `force_new_facility: true` on that stop.
- **`RateLineItem`** (`src/models/load.rs:106`): `description: String`* , `amount_usd: f64`* .
- **Lifecycle bodies:** cancel `{ reason?: String }`; invoice `{ invoice_number?, invoice_date? }`; settle **no body**.
- **DELETE = hard delete** (`src/api/fleet_portal/data.rs:396`): **409** with message `"load has {N} active trip(s); cancel or complete them first"` if referenced. **No soft delete, no reactivate** — so the two-tier delete UI from earlier phases does NOT apply; this is a single guarded permanent delete (surface the 409 like Terminals).

## Files

- **Create** `static/fleet/pages/load-form-payload.js` — pure helpers: `buildLoadPayload(state)`, `applyResolutionChoices(payload, choices)`, `serviceTypesFor(stopType)`, `toNaiveDateTime(localValue)`.
- **Create** `static/fleet/pages/load-form.js` — bespoke create/edit form UI; exports `renderLoadForm(id|null)`.
- **Create** `static/fleet/pages/loads.js` — list page; exports `renderLoadsView(params)`.
- **Create** `static/fleet/pages/load-detail.js` — detail page + lifecycle actions; exports `renderLoadDetail(id)`.
- **Create** `static/fleet/tests/../load-form-payload.test.js` (mirror existing `tests/fleet/` location) — Vitest units for the pure helpers.
- **Modify** `static/fleet/router.js` — add `load-new` (BEFORE `load-detail`) + `load-edit` routes.
- **Modify** `static/fleet/utils/dom.js` — add `load-new` / `load-edit` to `VIEW_PATHS`.
- **Modify** `static/fleet/app.js` — add imports + `VIEW_TITLES` + `case` dispatch for `loads`/`load-detail`/`load-new`/`load-edit`; **remove** the inline `renderLoadsView` + `renderLoadDetailView` (≈ lines 151–530) and their old cases.

> Naming caution: `load-new` route regex MUST be registered before `load-detail` (`/^\/fleet\/loads\/([^/]+)$/`), exactly as `facility-new` precedes `facility-detail` in `router.js:26-28`, or `/loads/new` matches the detail route.

---

## Task 1: Pure payload helpers (TDD)

**Files:**
- Create: `static/fleet/pages/load-form-payload.js`
- Test: `tests/fleet/load-form-payload.test.js`

- [ ] **Step 1: Write the failing tests**

```javascript
import { describe, it, expect } from 'vitest';
import {
  serviceTypesFor, toNaiveDateTime, buildLoadPayload, applyResolutionChoices,
} from '../../static/fleet/pages/load-form-payload.js';

describe('serviceTypesFor', () => {
  it('pickup options', () => {
    expect(serviceTypesFor('pickup')).toEqual(['pre_loaded', 'live_load', 'relay']);
  });
  it('delivery options', () => {
    expect(serviceTypesFor('delivery')).toEqual(['live_unload', 'drop_and_hook', 'relay']);
  });
});

describe('toNaiveDateTime', () => {
  it('appends seconds to a datetime-local value', () => {
    expect(toNaiveDateTime('2026-05-10T09:15')).toBe('2026-05-10T09:15:00');
  });
  it('passes through a value that already has seconds', () => {
    expect(toNaiveDateTime('2026-05-10T09:15:30')).toBe('2026-05-10T09:15:30');
  });
  it('returns empty for blank', () => {
    expect(toNaiveDateTime('')).toBe('');
  });
});

describe('buildLoadPayload', () => {
  const baseStop = {
    stop_type: 'pickup', service_type: 'live_load', timezone: 'America/Chicago',
    scheduled_arrive: '2026-05-10T09:15', facility_id: 'fac-1',
  };

  it('omits blank top fields and auto fields, keeps required', () => {
    const { payload, errors } = buildLoadPayload({
      top: { customer_name: 'Acme', load_number: '', miles: '', weight_lbs: '' },
      stops: [baseStop], rateItems: [],
    });
    expect(errors).toEqual([]);
    expect(payload.customer_name).toBe('Acme');
    expect('load_number' in payload).toBe(false);
    expect('miles' in payload).toBe(false);
    expect('weight_lbs' in payload).toBe(false);
  });

  it('requires customer_name and at least one stop', () => {
    const { errors } = buildLoadPayload({ top: { customer_name: '' }, stops: [], rateItems: [] });
    expect(errors).toContain('Customer name is required');
    expect(errors).toContain('At least one stop is required');
  });

  it('numbers stops by row order and normalizes datetime', () => {
    const { payload } = buildLoadPayload({
      top: { customer_name: 'Acme' },
      stops: [baseStop, { ...baseStop, stop_type: 'delivery', service_type: 'live_unload' }],
      rateItems: [],
    });
    expect(payload.stops[0].sequence).toBe(1);
    expect(payload.stops[1].sequence).toBe(2);
    expect(payload.stops[0].scheduled_arrive).toBe('2026-05-10T09:15:00');
  });

  it('sends facility_name+address when no facility_id', () => {
    const { payload } = buildLoadPayload({
      top: { customer_name: 'Acme' },
      stops: [{ ...baseStop, facility_id: '', facility_name: 'Dock 7', address: '1 Main St' }],
      rateItems: [],
    });
    expect(payload.stops[0].facility_id).toBeUndefined();
    expect(payload.stops[0].facility_name).toBe('Dock 7');
    expect(payload.stops[0].address).toBe('1 Main St');
  });

  it('coerces rate item amounts and drops empty rows', () => {
    const { payload } = buildLoadPayload({
      top: { customer_name: 'Acme' }, stops: [baseStop],
      rateItems: [{ description: 'Line haul', amount_usd: '1200.50' }, { description: '', amount_usd: '' }],
    });
    expect(payload.rate_items).toEqual([{ description: 'Line haul', amount_usd: 1200.5 }]);
  });

  it('errors on a stop missing both facility_id and name/address', () => {
    const { errors } = buildLoadPayload({
      top: { customer_name: 'Acme' },
      stops: [{ ...baseStop, facility_id: '', facility_name: '', address: '' }],
      rateItems: [],
    });
    expect(errors.some(e => /facility/i.test(e))).toBe(true);
  });
});

describe('applyResolutionChoices', () => {
  it('sets facility_id from a chosen candidate by stop_index', () => {
    const payload = { stops: [{ facility_name: 'X', address: 'Y' }] };
    const out = applyResolutionChoices(payload, { 0: { facility_id: 'fac-9' } });
    expect(out.stops[0].facility_id).toBe('fac-9');
    expect(out.stops[0].facility_name).toBeUndefined();
    expect(out.stops[0].address).toBeUndefined();
  });
  it('sets force_new_facility when chosen', () => {
    const payload = { stops: [{ facility_name: 'X', address: 'Y' }] };
    const out = applyResolutionChoices(payload, { 0: { force_new: true } });
    expect(out.stops[0].force_new_facility).toBe(true);
    expect(out.stops[0].facility_name).toBe('X');
  });
});
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cd static/fleet && npm test -- load-form-payload` (from repo root: `npm --prefix static/fleet test -- load-form-payload`)
Expected: FAIL — module not found / functions undefined. (Confirm the exact test dir by matching an existing file, e.g. `tests/fleet/form.test.js`; place the new test beside it and fix the relative import path.)

- [ ] **Step 3: Implement the helpers**

```javascript
// static/fleet/pages/load-form-payload.js
const PICKUP = ['pre_loaded', 'live_load', 'relay'];
const DELIVERY = ['live_unload', 'drop_and_hook', 'relay'];

export function serviceTypesFor(stopType) {
  return stopType === 'delivery' ? [...DELIVERY] : [...PICKUP];
}

export function toNaiveDateTime(v) {
  if (!v) return '';
  // datetime-local yields YYYY-MM-DDTHH:MM ; backend wants naive seconds, no Z/offset
  return /\dT\d{2}:\d{2}$/.test(v) ? `${v}:00` : v;
}

function str(v) { return (v ?? '').toString().trim(); }
function numOrUndef(v) {
  const s = str(v); if (s === '') return undefined;
  const n = Number(s); return Number.isNaN(n) ? undefined : n;
}
function intOrUndef(v) {
  const n = numOrUndef(v); return n === undefined ? undefined : Math.trunc(n);
}
function setIf(obj, key, val) { if (val !== undefined && val !== '') obj[key] = val; }

function buildStop(raw, index, errors) {
  const stop = {
    sequence: index + 1,
    stop_type: raw.stop_type || 'pickup',
    service_type: raw.service_type,
    timezone: str(raw.timezone),
    scheduled_arrive: toNaiveDateTime(str(raw.scheduled_arrive)),
  };
  if (!stop.timezone) errors.push(`Stop ${index + 1}: timezone is required`);
  if (!stop.scheduled_arrive) errors.push(`Stop ${index + 1}: scheduled arrival is required`);

  const facilityId = str(raw.facility_id);
  if (facilityId) {
    stop.facility_id = facilityId;
  } else {
    const name = str(raw.facility_name), addr = str(raw.address);
    if (!name || !addr) {
      errors.push(`Stop ${index + 1}: pick a facility or enter both name and address`);
    } else {
      stop.facility_name = name;
      stop.address = addr;
    }
  }
  setIf(stop, 'scheduled_arrive_end', toNaiveDateTime(str(raw.scheduled_arrive_end)));
  setIf(stop, 'expected_dwell_minutes', intOrUndef(raw.expected_dwell_minutes));
  setIf(stop, 'detention_free_minutes', intOrUndef(raw.detention_free_minutes));
  setIf(stop, 'detention_grace_minutes', intOrUndef(raw.detention_grace_minutes));
  setIf(stop, 'notes', str(raw.notes));
  return stop;
}

export function buildLoadPayload({ top = {}, stops = [], rateItems = [] }) {
  const errors = [];
  const payload = {};

  if (!str(top.customer_name)) errors.push('Customer name is required');
  setIf(payload, 'customer_name', str(top.customer_name));
  setIf(payload, 'customer_ref', str(top.customer_ref));
  setIf(payload, 'load_number', str(top.load_number));
  setIf(payload, 'commodity', str(top.commodity));
  setIf(payload, 'notes', str(top.notes));
  setIf(payload, 'weight_lbs', numOrUndef(top.weight_lbs));
  setIf(payload, 'miles', numOrUndef(top.miles));

  const tags = (top.tags || []).map(str).filter(Boolean);
  if (tags.length) payload.tags = tags;

  if (!stops.length) errors.push('At least one stop is required');
  payload.stops = stops.map((s, i) => buildStop(s, i, errors));

  const rate = rateItems
    .map(r => ({ description: str(r.description), amount_usd: numOrUndef(r.amount_usd) }))
    .filter(r => r.description && r.amount_usd !== undefined);
  if (rate.length) payload.rate_items = rate;

  return { payload, errors };
}

// choices: { [stopIndex]: { facility_id } | { force_new: true } }
export function applyResolutionChoices(payload, choices) {
  const stops = payload.stops.map((stop, i) => {
    const choice = choices[i];
    if (!choice) return stop;
    if (choice.facility_id) {
      const { facility_name, address, ...rest } = stop;
      return { ...rest, facility_id: choice.facility_id };
    }
    if (choice.force_new) return { ...stop, force_new_facility: true };
    return stop;
  });
  return { ...payload, stops };
}
```

- [ ] **Step 4: Run tests, verify they pass**

Run: `npm --prefix static/fleet test -- load-form-payload`
Expected: PASS (all cases green).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/pages/load-form-payload.js tests/fleet/load-form-payload.test.js
git commit -s -m "feat(fleet-ui): pure load payload builder + facility-resolution merge (Phase 4)"
```

---

## Task 2: Routing + nav wiring (no behavior change yet)

**Files:**
- Modify: `static/fleet/router.js` (add `load-new` BEFORE `load-detail`, add `load-edit`)
- Modify: `static/fleet/utils/dom.js:15-16` (add `load-new` / `load-edit` to `VIEW_PATHS`)
- Modify: `static/fleet/app.js` (imports, `VIEW_TITLES`, cases)

- [ ] **Step 1: Add routes** in `static/fleet/router.js`, mirroring the facilities block. The `load-new` entry MUST precede the existing `load-detail` entry:

```javascript
{ name: 'loads',       re: /^\/fleet\/loads$/ },
{ name: 'load-new',    re: /^\/fleet\/loads\/new$/ },
{ name: 'load-edit',   re: /^\/fleet\/loads\/([^/]+)\/edit$/, id: true },
{ name: 'load-detail', re: /^\/fleet\/loads\/([^/]+)$/, id: true },
```

- [ ] **Step 2: Add `VIEW_PATHS` entries** in `static/fleet/utils/dom.js` (next to the existing `loads` / `load-detail` lines):

```javascript
'load-new': () => '/fleet/loads/new',
'load-edit': (p) => `/fleet/loads/${p.id}/edit`,
```

- [ ] **Step 3: Wire ONLY the new form routes** in `static/fleet/app.js`. The inline `renderLoadsView` / `renderLoadDetailView` and their `loads`/`load-detail` cases stay untouched here (avoids a name collision with the page imports); Tasks 3 and 6 swap those over. Add one import:

```javascript
import { renderLoadForm } from './pages/load-form.js';
```

Add `VIEW_TITLES`:

```javascript
'load-new': 'New Load',
'load-edit': 'Edit Load',
```

Add two new cases (leave the existing `loads`/`load-detail` cases as they are):

```javascript
case 'load-new': renderLoadForm(null); break;
case 'load-edit': renderLoadForm(params.id); break;
```

> Only `pages/load-form.js` needs to exist now — create it as a stub so the import resolves: `import { setContent } from '../utils/dom.js'; export function renderLoadForm() { setContent('<div class="state-loading"><div class="spinner"></div></div>'); }`. `pages/loads.js` and `pages/load-detail.js` are created in Tasks 3 and 6 respectively (when their `import`/`case` swap happens), so there is never a duplicate `renderLoadsView` symbol in `app.js`.

- [ ] **Step 4: Verify router unit tests still pass + add precedence test.** In `tests/fleet/router.test.js`, add a case asserting `/fleet/loads/new` matches `load-new` (not `load-detail`) and `/fleet/loads/abc/edit` matches `load-edit`.

Run: `npm --prefix static/fleet test -- router`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add static/fleet/router.js static/fleet/utils/dom.js static/fleet/app.js static/fleet/pages/load-form.js tests/fleet/router.test.js
git commit -s -m "feat(fleet-ui): register load new/edit routes + form stub (Phase 4)"
```

---

## Task 3: Loads list page (migrate read-only + Create button)

**Files:**
- Modify: `static/fleet/pages/loads.js`
- Reference: the inline `renderLoadsView` in `app.js` (≈ lines 151–273) — port its behavior verbatim (status filter, scan-cap notice, row→`load-detail`, sort), then add a scope-gated **+ New Load** button (`hasScope('loads:write')` → `navigate('load-new')`).

- [ ] **Step 1:** Port the existing list rendering into `pages/loads.js` as `renderLoadsView(params)`, importing `apiFetch, API_BASE` from `../utils/api.js`, `hasScope` from `../utils/api.js`, and `setContent, navigate` from `../utils/dom.js`. Preserve: `GET ${API_BASE}/loads?status=...`, the `{loads}|{items}|array` response handling, the `LOAD_SCAN_CAP = 2000` notice, sort, and `No loads found` empty state.

- [ ] **Step 2:** Add the Create control in the page header, gated:

```javascript
const createBtn = hasScope('loads:write')
  ? `<button class="btn btn--primary" id="new-load">+ New Load</button>` : '';
// after render:
document.getElementById('new-load')?.addEventListener('click', () => navigate('load-new'));
```

- [ ] **Step 3:** Manually verify in the browser (see Task 7 harness): `/fleet/loads` lists loads, the filter narrows, a row opens the detail, and **+ New Load** shows only with `loads:write`.

- [ ] **Step 4: Commit**

```bash
git add static/fleet/pages/loads.js
git commit -s -m "feat(fleet-ui): migrate loads list to pages/ + gated Create (Phase 4)"
```

---

## Task 4: Load form (top fields + repeatable stops + rate items)

**Files:**
- Modify: `static/fleet/pages/load-form.js`
- Reuse: repeatable-row pattern from `pages/facility-form.js` (contacts: `[data-*-row]` + `[data-*-field="KEY"]`, add/remove via `appendChild`/`.remove()`); object-select pattern from `pages/driver-form.js`; `buildLoadPayload` from Task 1.

- [ ] **Step 1:** Implement `renderLoadForm(id|null)`:
  - If `id`, `GET ${API_BASE}/loads/${id}` and prefill; else blank create.
  - Fetch facilities for the per-stop typeahead/select: `GET ${API_BASE}/facilities` → `[{id, name, address}]`. Each stop offers a facility `<select>` (existing) **plus** name+address inputs used only when "facility not listed" is selected (value `''`).
  - Top fields: `customer_name`* , `customer_ref`, `load_number` (placeholder "auto"), `commodity`, `weight_lbs`, `miles` (placeholder "auto-routed if blank"), `notes`, `tags` (comma input — match the existing driver tags affordance; chips are a known carry-over, not required here).
  - **Stops** repeatable (≥1, seed one blank row on create): `stop_type` select (`pickup`/`delivery`); `service_type` select whose options come from `serviceTypesFor(stop_type)` and **re-populate on `stop_type` change** (clear an now-invalid selection); facility select + name/address inputs; `scheduled_arrive` (`<input type="datetime-local">`)* ; `scheduled_arrive_end`; `timezone` select* — **alias labels, IANA values**, default Central:
    `[{value:'America/New_York',label:'Eastern'},{value:'America/Chicago',label:'Central'},{value:'America/Denver',label:'Mountain'},{value:'America/Phoenix',label:'Arizona'},{value:'America/Los_Angeles',label:'Pacific'},{value:'America/Anchorage',label:'Alaska'},{value:'Pacific/Honolulu',label:'Hawaii'}]`; `expected_dwell_minutes`; detention fields; `notes`. A **Remove** button per row (disabled when only one remains); **+ Add stop**.
  - **Rate items** repeatable: `description` + `amount_usd`, with a live running total; **+ Add rate item**.

- [ ] **Step 2:** On submit, gather DOM into `{ top, stops, rateItems }`, call `buildLoadPayload(...)`. If `errors.length`, show them in the form's `alert--error` and stop. Else `POST ${API_BASE}/loads` (create) or `PUT ${API_BASE}/loads/${id}` (edit) with `JSON.stringify(payload)`.
  - On `res.ok` **and** a body that is a resolution array (`Array.isArray(body) && body.some(r => r.facility_resolution_required)`) → hand off to Task 5's disambiguation (do not treat as success).
  - On `res.ok` and a normal load body → `navigate('load-detail', { id: body.id })`.
  - On `!res.ok` → surface `body.error || HTTP ${res.status}` inline.

- [ ] **Step 3:** Browser-verify create with a single existing-facility stop persists and lands on the detail page; verify edit prefphills and PUT persists a changed `customer_name`.

- [ ] **Step 4: Commit**

```bash
git add static/fleet/pages/load-form.js
git commit -s -m "feat(fleet-ui): load create/edit form with repeatable stops + rate items (Phase 4)"
```

---

## Task 5: Facility disambiguation sub-flow

**Files:**
- Modify: `static/fleet/pages/load-form.js` (uses `applyResolutionChoices` from Task 1)

- [ ] **Step 1:** When submit returns a resolution array, render an inline panel (replace the submit area, keep the form values in memory) listing, **per `stop_index`**, the returned `candidates` (`name`, `address`, `score` as a %) as selectable radio options plus a **"Create new facility"** option (`force_new: true`). Default selection: the top candidate.

- [ ] **Step 2:** On "Confirm & resubmit", build `choices = { [stop_index]: { facility_id } | { force_new: true } }` from the selections, compute `const resolved = applyResolutionChoices(lastPayload, choices)`, and re-POST/PUT `resolved`. A second resolution response (possible if multiple ambiguous stops) re-renders the panel for the still-unresolved stops; a normal body → navigate to detail.

- [ ] **Step 3:** Browser-verify: enter a stop by a name+address close to a seeded facility → the candidate panel appears → picking the candidate resubmits and creates the load against the existing facility; choosing "Create new facility" creates a fresh one (verify via the facilities list).

- [ ] **Step 4: Commit**

```bash
git add static/fleet/pages/load-form.js
git commit -s -m "feat(fleet-ui): inline facility disambiguation step on load save (Phase 4)"
```

---

## Task 6: Load detail page + lifecycle actions + delete

**Files:**
- Modify: `static/fleet/pages/load-detail.js`
- Reference: inline `renderLoadDetailView` in `app.js` (≈ lines 277–530) — port the read-only display verbatim (detail grid, stops table, trips table, rate items, documents, status badge), then add gated actions. Use `confirm.js` for destructive confirms (as facility-detail does).

- [ ] **Step 1:** Port the read-only detail into `renderLoadDetail(id)`: `GET ${API_BASE}/loads/${id}`, render the existing grids/tables, preserve trip-row → `trip-detail` and document-row navigation, plus a **Back** link and an **Edit** button gated on `hasScope('loads:write')` → `navigate('load-edit', { id })`.

- [ ] **Step 2:** Add a gated action bar (each button hidden unless its scope is granted and the load's status permits it):
  - **Cancel** (`loads:write`): prompt for an optional reason → `POST ${API_BASE}/loads/${id}/cancel` `{ reason }`. Only show for pre-delivery statuses (`planned|assigned|dispatched|in_transit`).
  - **Invoice** (`loads:invoice`): prompt optional `invoice_number` → `POST .../invoice` `{ invoice_number }`.
  - **Settle** (`loads:settle`): confirm → `POST .../settle` (no body).
  - **Delete** (`loads:delete`): confirm (permanent — no undo) → `DELETE .../${id}`. On **409**, show the backend `error` ("load has N active trip(s)…") in the confirm dialog's `alert--error` and keep the load; on success → `navigate('loads')`.
  - After any non-delete action, re-fetch and re-render the detail (no optimistic UI).

- [ ] **Step 3:** Browser-verify each action against `cargo run`: cancel sets status `cancelled`; delete of a load with an active trip surfaces the 409; delete of an unreferenced load returns to the list; buttons hidden when scopes are dropped.

- [ ] **Step 4: Commit**

```bash
git add static/fleet/pages/load-detail.js
git commit -s -m "feat(fleet-ui): load detail page + cancel/invoice/settle/delete actions (Phase 4)"
```

---

## Task 7: Remove legacy inline views + full verification

**Files:**
- Modify: `static/fleet/app.js` — delete the inline `renderLoadsView` + `renderLoadDetailView` function bodies (≈ 151–530) now that pages cover them; ensure no dangling references remain.

- [ ] **Step 1:** Delete the two inline functions and any now-unused helpers they alone used. Grep to confirm nothing else calls them: `grep -n "renderLoadsView\|renderLoadDetailView" static/fleet/app.js` → only the import line should remain (which already points at the new module).

- [ ] **Step 2: Run the full JS suite.**

Run: `npm --prefix static/fleet test`
Expected: PASS — prior count (72) **plus** the new `load-form-payload` + router precedence tests.

- [ ] **Step 3: Backend untouched — sanity build only.**

Run: `cargo build`
Expected: success (no Rust changed; used to run the dev server).

- [ ] **Step 4: Playwright E2E** against a local `cargo run` (owner `owner@dev.local`, scopes `*`), mirroring the Phase 1–3 overnight verification. Cover: loads list + filter; **create** with an existing-facility stop → detail; **create** with name+address → disambiguation → resubmit; **edit** persists; **cancel**/**invoice**/**settle** transitions; **delete** blocked by active-trip 409 then allowed once unreferenced; scope-gated buttons hidden when a scope is removed. Capture console for only-benign errors (refresh 401 when logged out, favicon 404, expected 409).

- [ ] **Step 5: Update the progress doc + commit.** Append a "Phase 4 — Loads" section to `docs/superpowers/OVERNIGHT-PROGRESS.md` noting decisions (single hard-delete not two-tier; disambiguation is HTTP 200; tags via comma input; miles async) and verification results.

```bash
git add static/fleet/app.js docs/superpowers/OVERNIGHT-PROGRESS.md
git commit -s -m "refactor(fleet-ui): drop legacy inline loads views; Phase 4 verified"
```

---

## Self-review notes (spec coverage)

- **Top fields, stops (facility typeahead + disambiguation), rate items, running total** → Tasks 1, 4, 5.
- **Detail lifecycle (cancel/invoice/settle) + delete** → Task 6 (delete is single guarded hard-delete per backend, NOT the spec's two-tier — documented deviation).
- **Scope gating** (`loads:{write,invoice,settle,delete}`) → Tasks 3, 6.
- **Naive datetimes + IANA tz + service_type/stop_type validity** → Task 1 helpers, enforced in Task 4 UI.
- **Known carry-overs accepted (consistent with Phases 1–3):** tags via comma input not chips; miles shown pending while async routing completes; no soft-delete/reactivate for loads (backend has none).
