# Fleet UI CRUD — Phase 0b-ii (pushState Router + Read-only View Migration) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the fleet SPA to ES modules + real (pushState) URLs, wire in the 0b-i library (scope store, login gate), and migrate the read-only views to `pages/` modules — while the not-yet-migrated entity views keep working via a legacy shim.

**Architecture:** A new testable `router.js` maps URL paths → `{name, params}` via a route table. `app.js` becomes the ES-module entry: it imports the 0b-i utils (deleting its now-duplicate local helpers, so the still-inline legacy entity views keep working by name), boots with a `/me` scope load + login gate, and dispatches each route to either a migrated `pages/` module (read-only views) or a legacy inline render fn (loads/trips/drivers/terminals). Real URLs work because the fleet static mount already SPA-falls-back to index.html.

**Tech Stack:** Vanilla ES modules, Vitest + happy-dom (router unit tests), Playwright (app-run verification of the refactor). The 0b-i library (`utils/`, `components/`) is the dependency.

---

## Scope notes

- **Read-only views migrated here:** home, events, documents (+ document detail), account, login/setup. **Entity views stay as legacy inline render fns in app.js** (loads, load-detail, drivers, driver-detail, trips, trip-detail, terminals) — each is replaced by a real `pages/` module + CRUD in its own later phase (1, 2, 4, 5).
- **Trucks/Trailers/Facilities** nav entries are added to the sidebar but route to a small "coming soon" placeholder page until their phases — so the new nav doesn't dead-end.
- This phase is DOM/integration-heavy. The router is unit-tested; the app.js refactor is verified by **running the app** (Playwright via the `verify`/`run` skills) — confirm login, navigation, back-button, and that each route still renders.
- `app.js` currently duplicates helpers that now live in 0b-i modules (`apiFetch`, `tryRefresh`, token/JWT helpers, `escHtml`/`badge`/`fmt*`/`shortId`/`humanizeEventType`). We DELETE the duplicates and import them — the legacy view fns call the same names, so they keep working.

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `static/fleet/router.js` | pure `matchRoute(path)` + `navigate`/`startRouter` (pushState) | Create |
| `static/fleet/index.html` | `<script type="module">`; sidebar nav (Equipment group, Facilities) | Modify |
| `static/fleet/app.js` | ESM entry: imports, boot, scope load, login gate, route dispatch; legacy entity views remain inline | Modify (large) |
| `static/fleet/pages/home.js` | home view (moved) | Create |
| `static/fleet/pages/events.js` | events view (moved) | Create |
| `static/fleet/pages/documents.js` | documents list (moved) | Create |
| `static/fleet/pages/document-detail.js` | document detail (moved) | Create |
| `static/fleet/pages/account.js` | account view (moved) | Create |
| `static/fleet/pages/login.js` | login + setup forms (moved) | Create |
| `static/fleet/pages/placeholder.js` | "coming soon" page for trucks/trailers/facilities | Create |
| `tests/fleet/router.test.js` | router matchRoute unit tests | Create |

---

## URL scheme (this phase wires these routes)

| Path | Handler |
|---|---|
| `/fleet` , `/fleet/` | redirect → `/fleet/home` |
| `/fleet/home` | pages/home |
| `/fleet/loads` , `/fleet/loads/{id}` | legacy loads / load-detail |
| `/fleet/trips` , `/fleet/trips/{id}` | legacy trips / trip-detail |
| `/fleet/drivers` , `/fleet/drivers/{id}` | legacy drivers / driver-detail |
| `/fleet/terminals` | legacy terminals |
| `/fleet/trucks` , `/fleet/trailers` , `/fleet/facilities` | placeholder (until their phases) |
| `/fleet/events` | pages/events |
| `/fleet/documents` , `/fleet/documents/{id}` | pages/documents / document-detail |
| `/fleet/account` | pages/account |

(Entity create/edit subroutes like `/fleet/loads/new` are added by each entity phase, not here.)

---

## Task 1: `router.js` — testable pushState router

**Files:**
- Create: `static/fleet/router.js`
- Test: `tests/fleet/router.test.js`

- [ ] **Step 1: Write the failing test**

Create `tests/fleet/router.test.js`:

```js
import { describe, it, expect } from 'vitest';
import { matchRoute, ROUTES } from '../../static/fleet/router.js';

describe('matchRoute', () => {
  it('matches a bare list route', () => {
    expect(matchRoute('/fleet/home')).toEqual({ name: 'home', params: {} });
    expect(matchRoute('/fleet/loads')).toEqual({ name: 'loads', params: {} });
  });
  it('matches a detail route and captures the id', () => {
    expect(matchRoute('/fleet/loads/abc-123')).toEqual({ name: 'load-detail', params: { id: 'abc-123' } });
    expect(matchRoute('/fleet/documents/doc-9')).toEqual({ name: 'document-detail', params: { id: 'doc-9' } });
  });
  it('treats bare /fleet and /fleet/ as home', () => {
    expect(matchRoute('/fleet')).toEqual({ name: 'home', params: {} });
    expect(matchRoute('/fleet/')).toEqual({ name: 'home', params: {} });
  });
  it('maps placeholder entity routes', () => {
    expect(matchRoute('/fleet/trucks')).toEqual({ name: 'trucks', params: {} });
    expect(matchRoute('/fleet/facilities')).toEqual({ name: 'facilities', params: {} });
  });
  it('returns notfound for an unknown path', () => {
    expect(matchRoute('/fleet/nope/x/y')).toEqual({ name: 'notfound', params: {} });
  });
  it('ignores a trailing query string', () => {
    expect(matchRoute('/fleet/loads?status=planned')).toEqual({ name: 'loads', params: { query: 'status=planned' } });
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- router`
Expected: FAIL — module not found.

- [ ] **Step 3: Create `static/fleet/router.js`**

```js
// Route table: each entry maps a name to a path regex. Detail routes capture id.
// `query` (raw query string) is surfaced in params when present.
export const ROUTES = [
  { name: 'home',            re: /^\/fleet\/home$/ },
  { name: 'loads',           re: /^\/fleet\/loads$/ },
  { name: 'load-detail',     re: /^\/fleet\/loads\/([^/]+)$/, id: true },
  { name: 'trips',           re: /^\/fleet\/trips$/ },
  { name: 'trip-detail',     re: /^\/fleet\/trips\/([^/]+)$/, id: true },
  { name: 'drivers',         re: /^\/fleet\/drivers$/ },
  { name: 'driver-detail',   re: /^\/fleet\/drivers\/([^/]+)$/, id: true },
  { name: 'terminals',       re: /^\/fleet\/terminals$/ },
  { name: 'trucks',          re: /^\/fleet\/trucks$/ },
  { name: 'trailers',        re: /^\/fleet\/trailers$/ },
  { name: 'facilities',      re: /^\/fleet\/facilities$/ },
  { name: 'events',          re: /^\/fleet\/events$/ },
  { name: 'documents',       re: /^\/fleet\/documents$/ },
  { name: 'document-detail', re: /^\/fleet\/documents\/([^/]+)$/, id: true },
  { name: 'account',         re: /^\/fleet\/account$/ },
];

/** Pure: map a path (with optional ?query) to { name, params }. */
export function matchRoute(rawPath) {
  const qIdx = rawPath.indexOf('?');
  const path = qIdx === -1 ? rawPath : rawPath.slice(0, qIdx);
  const query = qIdx === -1 ? '' : rawPath.slice(qIdx + 1);

  if (path === '/fleet' || path === '/fleet/') return { name: 'home', params: {} };

  for (const r of ROUTES) {
    const m = path.match(r.re);
    if (m) {
      const params = {};
      if (r.id) params.id = m[1];
      if (query) params.query = query;
      return { name: r.name, params };
    }
  }
  return { name: 'notfound', params: {} };
}

/** pushState navigate, then run the registered handler. */
let _onRoute = () => {};
export function navigate(path) {
  history.pushState({}, '', path);
  _onRoute(matchRoute(path));
}
export function replaceNavigate(path) {
  history.replaceState({}, '', path);
  _onRoute(matchRoute(path));
}

/** Wire popstate + intercept same-origin /fleet link clicks; fire onRoute now. */
export function startRouter(onRoute) {
  _onRoute = onRoute;
  window.addEventListener('popstate', () => _onRoute(matchRoute(location.pathname + location.search)));
  document.addEventListener('click', (e) => {
    const a = e.target.closest && e.target.closest('a[data-link]');
    if (!a) return;
    const href = a.getAttribute('href');
    if (href && href.startsWith('/fleet')) {
      e.preventDefault();
      navigate(href);
    }
  });
  _onRoute(matchRoute(location.pathname + location.search));
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- router`
Expected: PASS (all matchRoute tests).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/router.js tests/fleet/router.test.js
git commit -m "feat(fleet-ui): add testable pushState router"
```

---

## Task 2: Convert app.js + index.html to ES modules (import shared utils, drop duplicates)

This is the structural change. After it, the app still uses its hash router (untouched in this task) — we only swap the helper SOURCES so nothing breaks. Verified by running the app.

**Files:**
- Modify: `static/fleet/index.html` (script tag → module)
- Modify: `static/fleet/app.js` (top: add imports; delete duplicate helper defs)

- [ ] **Step 1: Make index.html load app.js as a module**

In `static/fleet/index.html`, change the script tag (currently `<script src="/fleet/app.js?v=2.0.0"></script>`) to:

```html
<script type="module" src="/fleet/app.js?v=2.0.0"></script>
```

- [ ] **Step 2: Add imports at the very top of app.js**

Prepend to `static/fleet/app.js`:

```js
import {
  getToken, saveToken, clearToken, isAuthenticated,
} from './utils/auth.js';
import {
  apiFetch, tryRefresh, API_BASE, AUTH_BASE, loadMe, getScopes, hasScope, clearMe, setOnUnauthorized,
} from './utils/api.js';
import {
  escHtml, badge, shortId, fmtDate, fmtArrivalWindow, fmtBytes, fmtUSD, fmtMiles, humanizeEventType,
} from './utils/format.js';
```

- [ ] **Step 3: Delete the now-duplicate definitions from app.js**

Remove these local definitions (they are now imported; the legacy view fns call them by the same name):
- The `const TOKEN_KEY`, `getToken`, `saveToken`, `clearToken`, `decodeJwtPayload`, `isTokenExpired`, `isAuthenticated` block.
- `const API_BASE`, `const AUTH_BASE` consts.
- `tryRefresh`, `apiFetch`.
- `badge`, `shortId`, `fmtDate`, `fmtArrivalWindow`, `fmtBytes`, `fmtUSD`, `fmtMiles`, `escHtml`, `humanizeEventType`.

Keep `API_KEYS_BASE` (still local). Keep everything else (views, router, boot). If a deleted helper had subtle differences from the imported version, the imported version is authoritative (they were extracted verbatim in 0b-i) — do NOT re-add a local copy.

- [ ] **Step 4: Verify the app still builds/loads (no console errors)**

Use the `verify` or `run` skill (Playwright) to launch the app and load `/fleet`. Confirm: the page loads, no "X is not defined" / duplicate-declaration console errors, login renders. (No router change yet — hash routing still active.)

Run the JS suite to ensure nothing regressed: `npm test`
Expected: all green (the 0b-i tests don't touch app.js, but confirm no accidental breakage).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/index.html static/fleet/app.js
git commit -m "refactor(fleet-ui): load app.js as ESM, import shared utils, drop duplicate helpers"
```

---

## Task 3: Replace the hash router with the pushState router; update the sidebar

**Files:**
- Modify: `static/fleet/app.js` (router section + boot + sidebar wiring)
- Modify: `static/fleet/index.html` (sidebar nav: `data-view` → `<a data-link href>`, add Equipment group + Facilities)
- Create: `static/fleet/pages/placeholder.js`

- [ ] **Step 1: Add the placeholder page**

Create `static/fleet/pages/placeholder.js`:

```js
import { escHtml } from '../utils/format.js';

export function renderPlaceholder(container, name) {
  container.innerHTML = `<div class="state-empty">
    <p>${escHtml(name)} management is coming in a later release.</p>
  </div>`;
}
```

- [ ] **Step 2: Convert sidebar nav to links in index.html**

In `static/fleet/index.html`, replace the `<button class="sidebar__link" data-view="...">` nav items with `<a class="sidebar__link" data-link href="/fleet/...">` items, and add the Equipment group + Facilities. Order: Home, Loads, Trips, Drivers, Facilities, Equipment(▸ Trucks, Trailers), Events, Documents, Terminals, Account. Example for two items:

```html
<a class="sidebar__link" data-link href="/fleet/home"><span>Home</span></a>
<a class="sidebar__link" data-link href="/fleet/loads"><span>Loads</span></a>
<!-- ... drivers, facilities ... -->
<div class="sidebar__group-label">Equipment</div>
<a class="sidebar__link" data-link href="/fleet/trucks"><span>Trucks</span></a>
<a class="sidebar__link" data-link href="/fleet/trailers"><span>Trailers</span></a>
<!-- ... events, documents, terminals, account ... -->
```

Keep the `href` as the source of truth (drop `data-view`).

- [ ] **Step 3: Replace the router internals in app.js**

In `static/fleet/app.js`:
- Add `import { matchRoute, navigate, replaceNavigate, startRouter } from './router.js';` at the top.
- Add `import { renderPlaceholder } from './pages/placeholder.js';`.
- DELETE the hash-router pieces: `VIEW_TITLES` may stay (reuse for titles); delete `encodeViewHash`, `decodeViewHash`, and the `_renderView` body's hash-history logic. Replace `navigate(view, params)` / `goBack` usages.
- Add a single `renderRoute({ name, params })` dispatcher that: updates the topbar title (`VIEW_TITLES[name] || name`), sets the active sidebar link by matching `href` to `location.pathname`, clears the refresh indicator, revokes any active object URL, and switches on `name` to call the right render fn. Read-only names (`home`,`events`,`documents`,`document-detail`,`account`) call the new page modules (Task 5 wires these — until then, call the existing inline fns); entity names call the legacy inline fns; `trucks`/`trailers`/`facilities` call `renderPlaceholder(mainContent, 'Trucks')` etc.; `notfound`/default → `replaceNavigate('/fleet/home')`.
- Internal navigation: replace `navigate('view', {id})` calls inside legacy views with `navigate('/fleet/<path>/<id>')` (pushState). Replace `goBack()` with `history.back()`.

- [ ] **Step 4: Wire startRouter in boot (replace _renderView dispatch)**

In `boot()` (and the authenticated branch), after `showApp()`, call `startRouter(renderRoute)` instead of `_renderView(decodeViewHash(...))`. `startRouter` fires the initial route from `location.pathname`. Bare `/fleet` → `renderRoute` maps to home.

- [ ] **Step 5: Verify navigation end-to-end (Playwright)**

Use the `verify`/`run` skill: log in; click each sidebar link and confirm the URL changes to `/fleet/<name>` and the view renders; open a load/driver/trip detail and confirm `/fleet/loads/{id}` deep-links and the browser Back button returns to the list; reload a deep link (e.g. `/fleet/loads/{id}`) and confirm the SPA fallback serves it and the view renders. Confirm trucks/trailers/facilities show the placeholder.

Run: `npm test` — expect all green.

- [ ] **Step 6: Commit**

```bash
git add static/fleet/app.js static/fleet/index.html static/fleet/pages/placeholder.js
git commit -m "feat(fleet-ui): pushState routing + Equipment/Facilities nav (legacy views shimmed)"
```

---

## Task 4: Boot scope-store load + login gate + focus refresh

**Files:**
- Modify: `static/fleet/app.js` (boot, login gate, logout, sidebar gating hook)

- [ ] **Step 1: Load /me on boot and on tab focus**

In `app.js`, after a successful auth check in `boot()` (both the `isAuthenticated()` branch and the post-`tryRefresh` branch), `await loadMe();` BEFORE `startRouter(renderRoute)`, so scopes are known before the first render. Then add, once:

```js
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible' && isAuthenticated()) {
    loadMe().then(() => applyScopeGating());
  }
});
```

`applyScopeGating()` is a function that re-runs any scope-driven sidebar/control visibility (for now it can hide entity nav links the user lacks `:read` for — implement minimally: it may be a no-op placeholder that later phases extend; add a one-line comment saying entity pages call `hasScope` directly).

- [ ] **Step 2: Wire the 401 login gate**

Near the top of `boot()`, register the api module's unauthorized handler so a 401 from any `apiFetch` drops to login:

```js
setOnUnauthorized(() => {
  clearMe();
  showLogin();
});
```

(`showLogin` already exists in app.js; keep it.) Also: on a 403 from a write, the entity pages surface the message — no global handler needed here.

- [ ] **Step 3: Update logout to clear scope store**

In `initSidebar`'s logout handler, after `clearToken()`, add `clearMe();` so scopes don't leak across sessions. Then `showLogin()`.

- [ ] **Step 4: After successful login/refresh, load scopes before showing the app**

In the login form submit success path (`initLoginForm`) and setup success path, after saving the token and before showing the app/routing, `await loadMe();` so the first authenticated render has scopes.

- [ ] **Step 5: Verify (Playwright)**

Use `verify`/`run`: log in as the owner; confirm the app loads (scopes `*`); open devtools/network and confirm `GET /fleet/api/v1/me` fires on boot and again when you blur+refocus the tab; log out and confirm you return to the login pane and a protected fetch no longer succeeds.

Run: `npm test` — all green.

- [ ] **Step 6: Commit**

```bash
git add static/fleet/app.js
git commit -m "feat(fleet-ui): load /me scopes on boot+focus, wire 401 login gate, clear on logout"
```

---

## Task 5: Extract the read-only views to `pages/` modules

Move each read-only view fn out of app.js into its own module, importing what it needs. These are MOVES (not rewrites): copy the existing function body verbatim, add imports for the names it references, export it, delete it from app.js, and update `renderRoute` to call the imported version. Verify by running the app after each.

**Files:**
- Create: `static/fleet/pages/{home,events,documents,document-detail,account,login}.js`
- Modify: `static/fleet/app.js` (remove moved fns; import from pages/)

- [ ] **Step 1: Move the home view**

Move `renderHomeView` (currently `app.js` ~394-433) into `static/fleet/pages/home.js`:
- Export it: `export async function renderHomeView() { ... }` (body verbatim).
- Add imports it needs at the top of home.js: `import { apiFetch, API_BASE } from '../utils/api.js';` and whatever formatters/`navigate` it calls (inspect the body; likely `navigate` from `../router.js`, `escHtml`/`fmtUSD` from `../utils/format.js`). It also writes into `#main-content` — keep using `document.getElementById('main-content')` (or accept a `container`); match how the other moved views do it for consistency.
- In app.js: delete the local `renderHomeView`; `import { renderHomeView } from './pages/home.js';`; ensure `renderRoute` calls it for `name === 'home'`.

- [ ] **Step 2: Verify home renders (Playwright), then commit**

Run the app, load `/fleet/home`, confirm KPIs render. `npm test` green.
```bash
git add static/fleet/pages/home.js static/fleet/app.js
git commit -m "refactor(fleet-ui): extract home view to pages/home.js"
```

- [ ] **Step 3: Move the events view**

Same procedure for `renderEventsView` + its helpers (`fetchAndRenderEvents`, `clearEventsRefresh`, the events refresh timer state) → `static/fleet/pages/events.js`. The events module owns its own refresh-timer state (move `eventsRefreshTimer` and `clearEventsRefresh` with it; export `clearEventsRefresh` since boot/logout call it — import it back into app.js where referenced). Add needed imports (`apiFetch`, `humanizeEventType`, `escHtml`, `fmtDate`). Update app.js + `renderRoute`. Verify + commit:
```bash
git commit -am "refactor(fleet-ui): extract events view to pages/events.js"
```

- [ ] **Step 4: Move the documents list + detail**

Move `renderDocumentsView` → `pages/documents.js` and `renderDocumentDetailView` → `pages/document-detail.js`. These handle blob upload/download and the object-URL lifecycle (`activeObjectUrl`) — move that state with the detail module and export a `revokeActiveObjectUrl()` that `renderRoute` calls on navigation. Add imports (`apiFetch`, `API_BASE`, `escHtml`, `fmtBytes`, `fmtDate`, `navigate`). Verify (upload list renders; a document opens; navigating away revokes the URL) + commit.

- [ ] **Step 5: Move the account view**

Move `renderAccountView` (API-key create/delete) → `pages/account.js`. Needs `apiFetch`, `API_KEYS_BASE` (move this const or pass it), `escHtml`, `fmtDate`. Verify (keys list; create key shows the secret once; revoke works) + commit.

- [ ] **Step 6: Move login + setup forms**

Move `initLoginForm`, `initSetupForm`, `showLogin`, `showSetup`, `showLoginOrSetup`, `showApp` → `static/fleet/pages/login.js`. These call `loadMe` after success (Task 4) and `startRouter`/`renderRoute` to enter the app — pass the app's `enterApp` callback in, or import `loadMe`/`navigate`. Export `showLogin`/`showLoginOrSetup`/`showApp` (app.js + the 401 gate use them). Add imports (`saveToken`, `loadMe`, `API_BASE`, `AUTH_BASE`). Verify (first-run setup pane when no users; normal login; bad creds error) + commit.

- [ ] **Step 7: Final full run + suite**

Use `verify`/`run` for a full pass: setup/login, every sidebar route, a deep-linked detail + reload, back-button, logout. Then:
Run: `npm test` and `npm run test:driver` — all green.
```bash
git commit -am "refactor(fleet-ui): finish read-only view extraction"
```

---

## Done criteria

- [ ] `npm test` (incl. `router.test.js`) and `npm run test:driver` pass.
- [ ] The app loads as ES modules with real `/fleet/...` URLs; deep links + browser Back work; SPA fallback serves deep links.
- [ ] `/me` scopes load on boot + tab focus; 401 drops to login; logout clears the scope store.
- [ ] Read-only views (home, events, documents+detail, account, login) live in `pages/` modules; entity views (loads/trips/drivers/terminals) still render via legacy inline fns; trucks/trailers/facilities show a placeholder.
- [ ] `app.js` no longer duplicates the 0b-i helpers (imports them).
