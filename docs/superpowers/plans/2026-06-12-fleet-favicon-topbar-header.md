# Fleet Favicon + Unified Sticky Topbar Header — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a favicon to both web apps and collapse the duplicate header on fleet list views into a single sticky topbar that holds the title plus the page's filters/action buttons.

**Architecture:** A new `#topbar-controls` slot lives inside the fleet shell's fixed `.topbar`. A pair of helpers in `utils/dom.js` (`setTopbarControls`/`clearTopbarControls`) fill and empty it. `app.js`'s single route dispatcher clears the slot on every navigation; list pages repopulate it instead of rendering an in-content `.page-header`. The favicon is a tiny vector `favicon.svg` (the existing white "O"-on-blue monogram) linked from both `index.html` files, with the existing PNGs as `apple-touch-icon`.

**Tech Stack:** Vanilla ES-module SPA (`static/fleet/`), CSS custom-property design tokens, Vitest + happy-dom for tests. No build step.

**Spec:** `docs/superpowers/specs/2026-06-12-fleet-favicon-topbar-header-design.md`

**Conventions for every commit in this plan:**
- Sign off with `git commit -s` (this repo enforces DCO).
- Co-author trailer: `Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>`
- Run tests with: `npm test` (alias for `vitest run`) from the repo root.

---

## File Structure

**Create:**
- `static/fleet/favicon.svg` — vector brand mark (white "O" ring on `#1a56db`).
- `static/driver/favicon.svg` — identical copy.
- `static/fleet/icon-192.png`, `static/fleet/icon-512.png` — copied from `static/driver/` so the fleet app is self-contained.
- `tests/fleet/topbar-controls.test.js` — unit tests for the new `dom.js` helpers.

**Modify:**
- `static/fleet/index.html` — favicon `<link>`s in `<head>`; add `#topbar-controls` slot, reorder `.topbar__actions`.
- `static/driver/index.html` — favicon `<link>`s in `<head>`.
- `static/fleet/utils/dom.js` — add `setTopbarControls` + `clearTopbarControls`.
- `static/fleet/app.js` — call `clearTopbarControls()` in `renderRoute`.
- `static/fleet/pages/_list.js` — emit controls into the topbar slot, drop `.page-header`/`<h1>`.
- `static/fleet/pages/trips.js`, `loads.js`, `events.js` — move hand-built `.page-header` contents into the topbar slot.
- `static/fleet/css/components.css` — `.topbar__controls` styling, compact in-bar control sizing, `.topbar__actions` gap, `.content` top padding; remove dead `.page-header`/`.page-title`/`.page-controls` rules.
- `tests/fleet/maintenance.test.js` — harness gains `#topbar-controls`; select assertion targets the slot.

**Explicitly NOT touched:** `static/driver/sw.js` (cache-stamp governance — see spec).

---

## Task 1: Favicon asset + wiring (both apps)

**Files:**
- Create: `static/fleet/favicon.svg`, `static/driver/favicon.svg`
- Create (copy): `static/fleet/icon-192.png`, `static/fleet/icon-512.png`
- Modify: `static/fleet/index.html`, `static/driver/index.html`

This task is self-contained markup/asset work with no JS logic to unit-test; verification is by file presence + grep.

- [ ] **Step 1: Create the favicon SVG (fleet)**

The monogram is drawn as a geometric ring (renderer-independent at 16px, unlike `<text>` which depends on system fonts). Brand blue `#1a56db` matches the driver `manifest.json` `theme_color` and the existing PNGs.

Create `static/fleet/favicon.svg`:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="14" fill="#1a56db"/>
  <circle cx="32" cy="32" r="15" fill="none" stroke="#ffffff" stroke-width="7"/>
</svg>
```

- [ ] **Step 2: Copy the SVG to the driver app**

Run:
```bash
cp static/fleet/favicon.svg static/driver/favicon.svg
```

- [ ] **Step 3: Copy the PNG icons into the fleet app**

Run:
```bash
cp static/driver/icon-192.png static/fleet/icon-192.png
cp static/driver/icon-512.png static/fleet/icon-512.png
```

- [ ] **Step 4: Link the favicon in the fleet `<head>`**

In `static/fleet/index.html`, the current `<head>` ends with two stylesheet links (lines 7-8) before `</head>`. Add the icon links immediately after the `<title>` line (line 6). Replace:

```html
  <title>Ollie Fleet</title>
  <link rel="stylesheet" href="/fleet/css/base.css?v=2.1.0">
```

with:

```html
  <title>Ollie Fleet</title>
  <link rel="icon" href="/fleet/favicon.svg" type="image/svg+xml">
  <link rel="apple-touch-icon" href="/fleet/icon-192.png">
  <link rel="stylesheet" href="/fleet/css/base.css?v=2.1.0">
```

- [ ] **Step 5: Link the favicon in the driver `<head>`**

In `static/driver/index.html`, replace:

```html
  <link rel="manifest" href="/driver/manifest.json">
```

with:

```html
  <link rel="manifest" href="/driver/manifest.json">
  <link rel="icon" href="/driver/favicon.svg" type="image/svg+xml">
  <link rel="apple-touch-icon" href="/driver/icon-192.png">
```

- [ ] **Step 6: Verify assets and links exist**

Run:
```bash
ls -la static/fleet/favicon.svg static/driver/favicon.svg static/fleet/icon-192.png static/fleet/icon-512.png
grep -n 'rel="icon"\|apple-touch-icon' static/fleet/index.html static/driver/index.html
```
Expected: all four files listed; four grep matches (two per `index.html`).

- [ ] **Step 7: Verify the SVG is well-formed (sanity)**

A parser-free check is enough for a small static asset we author ourselves (avoid stdlib XML parsers, which are XXE-prone). Confirm it opens with `<svg` and the tags balance:

```bash
head -c 64 static/fleet/favicon.svg
grep -c '</svg>' static/fleet/favicon.svg
```
Expected: output starts with `<svg ...`; the grep count is `1`. Visual confirmation is optional; the ring + rounded square is deterministic.

- [ ] **Step 8: Commit**

```bash
git add static/fleet/favicon.svg static/driver/favicon.svg static/fleet/icon-192.png static/fleet/icon-512.png static/fleet/index.html static/driver/index.html
git commit -s -m "feat(ui): add favicon to fleet and driver apps

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 2: Topbar controls slot — shell markup + dom.js helpers

**Files:**
- Modify: `static/fleet/index.html` (topbar markup)
- Modify: `static/fleet/utils/dom.js`
- Test: `tests/fleet/topbar-controls.test.js` (create)

- [ ] **Step 1: Write the failing test**

Create `tests/fleet/topbar-controls.test.js`:

```javascript
import { describe, it, expect, beforeEach } from 'vitest';
import { setTopbarControls, clearTopbarControls } from '../../static/fleet/utils/dom.js';

beforeEach(() => {
  document.body.innerHTML = '<div id="topbar-controls"></div>';
});

describe('topbar controls slot', () => {
  it('setTopbarControls populates the slot via the builder', () => {
    setTopbarControls((slot) => {
      const btn = document.createElement('button');
      btn.id = 'probe';
      slot.appendChild(btn);
    });
    expect(document.querySelector('#topbar-controls #probe')).toBeTruthy();
  });

  it('setTopbarControls clears prior content before building', () => {
    setTopbarControls((slot) => { slot.appendChild(document.createElement('span')); });
    setTopbarControls((slot) => {
      const b = document.createElement('button');
      b.id = 'second';
      slot.appendChild(b);
    });
    const slot = document.getElementById('topbar-controls');
    expect(slot.querySelectorAll('span').length).toBe(0);
    expect(slot.querySelector('#second')).toBeTruthy();
  });

  it('clearTopbarControls empties the slot', () => {
    setTopbarControls((slot) => { slot.appendChild(document.createElement('button')); });
    clearTopbarControls();
    expect(document.getElementById('topbar-controls').children.length).toBe(0);
  });

  it('helpers no-op safely when the slot is absent', () => {
    document.body.innerHTML = '';
    expect(() => clearTopbarControls()).not.toThrow();
    expect(() => setTopbarControls(() => {})).not.toThrow();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- tests/fleet/topbar-controls.test.js`
Expected: FAIL — `setTopbarControls`/`clearTopbarControls` are not exported from `dom.js`.

- [ ] **Step 3: Add the helpers to `dom.js`**

In `static/fleet/utils/dom.js`, append after `setRefreshIndicator` (end of file):

```javascript
/** Empty the topbar controls slot. Safe no-op if the slot is absent. */
export function clearTopbarControls() {
  const el = document.getElementById('topbar-controls');
  if (el) el.replaceChildren();
}

/**
 * Populate the topbar controls slot. Clears it first, then calls
 * `builderFn(slotEl)` so the caller can append its filter/select/buttons.
 * Safe no-op if the slot is absent.
 */
export function setTopbarControls(builderFn) {
  const el = document.getElementById('topbar-controls');
  if (!el) return;
  el.replaceChildren();
  if (builderFn) builderFn(el);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- tests/fleet/topbar-controls.test.js`
Expected: PASS (4 tests).

- [ ] **Step 5: Add the slot to the shell markup**

In `static/fleet/index.html`, replace the topbar `.topbar__actions` block (currently the refresh indicator alone):

```html
      <div class="topbar__actions">
        <span class="refresh-indicator" id="refresh-indicator"></span>
      </div>
```

with (refresh indicator first, controls slot second — actions anchor the far right):

```html
      <div class="topbar__actions">
        <span class="refresh-indicator" id="refresh-indicator"></span>
        <div class="topbar__controls" id="topbar-controls"></div>
      </div>
```

- [ ] **Step 6: Commit**

```bash
git add static/fleet/utils/dom.js static/fleet/index.html tests/fleet/topbar-controls.test.js
git commit -s -m "feat(fleet): add topbar controls slot + dom helpers

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 3: Clear the slot on every navigation

**Files:**
- Modify: `static/fleet/app.js` (`renderRoute`, import line)

- [ ] **Step 1: Import the clear helper**

In `static/fleet/app.js`, the existing import from `./utils/dom.js` (around line 16) is:

```javascript
import {
  setRefreshIndicator,
} from './utils/dom.js';
```

Change it to:

```javascript
import {
  setRefreshIndicator, clearTopbarControls,
} from './utils/dom.js';
```

- [ ] **Step 2: Call it in `renderRoute`**

In `static/fleet/app.js`, `renderRoute` currently has (around lines 128-130):

```javascript
  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = VIEW_TITLES[name] || name;
  setRefreshIndicator('');
```

Add the clear call immediately after `setRefreshIndicator('')`:

```javascript
  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = VIEW_TITLES[name] || name;
  setRefreshIndicator('');
  clearTopbarControls();
```

- [ ] **Step 3: Verify the smoke test still passes**

Run: `npm test -- tests/fleet/smoke.test.js`
Expected: PASS (no regressions in boot/route wiring).

- [ ] **Step 4: Commit**

```bash
git add static/fleet/app.js
git commit -s -m "feat(fleet): clear topbar controls on each navigation

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 4: Move `renderEntityList` controls into the topbar

This is the shared helper behind drivers, trucks, trailers, facilities, maintenance, terminals, documents list pages. After this task those pages render their Create button + `extraControls` into the topbar and no longer emit a `.page-title`.

**Files:**
- Modify: `static/fleet/pages/_list.js`
- Test: reuse `tests/fleet/maintenance.test.js` (updated in Task 7) + add a focused `_list` test below.

- [ ] **Step 1: Write the failing test**

Create `tests/fleet/list.test.js`:

```javascript
import { describe, it, expect, beforeEach } from 'vitest';
import { clearMe } from '../../static/fleet/utils/api.js';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { renderEntityList } from '../../static/fleet/pages/_list.js';

beforeEach(() => {
  document.body.innerHTML =
    '<div id="topbar-controls"></div><div id="main-content"></div>';
  localStorage.clear();
  clearMe();
  saveToken('test-token');
});

describe('renderEntityList', () => {
  const cols = [{ header: 'Name', cell: r => r.name }];
  const rows = [{ id: '1', name: 'Alpha' }];

  it('does not emit an in-content page title or page-header', () => {
    renderEntityList({ title: 'Widgets', columns: cols, rows, detailView: 'x' });
    const main = document.getElementById('main-content').innerHTML;
    expect(main).not.toContain('page-header');
    expect(main).not.toContain('page-title');
    expect(main).not.toContain('Widgets');
  });

  it('renders the table rows into #main-content', () => {
    renderEntityList({ title: 'Widgets', columns: cols, rows, detailView: 'x' });
    expect(document.getElementById('main-content').innerHTML).toContain('Alpha');
  });

  it('puts extraControls into #topbar-controls (not main-content)', () => {
    renderEntityList({
      title: 'Widgets', columns: cols, rows, detailView: 'x',
      extraControls: (slot) => {
        const s = document.createElement('select');
        s.id = 'probe-filter';
        slot.appendChild(s);
      },
    });
    expect(document.querySelector('#topbar-controls #probe-filter')).toBeTruthy();
    expect(document.querySelector('#main-content #probe-filter')).toBeFalsy();
  });

  it('shows the empty state in #main-content when there are no rows', () => {
    renderEntityList({
      title: 'Widgets', columns: cols, rows: [], detailView: 'x',
      emptyText: 'Nothing here.',
    });
    expect(document.getElementById('main-content').innerHTML).toContain('Nothing here.');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- tests/fleet/list.test.js`
Expected: FAIL — current `_list.js` emits `page-header`/`page-title` into `#main-content` and appends `extraControls` there.

- [ ] **Step 3: Rewrite `renderEntityList`**

Replace the entire body of `static/fleet/pages/_list.js` with:

```javascript
import { escHtml } from '../utils/format.js';
import { renderTable } from '../components/table.js';
import { hasScope } from '../utils/api.js';
import { setContent, navigate, setTopbarControls } from '../utils/dom.js';

/**
 * Render a standard entity list page. The title is shown by the shell topbar
 * (set in app.js from VIEW_TITLES); this helper puts the page's filters
 * (extraControls) and the scope-gated Create button into the topbar controls
 * slot, and renders only the table into the content area.
 *
 * opts: { title, columns, rows, detailView,
 *         createView, createScope, createLabel, emptyText,
 *         rowClass, extraControls }
 * `title` is accepted for call-site compatibility but not rendered in-content.
 */
export function renderEntityList({
  columns, rows, detailView,
  createView, createScope, createLabel, emptyText,
  rowClass, extraControls,
}) {
  // Filters first, primary action (Create) last so it anchors the far right.
  setTopbarControls((slot) => {
    if (extraControls) extraControls(slot);
    if (createView && hasScope(createScope)) {
      const btn = document.createElement('button');
      btn.className = 'btn btn--primary';
      btn.textContent = createLabel || '+ Create';
      btn.addEventListener('click', () => navigate(createView));
      slot.appendChild(btn);
    }
  });

  setContent('<div id="list-table"></div>');

  const tableEl = document.getElementById('list-table');
  if (!rows.length) {
    tableEl.innerHTML = `<div class="state-empty">${escHtml(emptyText || 'No records found.')}</div>`;
    return;
  }
  renderTable(tableEl, {
    columns,
    rows,
    onRowClick: (id) => navigate(detailView, { id }),
    rowClass,
  });
}
```

Note: `extraControls` runs before the Create button, so `maintenance.js`'s `insertBefore(..., slot.firstChild)` calls still produce `[type][unit][category]` followed by the Create button — order preserved.

- [ ] **Step 4: Run the test to verify it passes**

Run: `npm test -- tests/fleet/list.test.js`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/pages/_list.js tests/fleet/list.test.js
git commit -s -m "feat(fleet): render entity-list controls in the topbar slot

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 5: Move the one-off list headers (trips, loads) into the topbar

`trips.js` and `loads.js` build their `.page-header` by hand (not via `renderEntityList`). Move their status filter + Create button into the topbar slot.

**Files:**
- Modify: `static/fleet/pages/trips.js`
- Modify: `static/fleet/pages/loads.js`
- Test: `tests/fleet/trips-loads-controls.test.js` (create)

- [ ] **Step 1: Write the failing test**

Create `tests/fleet/trips-loads-controls.test.js`:

```javascript
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { clearMe } from '../../static/fleet/utils/api.js';
import { saveToken } from '../../static/fleet/utils/auth.js';

function jsonResponse(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

async function seedScopes(fetchMock) {
  const { loadMe } = await import('../../static/fleet/utils/api.js');
  fetchMock.mockResolvedValueOnce(jsonResponse({
    fleet_user_id: 'u1', name: 'T', email: 't@x.com', role: 'owner',
    effective_scopes: ['*'],
  }));
  await loadMe();
}

beforeEach(() => {
  document.body.innerHTML =
    '<div id="topbar-controls"></div><div id="main-content"></div>';
  localStorage.clear();
  clearMe();
  saveToken('test-token');
  vi.restoreAllMocks();
});
afterEach(() => vi.restoreAllMocks());

describe('trips list header', () => {
  it('renders the status filter + New Trip in the topbar, table in content', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ items: [] }));

    const { renderTripsView } = await import('../../static/fleet/pages/trips.js');
    await renderTripsView({});
    await Promise.resolve();

    expect(document.querySelector('#topbar-controls #trip-status-filter')).toBeTruthy();
    expect(document.querySelector('#topbar-controls #new-trip')).toBeTruthy();
    const main = document.getElementById('main-content').innerHTML;
    expect(main).not.toContain('page-title');
    expect(main).toContain('Trip #'); // table header still in content
  });
});

describe('loads list header', () => {
  it('renders the status filter + New Load in the topbar, table in content', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ loads: [] }));

    const { renderLoadsView } = await import('../../static/fleet/pages/loads.js');
    await renderLoadsView({});
    await Promise.resolve();

    expect(document.querySelector('#topbar-controls #status-filter')).toBeTruthy();
    expect(document.querySelector('#topbar-controls #new-load')).toBeTruthy();
    const main = document.getElementById('main-content').innerHTML;
    expect(main).not.toContain('page-title');
    expect(main).toContain('Load #');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- tests/fleet/trips-loads-controls.test.js`
Expected: FAIL — controls are currently in `#main-content`'s `.page-header`, not `#topbar-controls`.

- [ ] **Step 3: Update `trips.js`**

In `static/fleet/pages/trips.js`, add `setTopbarControls` to the import on line 3:

```javascript
import { setContent, navigate, setTopbarControls } from '../utils/dom.js';
```

Replace the `setContent(...)` template (lines 51-59) — which currently includes the `.page-header` — with a topbar-controls call plus a content template that starts at the table:

```javascript
    setTopbarControls((slot) => { slot.innerHTML = `${selectHtml}${createBtn}`; });

    setContent(`
      <div class="table-wrapper">
        <table class="data-table">
          <thead><tr><th>Trip #</th><th>Load #</th><th>Status</th><th>Driver</th><th>Route</th><th>Pickup</th><th>Delivery</th></tr></thead>
          <tbody id="trips-tbody">${rows}</tbody>
        </table>
      </div>
    `);
```

The handler wiring below it is unchanged — `document.getElementById('new-trip')` and `#trip-status-filter` now resolve inside the topbar slot, but `getElementById`/`querySelectorAll` are document-wide so they still match.

- [ ] **Step 4: Update `loads.js`**

In `static/fleet/pages/loads.js`, add `setTopbarControls` to the import on line 3:

```javascript
import { setContent, navigate, setTopbarControls } from '../utils/dom.js';
```

`loads.js` builds its markup in `buildContent` (returns an HTML string) and the caller does `setContent(buildContent(...))`. The `.page-header` lives inside that returned string (lines 67-74). Two edits:

(a) In `buildContent`, change the `return` template so it no longer contains the `.page-header` — keep the cap banner and table only:

```javascript
    return `
      ${capBanner}<div class="table-wrapper">
        <table class="data-table">
          <thead>
            <tr>
              <th>Load #</th>
              <th>Status</th>
              <th>Customer</th>
              <th>Route</th>
              <th>Pickup</th>
              <th>Delivery</th>
            </tr>
          </thead>
          <tbody id="loads-tbody">
            ${rows}
          </tbody>
        </table>
      </div>
    `;
```

(b) In `fetchAndRender`, right after `setContent(buildContent(loads, status, capTotal));` (line 107), inject the controls into the topbar. The `selectHtml`/`createBtn` strings are built inside `buildContent`'s scope, so lift them: change `buildContent` to also return the controls markup, or rebuild here. Simplest — rebuild the controls inline in `fetchAndRender` using the same data. Replace line 107:

```javascript
      setContent(buildContent(loads, status, capTotal));
```

with:

```javascript
      setContent(buildContent(loads, status, capTotal));

      const statusOptions = [
        '', 'planned', 'assigned', 'dispatched', 'in_transit',
        'delivered', 'invoiced', 'settled', 'cancelled',
      ];
      const filterStatus = status || '';
      const selectHtml = `
        <select class="form-select" id="status-filter">
          ${statusOptions.map(s =>
            `<option value="${s}" ${s === filterStatus ? 'selected' : ''}>${s || 'All Statuses'}</option>`
          ).join('')}
        </select>
      `;
      const createBtn = hasScope('loads:write')
        ? `<button class="btn btn--primary" id="new-load">+ New Load</button>`
        : '';
      setTopbarControls((slot) => { slot.innerHTML = `${selectHtml}${createBtn}`; });
```

Then remove the now-duplicated `statusOptions`/`selectHtml`/`createBtn` declarations from inside `buildContent` (lines 19-34) since `buildContent` no longer renders them. `buildContent` keeps only the cap-banner, sort, and rows logic.

- [ ] **Step 5: Run test to verify it passes**

Run: `npm test -- tests/fleet/trips-loads-controls.test.js`
Expected: PASS (2 tests).

- [ ] **Step 6: Run the full suite to catch regressions**

Run: `npm test`
Expected: PASS. If any pre-existing trips/loads test asserted on `.page-title`, update it to match the topbar-slot location (mirror the assertions in the new test).

- [ ] **Step 7: Commit**

```bash
git add static/fleet/pages/trips.js static/fleet/pages/loads.js tests/fleet/trips-loads-controls.test.js
git commit -s -m "feat(fleet): move trips/loads list controls into the topbar

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 6: Move the events header into the topbar

`events.js` has a richer header: a static "Auto-refreshes every 30s" hint and a "Needs attention only" toggle. The topbar refresh indicator already shows "Updated <time>" (set in `fetchAndRenderEvents`), so the static hint is redundant — drop it and move the toggle into the topbar slot.

**Files:**
- Modify: `static/fleet/pages/events.js`
- Test: `tests/fleet/events.test.js` (extend)

- [ ] **Step 1: Write the failing test**

Append to `tests/fleet/events.test.js` (inside the top-level `describe` or a new one). First check the file's existing imports; it imports named helpers from `events.js`. Add this block:

```javascript
import { vi as _vi } from 'vitest';

describe('renderEventsView header', () => {
  beforeEach(() => {
    document.body.innerHTML =
      '<div id="topbar-controls"></div><div id="main-content"></div>';
    localStorage.clear();
  });

  it('puts the attention toggle in the topbar, list in content, no page-title', async () => {
    const fetchMock = _vi.fn().mockResolvedValue({
      ok: true, status: 200, json: async () => ({ events: [] }),
    });
    _vi.stubGlobal('fetch', fetchMock);
    const { saveToken } = await import('../../static/fleet/utils/auth.js');
    saveToken('test-token');

    const { renderEventsView } = await import('../../static/fleet/pages/events.js');
    await renderEventsView();
    await Promise.resolve();

    expect(document.querySelector('#topbar-controls #events-attention')).toBeTruthy();
    const main = document.getElementById('main-content').innerHTML;
    expect(main).not.toContain('page-title');
    expect(document.getElementById('events-list')).toBeTruthy();
    _vi.restoreAllMocks();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- tests/fleet/events.test.js`
Expected: FAIL — `#events-attention` is currently inside the in-content `.page-header`.

- [ ] **Step 3: Update `renderEventsView`**

In `static/fleet/pages/events.js`, add `setTopbarControls` to the import on line 3:

```javascript
import { setContent, setRefreshIndicator, setTopbarControls } from '../utils/dom.js';
```

Replace the `setContent(...)` skeleton (lines 139-148) — currently the `.page-header` plus the list container — with a topbar-controls call plus a content template holding only the list:

```javascript
  // Attention toggle lives in the topbar; the auto-refresh status is already
  // surfaced by the topbar refresh indicator (set in fetchAndRenderEvents).
  setTopbarControls((slot) => {
    slot.innerHTML =
      '<button id="events-attention" class="btn btn--ghost" aria-pressed="false">Needs attention only</button>';
  });

  setContent(`
    <div class="events-list" id="events-list">
      <div class="state-loading"><div class="spinner"></div></div>
    </div>
  `);
```

The rest of `renderEventsView` (the `attachEventHandlers`, the `getElementById('events-attention')` wiring, the 30s interval) is unchanged — `getElementById` resolves the button in the topbar slot.

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- tests/fleet/events.test.js`
Expected: PASS (existing tests + the new one).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/pages/events.js tests/fleet/events.test.js
git commit -s -m "feat(fleet): move events attention toggle into the topbar

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 7: Update the maintenance test harness

`maintenance.test.js` builds the shell DOM as only `<div id="main-content">` and asserts its filter `<select>` lands in `#main-content` (line 90). After Task 4 the selects render into `#topbar-controls`.

**Files:**
- Modify: `tests/fleet/maintenance.test.js`

- [ ] **Step 1: Add the slot to the harness**

In `tests/fleet/maintenance.test.js`, line 21:

```javascript
  document.body.innerHTML = '<div id="main-content"></div>';
```

becomes:

```javascript
  document.body.innerHTML =
    '<div id="topbar-controls"></div><div id="main-content"></div>';
```

- [ ] **Step 2: Retarget the select assertion**

In the same file, line 90:

```javascript
    const selects = document.querySelectorAll('#main-content select');
```

becomes:

```javascript
    const selects = document.querySelectorAll('#topbar-controls select');
```

(The `document.querySelector('select[aria-label="..."]')` lookups on lines 129 are document-wide and need no change.)

- [ ] **Step 3: Run the maintenance test**

Run: `npm test -- tests/fleet/maintenance.test.js`
Expected: PASS. The list-row assertions (`alternator`, `$412.50`) still target `#main-content` and pass because rows render there; only the filter `<select>` moved.

- [ ] **Step 4: Commit**

```bash
git add tests/fleet/maintenance.test.js
git commit -s -m "test(fleet): point maintenance filter assertion at topbar slot

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 8: CSS — style the slot, tighten spacing, remove dead rules

**Files:**
- Modify: `static/fleet/css/components.css`

No unit test (CSS); verify by grep + a Playwright visual check at the end.

- [ ] **Step 1: Add `.topbar__controls` + compact in-bar sizing**

In `static/fleet/css/components.css`, after the `.topbar__actions` rule (ends at line 262), add:

```css
.topbar__controls {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  flex-wrap: nowrap;
}

/* Compact control sizing so filters/buttons fit the 48px (--space-8) bar. */
.topbar__controls .form-select,
.topbar__controls .btn {
  height: 32px;
  padding: 0 var(--space-3);
  font-size: 0.8125rem;
}
```

- [ ] **Step 2: Widen the actions gap**

In the `.topbar__actions` rule (lines 258-262), change `gap: var(--space-3);` to `gap: var(--space-4);` so the refresh status isn't cramped against the first control.

- [ ] **Step 3: Tighten the content top padding only**

In the `.content` rule (lines 265-271), the current line is `padding: var(--space-5);`. Replace it with an explicit top override while keeping side/bottom gutters:

```css
  padding: var(--space-3) var(--space-5) var(--space-5);
```

(top = 12px, right/left = 24px, bottom = 24px.)

- [ ] **Step 4: Remove the now-dead page-header rules**

The `.page-header`, `.page-title`, and `.page-controls` rules (lines 535-556, the "Page Header" section) are no longer referenced after Tasks 4-6. Verify and remove.

Run first:
```bash
grep -rn 'page-header\|page-title\|page-controls' static/fleet/
```
Expected: matches ONLY in `static/fleet/css/components.css` (the rules themselves). If any `.js`/`.html` still references them, that file was missed in an earlier task — fix it before deleting the rules.

Then delete the three rules (the `/* ===== Page Header ===== */` block through the end of `.page-controls`).

- [ ] **Step 5: Verify no dangling references**

Run:
```bash
grep -rn 'page-header\|page-title\|page-controls' static/fleet/
```
Expected: no matches.

- [ ] **Step 6: Commit**

```bash
git add static/fleet/css/components.css
git commit -s -m "style(fleet): topbar controls styling + tighter list top spacing

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 9: Full verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full JS test suite**

Run: `npm test`
Expected: PASS — all fleet tests green (the ~190 existing plus the new `topbar-controls`, `list`, `trips-loads-controls`, extended `events`, updated `maintenance`).

- [ ] **Step 2: Lint/format check (match existing tooling)**

Run:
```bash
node --check static/fleet/pages/_list.js
node --check static/fleet/pages/trips.js
node --check static/fleet/pages/loads.js
node --check static/fleet/pages/events.js
node --check static/fleet/utils/dom.js
node --check static/fleet/app.js
```
Expected: no output (all parse clean).

- [ ] **Step 3: Visual smoke check with Playwright (optional but recommended)**

If the app can be run locally, navigate to `/fleet/trips`, `/fleet/loads`, `/fleet/maintenance`, `/fleet/facilities`, `/fleet/events` and confirm for each: title appears once (topbar only), filters + Create button sit in the bar with the refresh indicator to their left, the table starts directly under the bar with no large gap, and the favicon shows in the browser tab. Screenshot each and compare against the approved mockup (`docs/superpowers/specs/2026-06-12-...`).

- [ ] **Step 4: Confirm clean tree**

Run: `git status --short`
Expected: empty (all work committed).

---

## Self-Review Notes (author)

- **Spec coverage:** Favicon (Task 1) ✓; topbar slot + helpers (Task 2) ✓; clear-on-nav (Task 3) ✓; `_list.js` refactor (Task 4) ✓; one-off pages trips/loads (Task 5) + events (Task 6) ✓; CSS slot styling, gap, top-padding, dead-rule removal (Task 8) ✓; tests including the maintenance harness fix (Tasks 2,4,5,6,7) ✓; `sw.js` deliberately untouched (noted in Task 1 file list + spec) ✓.
- **Control order:** `extraControls` before Create button in `_list.js` (Task 4 Step 3) yields filters-then-action, action rightmost — consistent with trips/loads ordering and the approved mockup. `maintenance.js`'s `insertBefore(firstChild)` still resolves correctly because Create is appended after.
- **Naming consistency:** `setTopbarControls` / `clearTopbarControls` used identically across `dom.js`, `app.js`, `_list.js`, `trips.js`, `loads.js`, `events.js`, and all tests.
