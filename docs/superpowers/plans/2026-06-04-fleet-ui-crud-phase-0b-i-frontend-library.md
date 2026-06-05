# Fleet UI CRUD — Phase 0b-i (Frontend Toolchain + Module Library) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the fleet UI's JS test toolchain (Vitest + happy-dom + CI) and build the reusable `utils/` + `components/` ES module library — each unit-tested — WITHOUT touching the live `static/fleet/app.js`.

**Architecture:** New ES modules created alongside the untouched single-file `app.js`. The modules extract/standardize the logic the upcoming pushState rewrite (plan 0b-ii) will consume: formatting, auth/token, the `/me` scope store, the pure scope matcher, the declarative form (incl. the inherited-value submit rule), confirm, and table helpers. Pure logic is split from DOM rendering so the high-value pieces (payload building, scope matching, inherited-override rule) are tested as pure functions; DOM pieces are tested under happy-dom. The running app keeps using its in-file copies until 0b-ii swaps them out, so this phase carries zero risk to the live app.

**Tech Stack:** Vanilla ES modules (browser), Vitest 2.x test runner, happy-dom DOM environment, GitHub Actions CI. Backend `/me` endpoint (shipped in Phase 0a) is the scope source.

---

## Scope notes

- **This is plan 0b-i of a split Phase 0b.** The pushState **router + login gate + read-only view migration that rewrites `app.js`** is plan **0b-ii** (next). Here we ONLY add new files; `app.js` is not modified.
- The existing `tests/driver/*.test.js` use Node's built-in `node:test` runner. We are NOT migrating them; Vitest covers the new `tests/fleet/` tree, and CI runs both runners.
- `hasScope` (store-aware) lives in `utils/api.js` next to the scope store; the **pure** matcher `scopeGranted` lives in `components/scope-gate.js` (no imports, fully unit-testable). This is a minor, deliberate refinement of the spec, which named scope-gate as the home for `hasScope`.

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `package.json` | dev deps (vitest, happy-dom) + test scripts | Create |
| `vitest.config.js` | happy-dom env; only `tests/fleet/**` | Create |
| `.github/workflows/js-tests.yml` | CI: vitest (fleet) + `node --test` (driver) | Create |
| `static/fleet/utils/format.js` | pure formatters (escHtml, fmtUSD, …) | Create |
| `static/fleet/utils/auth.js` | token storage + JWT decode/expiry | Create |
| `static/fleet/utils/api.js` | `apiFetch`, `tryRefresh`, `/me` scope store, `hasScope` | Create |
| `static/fleet/components/scope-gate.js` | pure `scopeGranted` + DOM `gate` | Create |
| `static/fleet/components/form.js` | `buildPayload` (pure, incl. inherited rule) + `renderForm` (DOM) | Create |
| `static/fleet/components/confirm.js` | `confirmDelete` wrapper | Create |
| `static/fleet/components/table.js` | `renderTable` DOM helper | Create |
| `tests/fleet/*.test.js` | Vitest unit tests per module | Create |

All `static/fleet/` modules use `export function` (matching `static/driver/utils/*.js`).

---

## Task 1: JS toolchain (Vitest + happy-dom + CI)

**Files:**
- Create: `package.json`, `vitest.config.js`, `.github/workflows/js-tests.yml`, `tests/fleet/smoke.test.js`

- [ ] **Step 1: Create `package.json`**

```json
{
  "name": "ollie-fleet-ui",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "vitest run",
    "test:watch": "vitest",
    "test:driver": "node --test tests/driver/*.test.js"
  },
  "devDependencies": {
    "happy-dom": "^15.11.0",
    "vitest": "^2.1.8"
  }
}
```

- [ ] **Step 2: Create `vitest.config.js`**

happy-dom gives DOM globals (`document`, `localStorage`, `atob`, `fetch` is mockable). Restrict the include glob to `tests/fleet/` so the `node:test`-style `tests/driver/*.test.js` are NOT picked up by Vitest.

```js
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'happy-dom',
    include: ['tests/fleet/**/*.test.js'],
  },
});
```

- [ ] **Step 3: Add `node_modules/` to `.gitignore` (if not already present)**

Read `.gitignore`. If `node_modules` is not already ignored, append a line `node_modules/`. (Do not duplicate if present.)

- [ ] **Step 4: Write a smoke test**

Create `tests/fleet/smoke.test.js`:

```js
import { describe, it, expect } from 'vitest';

describe('toolchain smoke', () => {
  it('runs vitest', () => {
    expect(1 + 1).toBe(2);
  });

  it('has a happy-dom document', () => {
    const el = document.createElement('div');
    el.textContent = 'hi';
    expect(el.textContent).toBe('hi');
  });
});
```

- [ ] **Step 5: Install deps and run the smoke test**

Run: `npm install` (generates `package-lock.json` and `node_modules/`).
Then: `npm test`
Expected: PASS — 2 tests in `tests/fleet/smoke.test.js`. The `document` test confirms happy-dom is active.

- [ ] **Step 6: Create the CI workflow `.github/workflows/js-tests.yml`**

This repo pins GitHub Actions to commit SHAs. Use the SAME `actions/checkout`
pin every other workflow uses: `actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4`.
The `ubuntu-latest` runner ships Node 20+ preinstalled, so we do NOT add a
`setup-node` action (avoids introducing a new, separately-pinned action); we run
`npm` directly. Create `.github/workflows/js-tests.yml`:

```yaml
name: JS tests
on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

jobs:
  fleet-vitest:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4
      - run: node --version   # runner default Node (20+); fail loudly if ever downgraded
      - run: npm ci
      - run: npm test
      - run: npm run test:driver
```

- [ ] **Step 7: Commit**

```bash
git add package.json package-lock.json vitest.config.js .github/workflows/js-tests.yml tests/fleet/smoke.test.js .gitignore
git commit -m "build: add Vitest+happy-dom JS toolchain and CI job"
```

---

## Task 2: `utils/format.js` — pure formatters

Port the formatting helpers verbatim from `static/fleet/app.js` (lines ~286-377) into an ES module. Pure functions, no DOM.

**Files:**
- Create: `static/fleet/utils/format.js`
- Test: `tests/fleet/format.test.js`

- [ ] **Step 1: Write the failing test**

Create `tests/fleet/format.test.js`:

```js
import { describe, it, expect } from 'vitest';
import { escHtml, shortId, fmtUSD, fmtMiles, fmtBytes, badge, humanizeEventType } from '../../static/fleet/utils/format.js';

describe('escHtml', () => {
  it('escapes HTML metacharacters', () => {
    expect(escHtml('<a href="x">&')).toBe('&lt;a href=&quot;x&quot;&gt;&amp;');
  });
  it('returns empty string for falsy', () => {
    expect(escHtml('')).toBe('');
    expect(escHtml(null)).toBe('');
  });
});

describe('shortId', () => {
  it('takes first 8 chars', () => {
    expect(shortId('abcdef1234567890')).toBe('abcdef12');
  });
  it('em-dash for empty', () => {
    expect(shortId('')).toBe('—');
  });
});

describe('fmtUSD', () => {
  it('formats positive with 2 decimals', () => {
    expect(fmtUSD(1234.5)).toBe('$1,234.50');
  });
  it('formats negative with leading minus', () => {
    expect(fmtUSD(-5)).toBe('-$5.00');
  });
  it('em-dash for null/undefined', () => {
    expect(fmtUSD(null)).toBe('—');
    expect(fmtUSD(undefined)).toBe('—');
  });
  it('keeps zero as $0.00 (not em-dash)', () => {
    expect(fmtUSD(0)).toBe('$0.00');
  });
});

describe('fmtMiles', () => {
  it('one decimal + unit', () => {
    expect(fmtMiles(12)).toBe('12.0 mi');
  });
  it('em-dash for null', () => {
    expect(fmtMiles(null)).toBe('—');
  });
});

describe('fmtBytes', () => {
  it('B / KB / MB thresholds', () => {
    expect(fmtBytes(512)).toBe('512 B');
    expect(fmtBytes(2048)).toBe('2.0 KB');
    expect(fmtBytes(5 * 1024 * 1024)).toBe('5.0 MB');
  });
});

describe('badge', () => {
  it('slugifies status into a badge span', () => {
    expect(badge('In Transit')).toBe('<span class="badge badge--in_transit">In Transit</span>');
  });
  it('empty string for falsy', () => {
    expect(badge(null)).toBe('');
  });
});

describe('humanizeEventType', () => {
  it('maps known types', () => {
    expect(humanizeEventType('trip.assigned')).toBe('Trip Assigned');
  });
  it('title-cases unknown types', () => {
    expect(humanizeEventType('some_custom.event')).toBe('Some Custom Event');
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- format`
Expected: FAIL — cannot resolve `../../static/fleet/utils/format.js`.

- [ ] **Step 3: Create `static/fleet/utils/format.js`**

Port verbatim from `app.js` (keep behavior identical), adding `export`:

```js
export function escHtml(s) {
  if (!s) return '';
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

export function badge(status) {
  if (!status) return '';
  const slug = status.toLowerCase().replace(/[^a-z0-9_]/g, '_');
  return `<span class="badge badge--${slug}">${escHtml(status)}</span>`;
}

export function shortId(id) {
  if (!id) return '—';
  return id.slice(0, 8);
}

export function fmtDate(isoStr) {
  if (!isoStr) return '—';
  try {
    return new Date(isoStr).toLocaleString();
  } catch {
    return isoStr;
  }
}

export function fmtArrivalWindow(start, end) {
  if (!start) return '—';
  if (!end) {
    try { return new Date(start).toLocaleString(); } catch { return start; }
  }
  try {
    const s = new Date(start);
    const e = new Date(end);
    const sameDay = s.toDateString() === e.toDateString();
    if (sameDay) {
      const sStr = s.toLocaleString();
      const eStr = e.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      return `${sStr}–${eStr}`;
    }
    return `${s.toLocaleString()} – ${e.toLocaleString()}`;
  } catch {
    return start;
  }
}

export function fmtBytes(n) {
  if (!n) return '—';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function fmtUSD(n) {
  if (n === null || n === undefined) return '—';
  const sign = n < 0 ? '-' : '';
  const abs = Math.abs(n);
  return `${sign}$${abs.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

export function fmtMiles(n) {
  if (n === null || n === undefined) return '—';
  return `${n.toFixed(1)} mi`;
}

export function humanizeEventType(type) {
  const map = {
    'trip.assigned':     'Trip Assigned',
    'trip.unassigned':   'Trip Unassigned',
    'trip.dispatched':   'Trip Dispatched',
    'trip.undispatched': 'Trip Undispatched',
    'trip.in_transit':   'Trip In Transit',
    'trip.delivered':    'Trip Delivered',
    'trip_completed':    'Trip Completed',
    'trip.cancelled':    'Trip Cancelled',
    'stop.arrived':      'Stop Arrived',
    'stop.departed':     'Stop Departed',
    'stop.late':         'Stop Late',
    'check_call':        'Check Call',
    'driver_available':  'Driver Available',
    'truck_available':   'Truck Available',
    'trailer_available': 'Trailer Available',
  };
  return map[type] || String(type).replace(/[_.]/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- format`
Expected: PASS (all format tests).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/utils/format.js tests/fleet/format.test.js
git commit -m "feat(fleet-ui): add utils/format.js with tests"
```

---

## Task 3: `utils/auth.js` — token storage + JWT decode/expiry

Port the token + JWT helpers from `app.js` (lines 20-63). Pure-ish (uses `localStorage` + `atob`, both provided by happy-dom).

**Files:**
- Create: `static/fleet/utils/auth.js`
- Test: `tests/fleet/auth.test.js`

- [ ] **Step 1: Write the failing test**

happy-dom provides `localStorage`, `atob`, `btoa`. Build a fake JWT (`header.payload.signature`) with a base64url payload so `decodeJwtPayload` / `isTokenExpired` can be exercised deterministically.

Create `tests/fleet/auth.test.js`:

```js
import { describe, it, expect, beforeEach } from 'vitest';
import {
  getToken, saveToken, clearToken,
  decodeJwtPayload, isTokenExpired, isAuthenticated, TOKEN_KEY,
} from '../../static/fleet/utils/auth.js';

// base64url-encode a JS object as a fake JWT payload
function makeJwt(payloadObj) {
  const b64 = btoa(JSON.stringify(payloadObj)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  return `header.${b64}.sig`;
}

beforeEach(() => {
  localStorage.clear();
});

describe('token storage', () => {
  it('save/get/clear round-trip', () => {
    saveToken('abc');
    expect(getToken()).toBe('abc');
    clearToken();
    expect(getToken()).toBe(null);
  });
});

describe('decodeJwtPayload', () => {
  it('decodes the payload segment', () => {
    const tok = makeJwt({ sub: 'u1', exp: 9999999999 });
    expect(decodeJwtPayload(tok)).toMatchObject({ sub: 'u1' });
  });
  it('returns null for malformed token', () => {
    expect(decodeJwtPayload('not-a-jwt')).toBe(null);
  });
});

describe('isTokenExpired', () => {
  it('false for a future exp', () => {
    expect(isTokenExpired(makeJwt({ exp: Math.floor(Date.now() / 1000) + 3600 }))).toBe(false);
  });
  it('true for a past exp', () => {
    expect(isTokenExpired(makeJwt({ exp: Math.floor(Date.now() / 1000) - 10 }))).toBe(true);
  });
  it('true when exp missing', () => {
    expect(isTokenExpired(makeJwt({ sub: 'u1' }))).toBe(true);
  });
});

describe('isAuthenticated', () => {
  it('false with no token', () => {
    expect(isAuthenticated()).toBe(false);
  });
  it('true with an unexpired token', () => {
    saveToken(makeJwt({ exp: Math.floor(Date.now() / 1000) + 3600 }));
    expect(isAuthenticated()).toBe(true);
  });
  it('clears and returns false for an expired token', () => {
    saveToken(makeJwt({ exp: Math.floor(Date.now() / 1000) - 10 }));
    expect(isAuthenticated()).toBe(false);
    expect(getToken()).toBe(null); // expired token is cleared
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- auth`
Expected: FAIL — module not found.

- [ ] **Step 3: Create `static/fleet/utils/auth.js`**

```js
export const TOKEN_KEY = 'dispatch_token';

export function getToken() {
  return localStorage.getItem(TOKEN_KEY);
}

export function saveToken(token) {
  localStorage.setItem(TOKEN_KEY, token);
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY);
}

/** Decode a JWT payload (base64url → JSON) WITHOUT verifying the signature.
 *  Used only for reading `exp` for UX. */
export function decodeJwtPayload(token) {
  try {
    const parts = token.split('.');
    if (parts.length !== 3) return null;
    const payload = parts[1].replace(/-/g, '+').replace(/_/g, '/');
    return JSON.parse(atob(payload));
  } catch {
    return null;
  }
}

export function isTokenExpired(token) {
  const payload = decodeJwtPayload(token);
  if (!payload || !payload.exp) return true;
  return payload.exp * 1000 < Date.now();
}

export function isAuthenticated() {
  const token = getToken();
  if (!token) return false;
  if (isTokenExpired(token)) {
    clearToken();
    return false;
  }
  return true;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- auth`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add static/fleet/utils/auth.js tests/fleet/auth.test.js
git commit -m "feat(fleet-ui): add utils/auth.js with tests"
```

---

## Task 4: `components/scope-gate.js` — pure scope matcher + DOM gate

Replicates the backend `scope_granted` semantics (`src/models/permission.rs:140`): a required scope `r:a` is granted by the global `*`, an exact match, or the per-resource wildcard `r:*`.

**Files:**
- Create: `static/fleet/components/scope-gate.js`
- Test: `tests/fleet/scope-gate.test.js`

- [ ] **Step 1: Write the failing test**

```js
import { describe, it, expect } from 'vitest';
import { scopeGranted, gate } from '../../static/fleet/components/scope-gate.js';

describe('scopeGranted', () => {
  it('grants on exact match', () => {
    expect(scopeGranted(['loads:write'], 'loads:write')).toBe(true);
  });
  it('grants on per-resource wildcard', () => {
    expect(scopeGranted(['loads:*'], 'loads:write')).toBe(true);
  });
  it('grants on global superuser', () => {
    expect(scopeGranted(['*'], 'anything:delete')).toBe(true);
  });
  it('denies when absent', () => {
    expect(scopeGranted(['loads:read'], 'loads:write')).toBe(false);
  });
  it('denies cross-resource wildcard', () => {
    expect(scopeGranted(['trucks:*'], 'loads:write')).toBe(false);
  });
  it('denies on empty/missing scopes', () => {
    expect(scopeGranted([], 'loads:write')).toBe(false);
    expect(scopeGranted(null, 'loads:write')).toBe(false);
  });
});

describe('gate', () => {
  it('hides the element when not granted', () => {
    const el = document.createElement('button');
    gate(el, false);
    expect(el.hidden).toBe(true);
  });
  it('shows the element when granted', () => {
    const el = document.createElement('button');
    el.hidden = true;
    gate(el, true);
    expect(el.hidden).toBe(false);
  });
  it('no-ops on null element', () => {
    expect(() => gate(null, true)).not.toThrow();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- scope-gate`
Expected: FAIL — module not found.

- [ ] **Step 3: Create `static/fleet/components/scope-gate.js`**

```js
/**
 * Pure scope matcher mirroring the backend `scope_granted`:
 * a required scope `r:a` is satisfied by the global `*`, an exact match,
 * or the per-resource wildcard `r:*`.
 */
export function scopeGranted(scopes, required) {
  if (!Array.isArray(scopes) || scopes.length === 0) return false;
  const colon = required.indexOf(':');
  const resourceWildcard = colon === -1 ? null : `${required.slice(0, colon)}:*`;
  return scopes.some(s => s === '*' || s === required || s === resourceWildcard);
}

/** Show/hide a control by grant. Fail-safe: a null element is a no-op. */
export function gate(el, granted) {
  if (!el) return;
  el.hidden = !granted;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- scope-gate`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add static/fleet/components/scope-gate.js tests/fleet/scope-gate.test.js
git commit -m "feat(fleet-ui): add components/scope-gate.js with tests"
```

---

## Task 5: `utils/api.js` — apiFetch, refresh, `/me` scope store, `hasScope`

Ports `apiFetch`/`tryRefresh` from `app.js` (lines 67-115) and adds the `/me` scope store + the store-aware `hasScope`. The 401 path is decoupled from the app shell: instead of calling `showLogin()` directly (which only exists in the monolith), `apiFetch` invokes an injectable `onUnauthorized` callback (0b-ii wires it to the router's login gate). Default is a no-op.

**Files:**
- Create: `static/fleet/utils/api.js`
- Test: `tests/fleet/api.test.js`

- [ ] **Step 1: Write the failing test**

We test the high-value, deterministic parts: the scope store (`loadMe` → `getScopes`/`hasScope`/`getIdentity`, and the failure fallback) and `apiFetch` attaching the bearer header. `fetch` is stubbed via `vi.fn`.

```js
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { saveToken, clearToken } from '../../static/fleet/utils/auth.js';
import {
  apiFetch, loadMe, getScopes, getIdentity, hasScope, clearMe, API_BASE,
} from '../../static/fleet/utils/api.js';

function jsonResponse(body, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  };
}

beforeEach(() => {
  localStorage.clear();
  clearMe();
  vi.restoreAllMocks();
});

describe('apiFetch', () => {
  it('attaches the bearer token and JSON content-type', async () => {
    saveToken('tok123');
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ ok: true }));
    vi.stubGlobal('fetch', fetchMock);

    await apiFetch(`${API_BASE}/loads`);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [, opts] = fetchMock.mock.calls[0];
    expect(opts.headers.Authorization).toBe('Bearer tok123');
    expect(opts.headers['Content-Type']).toBe('application/json');
  });

  it('does not set JSON content-type for FormData bodies', async () => {
    saveToken('tok123');
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);

    await apiFetch(`${API_BASE}/blobs`, { method: 'POST', body: new FormData() });

    const [, opts] = fetchMock.mock.calls[0];
    expect(opts.headers['Content-Type']).toBeUndefined();
  });
});

describe('scope store', () => {
  it('loadMe populates scopes + identity', async () => {
    saveToken('tok');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({
      fleet_user_id: 'u1', name: 'Jane', email: 'j@x.com', role: 'owner',
      effective_scopes: ['*'],
    })));

    const me = await loadMe();
    expect(me.email).toBe('j@x.com');
    expect(getScopes()).toEqual(['*']);
    expect(getIdentity().name).toBe('Jane');
    expect(hasScope('loads:delete')).toBe(true); // '*' grants everything
  });

  it('hasScope is false before loadMe (fail-safe)', () => {
    expect(getScopes()).toEqual([]);
    expect(hasScope('loads:write')).toBe(false);
  });

  it('loadMe failure yields empty scopes (controls stay hidden)', async () => {
    saveToken('tok');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({}, 500)));
    const me = await loadMe();
    expect(me).toBe(null);
    expect(getScopes()).toEqual([]);
    expect(hasScope('loads:write')).toBe(false);
  });

  it('clearMe resets the store', async () => {
    saveToken('tok');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({
      role: 'dispatcher', effective_scopes: ['loads:read'],
    })));
    await loadMe();
    expect(getScopes()).toEqual(['loads:read']);
    clearMe();
    expect(getScopes()).toEqual([]);
    expect(getIdentity()).toBe(null);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- api`
Expected: FAIL — module not found.

- [ ] **Step 3: Create `static/fleet/utils/api.js`**

```js
import { getToken, saveToken, clearToken } from './auth.js';
import { scopeGranted } from '../components/scope-gate.js';

export const API_BASE = '/fleet/api/v1';
export const AUTH_BASE = '/fleet/auth';

// ─── 401 handler injection ───────────────────────────────────
// 0b-ii wires this to the router's login gate. Default: no-op.
let _onUnauthorized = () => {};
export function setOnUnauthorized(fn) { _onUnauthorized = fn || (() => {}); }

// ─── Token refresh ───────────────────────────────────────────
export async function tryRefresh() {
  try {
    const res = await fetch(`${AUTH_BASE}/refresh`, { method: 'POST', credentials: 'same-origin' });
    if (!res.ok) return false;
    const data = await res.json();
    const token = data.token || data.access_token;
    if (!token) return false;
    saveToken(token);
    return true;
  } catch {
    return false;
  }
}

// ─── API fetch wrapper ───────────────────────────────────────
export async function apiFetch(path, options = {}) {
  const token = getToken();
  const isFormData = options.body instanceof FormData;
  const headers = {
    ...(isFormData ? {} : { 'Content-Type': 'application/json' }),
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
    ...(options.headers || {}),
  };

  const res = await fetch(path, { ...options, headers });

  if (res.status === 401) {
    const refreshed = await tryRefresh();
    if (refreshed) {
      const newToken = getToken();
      const retryHeaders = {
        ...(isFormData ? {} : { 'Content-Type': 'application/json' }),
        ...(newToken ? { Authorization: `Bearer ${newToken}` } : {}),
        ...(options.headers || {}),
      };
      const retry = await fetch(path, { ...options, headers: retryHeaders });
      if (retry.status !== 401) return retry;
    }
    clearToken();
    clearMe();
    _onUnauthorized();
    throw new Error('Unauthorized — please sign in again.');
  }

  return res;
}

// ─── /me scope store ─────────────────────────────────────────
let _scopes = null;
let _identity = null;

/** Fetch /me and cache identity + effective scopes. Returns the body, or
 *  null on failure (scopes reset to empty so controls stay hidden). */
export async function loadMe() {
  try {
    const res = await apiFetch(`${API_BASE}/me`);
    if (!res.ok) { _scopes = []; _identity = null; return null; }
    const me = await res.json();
    _scopes = Array.isArray(me.effective_scopes) ? me.effective_scopes : [];
    _identity = me;
    return me;
  } catch {
    _scopes = []; _identity = null;
    return null;
  }
}

export function getScopes() { return _scopes || []; }
export function getIdentity() { return _identity; }
export function clearMe() { _scopes = null; _identity = null; }

/** Store-aware authority check used by pages to gate controls. */
export function hasScope(required) {
  return scopeGranted(getScopes(), required);
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- api`
Expected: PASS (all api tests).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/utils/api.js tests/fleet/api.test.js
git commit -m "feat(fleet-ui): add utils/api.js with /me scope store + tests"
```

---

## Task 6: `components/form.js` — payload builder (incl. inherited rule) + renderForm

The load-bearing component. Split into a **pure** `buildPayload(fields, raw, reverted)` (fully unit-tested, including the inherited-value rule) and a DOM `renderForm(container, opts)` (tested under happy-dom for structure + submit wiring).

**Field shapes** (declarative):
- `{ key, label, type: 'text'|'number'|'int'|'checkbox'|'select'|'inheritable', required?, options? }`
- `inheritable` fields also carry `inheritedValue` + `inheritedFrom` for display.

**`buildPayload(fields, raw, reverted)`** — `raw` is a map of `key → input string/boolean`; `reverted` is a `Set` of inheritable keys the user clicked "revert to inherited" on. Returns `{ payload, errors }`.

Coercion + inclusion rules:
- `text`/`select`: blank → omit; else string.
- `number`: blank → omit; else `parseFloat`.
- `int`: blank → omit; else `parseInt(…, 10)`.
- `checkbox`: always include boolean.
- `required` text/select with blank value → an error entry; field omitted.
- `inheritable` (the override-safety rule):
  - key in `reverted` → `payload[key] = null` (explicit clear).
  - else blank → omit (inherited stays; never bakes in the inherited number).
  - else → coerce as a number and include (intentional override).

**Files:**
- Create: `static/fleet/components/form.js`
- Test: `tests/fleet/form.test.js`

- [ ] **Step 1: Write the failing test**

```js
import { describe, it, expect, vi } from 'vitest';
import { buildPayload, renderForm } from '../../static/fleet/components/form.js';

const FIELDS = [
  { key: 'name', label: 'Name', type: 'text', required: true },
  { key: 'year', label: 'Year', type: 'int' },
  { key: 'rate', label: 'Rate', type: 'number' },
  { key: 'is_default', label: 'Default', type: 'checkbox' },
  { key: 'status', label: 'Status', type: 'select', options: ['active', 'inactive'] },
  { key: 'loaded_rate_per_mile', label: 'Loaded $/mi', type: 'inheritable', inheritedValue: 0.55, inheritedFrom: 'Terminal: Dallas' },
];

describe('buildPayload coercion + omission', () => {
  it('coerces by type and omits blanks', () => {
    const { payload, errors } = buildPayload(FIELDS, {
      name: 'Unit 1', year: '2022', rate: '', is_default: true, status: 'active',
      loaded_rate_per_mile: '',
    }, new Set());
    expect(errors).toEqual([]);
    expect(payload).toEqual({ name: 'Unit 1', year: 2022, is_default: true, status: 'active' });
    // rate blank → omitted; inheritable blank → omitted (NOT persisted as override)
    expect('rate' in payload).toBe(false);
    expect('loaded_rate_per_mile' in payload).toBe(false);
  });

  it('flags a required blank field', () => {
    const { payload, errors } = buildPayload(FIELDS, { name: '' }, new Set());
    expect(errors).toContain('Name is required.');
    expect('name' in payload).toBe(false);
  });
});

describe('buildPayload inherited-value rule', () => {
  it('inherited + typed value → sent as override', () => {
    const { payload } = buildPayload(FIELDS, { name: 'x', loaded_rate_per_mile: '0.80' }, new Set());
    expect(payload.loaded_rate_per_mile).toBe(0.80);
  });
  it('inherited + blank → omitted (never bakes in inherited number)', () => {
    const { payload } = buildPayload(FIELDS, { name: 'x', loaded_rate_per_mile: '' }, new Set());
    expect('loaded_rate_per_mile' in payload).toBe(false);
  });
  it('revert clicked → explicit null', () => {
    const { payload } = buildPayload(FIELDS, { name: 'x', loaded_rate_per_mile: '0.80' }, new Set(['loaded_rate_per_mile']));
    expect(payload.loaded_rate_per_mile).toBe(null);
  });
});

describe('renderForm', () => {
  it('renders inputs and submits the built payload', async () => {
    const container = document.createElement('div');
    const onSubmit = vi.fn().mockResolvedValue({ ok: true });
    renderForm(container, {
      title: 'Edit',
      fields: [{ key: 'name', label: 'Name', type: 'text', required: true }],
      values: { name: 'Start' },
      submitLabel: 'Save',
      onSubmit,
    });
    // input pre-filled from values
    const input = container.querySelector('[data-field="name"]');
    expect(input).not.toBe(null);
    expect(input.value).toBe('Start');
    input.value = 'Changed';
    // submit
    container.querySelector('[data-form-submit]').click();
    await Promise.resolve(); await Promise.resolve();
    expect(onSubmit).toHaveBeenCalledWith({ name: 'Changed' });
  });

  it('blocks submit and shows an error when a required field is blank', async () => {
    const container = document.createElement('div');
    const onSubmit = vi.fn();
    renderForm(container, {
      title: 'New',
      fields: [{ key: 'name', label: 'Name', type: 'text', required: true }],
      values: {},
      submitLabel: 'Save',
      onSubmit,
    });
    container.querySelector('[data-form-submit]').click();
    await Promise.resolve();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(container.querySelector('[data-form-error]').hidden).toBe(false);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm test -- form`
Expected: FAIL — module not found.

- [ ] **Step 3: Create `static/fleet/components/form.js`**

```js
import { escHtml } from '../utils/format.js';

/**
 * Pure payload builder. `raw` maps field key → input value (string or bool);
 * `reverted` is a Set of inheritable keys the user chose to revert to inherited.
 * Returns { payload, errors }.
 */
export function buildPayload(fields, raw, reverted = new Set()) {
  const payload = {};
  const errors = [];
  for (const f of fields) {
    const v = raw[f.key];
    if (f.type === 'checkbox') {
      payload[f.key] = !!v;
      continue;
    }
    if (f.type === 'inheritable') {
      if (reverted.has(f.key)) { payload[f.key] = null; continue; }   // explicit clear
      if (v === '' || v === undefined || v === null) continue;        // inherited stays
      payload[f.key] = parseFloat(v);                                 // intentional override
      continue;
    }
    const blank = v === '' || v === undefined || v === null;
    if (f.required && blank) { errors.push(`${f.label} is required.`); continue; }
    if (blank) continue;                                              // omit → "leave unchanged"
    if (f.type === 'number') payload[f.key] = parseFloat(v);
    else if (f.type === 'int') payload[f.key] = parseInt(v, 10);
    else payload[f.key] = v;                                          // text / select
  }
  return { payload, errors };
}

function fieldControl(f, value) {
  const val = value === undefined || value === null ? '' : value;
  if (f.type === 'checkbox') {
    return `<input class="form-checkbox" type="checkbox" data-field="${f.key}" ${value ? 'checked' : ''}>`;
  }
  if (f.type === 'select') {
    const opts = (f.options || []).map(o =>
      `<option value="${escHtml(o)}" ${o === value ? 'selected' : ''}>${escHtml(o)}</option>`).join('');
    return `<select class="form-input" data-field="${f.key}"><option value=""></option>${opts}</select>`;
  }
  if (f.type === 'inheritable') {
    const ph = f.inheritedValue != null ? `Inherited: ${f.inheritedValue} (${escHtml(f.inheritedFrom || '')})` : '';
    return `<input class="form-input" type="number" step="any" data-field="${f.key}"
      value="${value != null ? value : ''}" placeholder="${escHtml(ph)}">`;
  }
  const inputType = (f.type === 'number' || f.type === 'int') ? 'number' : 'text';
  const step = f.type === 'number' ? ' step="any"' : '';
  return `<input class="form-input" type="${inputType}"${step} data-field="${f.key}" value="${escHtml(String(val))}">`;
}

/**
 * Render an inline form panel into `container`.
 * opts: { title, fields, values, submitLabel, onSubmit(payload) -> Promise }
 */
export function renderForm(container, { title, fields, values = {}, submitLabel = 'Save', onSubmit }) {
  const rows = fields.map(f => `
    <div class="form-group">
      <label class="form-label">${escHtml(f.label)}</label>
      ${fieldControl(f, values[f.key])}
    </div>`).join('');

  container.innerHTML = `
    <div class="form-panel">
      <h2 class="form-panel__title">${escHtml(title || '')}</h2>
      <div class="alert alert--error" data-form-error hidden></div>
      ${rows}
      <div class="form-panel__actions">
        <button class="btn btn--primary" data-form-submit>${escHtml(submitLabel)}</button>
      </div>
    </div>`;

  const errEl = container.querySelector('[data-form-error]');
  const submitBtn = container.querySelector('[data-form-submit]');

  function readRaw() {
    const raw = {};
    for (const f of fields) {
      const el = container.querySelector(`[data-field="${f.key}"]`);
      if (!el) continue;
      raw[f.key] = f.type === 'checkbox' ? el.checked : el.value;
    }
    return raw;
  }

  submitBtn.addEventListener('click', async () => {
    const { payload, errors } = buildPayload(fields, readRaw());
    if (errors.length) {
      errEl.textContent = errors.join(' ');
      errEl.hidden = false;
      return;
    }
    errEl.hidden = true;
    submitBtn.disabled = true;
    try {
      const res = await onSubmit(payload);
      if (res && res.ok === false) {
        const data = await res.json().catch(() => ({}));
        errEl.textContent = data.error || `HTTP ${res.status}`;
        errEl.hidden = false;
      }
    } catch (err) {
      if (err && err.message !== 'Unauthorized — please sign in again.') {
        errEl.textContent = `Save failed: ${err.message}`;
        errEl.hidden = false;
      }
    } finally {
      submitBtn.disabled = false;
    }
  });
}
```

Note: `renderForm`'s submit passes only `payload` to `onSubmit` (the test asserts `toHaveBeenCalledWith({ name: 'Changed' })`). The revert-to-inherited UI control is added in a later phase when inheritable fields first appear in a real form (Drivers, Phase 2); `buildPayload` already supports it via the `reverted` set.

- [ ] **Step 4: Run to verify it passes**

Run: `npm test -- form`
Expected: PASS (all form tests, pure + DOM).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/components/form.js tests/fleet/form.test.js
git commit -m "feat(fleet-ui): add components/form.js (payload builder + renderForm) with tests"
```

---

## Task 7: `components/confirm.js` + `components/table.js`

Two small helpers. `confirmDelete` wraps native `confirm()` with a standard message; `renderTable` builds a clickable list table.

**Files:**
- Create: `static/fleet/components/confirm.js`, `static/fleet/components/table.js`
- Test: `tests/fleet/confirm.test.js`, `tests/fleet/table.test.js`

- [ ] **Step 1: Write the failing tests**

`tests/fleet/confirm.test.js`:

```js
import { describe, it, expect, vi, afterEach } from 'vitest';
import { confirmDelete } from '../../static/fleet/components/confirm.js';

afterEach(() => vi.restoreAllMocks());

describe('confirmDelete', () => {
  it('returns true when the user confirms', () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    expect(confirmDelete('a driver')).toBe(true);
    expect(globalThis.confirm).toHaveBeenCalledWith('Delete a driver? This can be undone by reactivating.');
  });
  it('returns false when the user cancels', () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(false));
    expect(confirmDelete('a driver')).toBe(false);
  });
});
```

`tests/fleet/table.test.js`:

```js
import { describe, it, expect, vi } from 'vitest';
import { renderTable } from '../../static/fleet/components/table.js';

describe('renderTable', () => {
  it('renders headers and a row per item, escaping cell content', () => {
    const container = document.createElement('div');
    renderTable(container, {
      columns: [{ header: 'Name', cell: r => r.name }, { header: 'Status', cell: r => r.status }],
      rows: [{ id: '1', name: '<b>A</b>', status: 'active' }, { id: '2', name: 'B', status: 'inactive' }],
      onRowClick: () => {},
    });
    expect(container.querySelectorAll('thead th').length).toBe(2);
    expect(container.querySelectorAll('tbody tr').length).toBe(2);
    // cell content is escaped
    expect(container.querySelector('tbody tr td').innerHTML).toBe('&lt;b&gt;A&lt;/b&gt;');
  });

  it('invokes onRowClick with the row id', () => {
    const container = document.createElement('div');
    const onRowClick = vi.fn();
    renderTable(container, {
      columns: [{ header: 'Name', cell: r => r.name }],
      rows: [{ id: 'abc', name: 'A' }],
      onRowClick,
    });
    container.querySelector('tbody tr').click();
    expect(onRowClick).toHaveBeenCalledWith('abc');
  });
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `npm test -- confirm table`
Expected: FAIL — modules not found.

- [ ] **Step 3: Create the modules**

`static/fleet/components/confirm.js`:

```js
/** Standard destructive-action confirm. Soft delete is reversible, so the
 *  copy says so. Returns the user's choice as a boolean. */
export function confirmDelete(what) {
  return confirm(`Delete ${what}? This can be undone by reactivating.`);
}
```

`static/fleet/components/table.js`:

```js
import { escHtml } from '../utils/format.js';

/**
 * Render a clickable list table into `container`.
 * opts: { columns: [{header, cell(row)->string}], rows: [{id, ...}], onRowClick(id) }
 */
export function renderTable(container, { columns, rows, onRowClick }) {
  const head = columns.map(c => `<th>${escHtml(c.header)}</th>`).join('');
  const body = rows.map(r => {
    const cells = columns.map(c => `<td>${escHtml(String(c.cell(r) ?? ''))}</td>`).join('');
    return `<tr data-row-id="${escHtml(String(r.id))}">${cells}</tr>`;
  }).join('');

  container.innerHTML = `<table class="table"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;

  if (onRowClick) {
    container.querySelectorAll('tbody tr').forEach(tr => {
      tr.addEventListener('click', () => onRowClick(tr.dataset.rowId));
    });
  }
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `npm test -- confirm table`
Expected: PASS.

- [ ] **Step 5: Run the FULL fleet suite**

Run: `npm test`
Expected: PASS — all `tests/fleet/**` suites green (smoke, format, auth, scope-gate, api, form, confirm, table).

- [ ] **Step 6: Commit**

```bash
git add static/fleet/components/confirm.js static/fleet/components/table.js tests/fleet/confirm.test.js tests/fleet/table.test.js
git commit -m "feat(fleet-ui): add components/confirm.js + table.js with tests"
```

---

## Done criteria

- [ ] `npm test` runs Vitest under happy-dom over `tests/fleet/**` and passes.
- [ ] `npm run test:driver` still runs the existing `node:test` driver tests.
- [ ] CI workflow `.github/workflows/js-tests.yml` runs both on push/PR.
- [ ] New modules exist and are tested: `utils/{format,auth,api}.js`, `components/{scope-gate,form,confirm,table}.js`.
- [ ] `buildPayload`'s inherited-value rule is proven: inherited+blank omitted, inherited+typed sent, revert→null.
- [ ] `static/fleet/app.js` is UNCHANGED (the rewrite is plan 0b-ii).
