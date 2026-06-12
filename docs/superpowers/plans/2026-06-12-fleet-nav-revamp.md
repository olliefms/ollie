# Fleet Sidebar Nav Revamp Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the fleet SPA's flat, mis-grouped sidebar with a scope-gated, icon-led, JS-rendered nav (Operations/Fleet/Network/Admin) plus a Claude-Desktop-style account footer with a theme switcher.

**Architecture:** A single `NAV_GROUPS` config drives a `renderSidebar(container, {scopes, pathname})` function that filters items by the scopes already returned from `/me` and drops empty groups. The Account link and Sign-out button collapse into `renderAccountFooter(container, {identity, scopes, onSignOut})` with a pull-up menu (Account / Theme / Sign out). A small `theme.js` persists a light/dark/system choice and sets `[data-theme]`; the dark palette itself is deferred (stub token block). `app.js` renders both after every `loadMe()`.

**Tech Stack:** Vanilla ES modules (no bundler — native relative imports), Vitest + happy-dom for unit tests, inline-SVG icons via `DOMParser` (ported from the driver surface). Static assets only — no Rust rebuild.

---

## Design reference

Spec: `docs/superpowers/specs/2026-06-12-fleet-nav-revamp-design.md`

Scope table (verified against `src/api/fleet_portal/data.rs` & siblings):

| Item | Path | Gating scope |
|---|---|---|
| Home | `/fleet/home` | *(none)* |
| Loads | `/fleet/loads` | `loads:read` |
| Trips | `/fleet/trips` | `trips:read` |
| Events | `/fleet/events` | `events:read` |
| Drivers | `/fleet/drivers` | `drivers:read` |
| Trucks | `/fleet/trucks` | `trucks:read` |
| Trailers | `/fleet/trailers` | `trailers:read` |
| Facilities | `/fleet/facilities` | `facilities:read` |
| Terminals | `/fleet/terminals` | `terminals:read` |
| Documents | `/fleet/documents` | `blobs:read` |
| Account (footer menu) | `/fleet/account` | `api_keys:read` |

## File structure

**New**
- `static/fleet/components/icons.js` — inline-SVG icon factories (one per nav item + chevron/theme/key/logout). Each exported fn returns a fresh cloned `<svg>`.
- `static/fleet/utils/theme.js` — `getTheme`/`setTheme`/`applyTheme`/`resolveTheme`/`initTheme`.
- `static/fleet/components/nav.js` — `NAV_GROUPS`, `visibleGroups(scopes)`, `renderSidebar(container, opts)`.
- `static/fleet/components/account-footer.js` — `initials`, `roleLabel`, `renderAccountFooter(container, opts)`.
- `tests/fleet/icons.test.js`, `tests/fleet/theme.test.js`, `tests/fleet/nav.test.js`, `tests/fleet/account-footer.test.js`.

**Modified**
- `static/fleet/index.html` — replace static nav links + footer button with mount points `#sidebar-nav` / `#sidebar-footer`.
- `static/fleet/app.js` — render chrome after `loadMe()`; drop the old `initSidebar` logout wiring (moves into the footer's `onSignOut`).
- `static/fleet/css/base.css` — `[data-theme="dark"]` stub token block.
- `static/fleet/css/components.css` — `.sidebar__icon`, account footer, and pull-up menu rules.

**Unchanged:** all backend (`src/**`), `router.js`, `components/scope-gate.js`, the `/fleet/account` page module and route, and all `?v=` asset stamps (only `cut-release` bumps those — do **not** touch them here).

## Conventions for every commit

- Sign off: `git commit -s` (repo enforces DCO).
- Co-author trailer: `Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>`.
- Run `npm test` (Vitest) before each commit that touches JS. There is no JS lint script and no Rust change, so no clippy/build is required.

---

## Task 1: Fleet icon set

**Files:**
- Create: `static/fleet/components/icons.js`
- Test: `tests/fleet/icons.test.js`

- [ ] **Step 1: Write the failing test**

```js
// tests/fleet/icons.test.js
import { describe, it, expect } from 'vitest';
import * as icons from '../../static/fleet/components/icons.js';

const NAMES = [
  'homeIcon', 'loadsIcon', 'tripsIcon', 'eventsIcon', 'driversIcon',
  'trucksIcon', 'trailersIcon', 'facilitiesIcon', 'terminalsIcon',
  'documentsIcon', 'keyIcon', 'chevronUpIcon', 'themeIcon', 'logoutIcon',
];

describe('fleet icons', () => {
  it('exports a factory for every nav + footer icon', () => {
    for (const name of NAMES) {
      expect(typeof icons[name]).toBe('function');
    }
  });

  it('each factory returns a fresh <svg> element', () => {
    for (const name of NAMES) {
      const a = icons[name]();
      const b = icons[name]();
      expect(a.tagName.toLowerCase()).toBe('svg');
      expect(a).not.toBe(b); // distinct clones, not a shared node
    }
  });

  it('icons use currentColor so they inherit link color', () => {
    expect(icons.homeIcon().getAttribute('stroke')).toBe('currentColor');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- icons`
Expected: FAIL — cannot resolve `static/fleet/components/icons.js`.

- [ ] **Step 3: Write the implementation**

```js
// static/fleet/components/icons.js
function parse(source) {
  const doc = new DOMParser().parseFromString(source, 'image/svg+xml');
  const root = doc.documentElement;
  if (root.tagName.toLowerCase() === 'parsererror') {
    throw new Error('SVG parse failed');
  }
  return root;
}

const OPEN = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" '
  + 'fill="none" stroke="currentColor" stroke-width="2" '
  + 'stroke-linecap="round" stroke-linejoin="round">';

const make = (body) => parse(OPEN + body + '</svg>');

const ICON_HOME = make('<path d="m3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><polyline points="9 22 9 12 15 12 15 22"/>');
const ICON_LOADS = make('<path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/>');
const ICON_TRIPS = make('<circle cx="6" cy="19" r="3"/><path d="M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15"/><circle cx="18" cy="5" r="3"/>');
const ICON_EVENTS = make('<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>');
const ICON_DRIVERS = make('<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>');
const ICON_TRUCKS = make('<path d="M14 18V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v11a1 1 0 0 0 1 1h2"/><path d="M15 18H9"/><path d="M19 18h2a1 1 0 0 0 1-1v-3.65a1 1 0 0 0-.22-.624l-3.48-4.35A1 1 0 0 0 17.52 8H14"/><circle cx="17" cy="18" r="2"/><circle cx="7" cy="18" r="2"/>');
const ICON_TRAILERS = make('<rect x="2" y="6" width="18" height="12" rx="1"/><circle cx="8" cy="20" r="2"/><circle cx="16" cy="20" r="2"/><path d="M20 12h2"/>');
const ICON_FACILITIES = make('<rect x="4" y="2" width="16" height="20" rx="2"/><path d="M9 22v-4h6v4"/><path d="M8 6h.01"/><path d="M16 6h.01"/><path d="M12 6h.01"/><path d="M12 10h.01"/><path d="M12 14h.01"/><path d="M16 10h.01"/><path d="M8 10h.01"/>');
const ICON_TERMINALS = make('<path d="M22 8.35V20a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8.35A2 2 0 0 1 3.26 6.5l8-3.2a2 2 0 0 1 1.48 0l8 3.2A2 2 0 0 1 22 8.35Z"/><path d="M6 18h12"/><path d="M6 14h12"/><path d="M6 10h12"/>');
const ICON_DOCUMENTS = make('<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/>');
const ICON_KEY = make('<path d="m15.5 7.5 2.3 2.3a1 1 0 0 0 1.4 0l2.1-2.1a1 1 0 0 0 0-1.4L19 4"/><path d="m21 2-9.6 9.6"/><circle cx="7.5" cy="15.5" r="5.5"/>');
const ICON_CHEV_UP = make('<polyline points="18 15 12 9 6 15"/>');
const ICON_THEME = make('<path d="M12 8a2.83 2.83 0 0 0 4 4 4 4 0 1 1-4-4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.9 4.9 1.4 1.4"/><path d="m17.7 17.7 1.4 1.4"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.3 17.7-1.4 1.4"/><path d="m19.1 4.9-1.4 1.4"/>');
const ICON_LOGOUT = make('<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/>');

export const homeIcon = () => ICON_HOME.cloneNode(true);
export const loadsIcon = () => ICON_LOADS.cloneNode(true);
export const tripsIcon = () => ICON_TRIPS.cloneNode(true);
export const eventsIcon = () => ICON_EVENTS.cloneNode(true);
export const driversIcon = () => ICON_DRIVERS.cloneNode(true);
export const trucksIcon = () => ICON_TRUCKS.cloneNode(true);
export const trailersIcon = () => ICON_TRAILERS.cloneNode(true);
export const facilitiesIcon = () => ICON_FACILITIES.cloneNode(true);
export const terminalsIcon = () => ICON_TERMINALS.cloneNode(true);
export const documentsIcon = () => ICON_DOCUMENTS.cloneNode(true);
export const keyIcon = () => ICON_KEY.cloneNode(true);
export const chevronUpIcon = () => ICON_CHEV_UP.cloneNode(true);
export const themeIcon = () => ICON_THEME.cloneNode(true);
export const logoutIcon = () => ICON_LOGOUT.cloneNode(true);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- icons`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/components/icons.js tests/fleet/icons.test.js
git commit -s -m "feat(fleet): add sidebar icon set

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 2: Theme module

**Files:**
- Create: `static/fleet/utils/theme.js`
- Test: `tests/fleet/theme.test.js`

Note: tests rely on the existing in-memory `localStorage` from `tests/fleet/setup.js` and stub `window.matchMedia`.

- [ ] **Step 1: Write the failing test**

```js
// tests/fleet/theme.test.js
import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  getTheme, setTheme, resolveTheme, applyTheme, initTheme,
} from '../../static/fleet/utils/theme.js';

function stubMatchMedia(matches) {
  const listeners = [];
  const mql = {
    matches,
    addEventListener: (_e, fn) => listeners.push(fn),
    _fire: (next) => { mql.matches = next; listeners.forEach(fn => fn()); },
  };
  window.matchMedia = vi.fn().mockReturnValue(mql);
  return mql;
}

beforeEach(() => {
  localStorage.clear();
  document.documentElement.removeAttribute('data-theme');
});

describe('theme', () => {
  it('defaults to system when unset or invalid', () => {
    expect(getTheme()).toBe('system');
    localStorage.setItem('fleet.theme', 'bogus');
    expect(getTheme()).toBe('system');
  });

  it('persists and reads back a valid choice', () => {
    stubMatchMedia(false);
    setTheme('dark');
    expect(getTheme()).toBe('dark');
    expect(localStorage.getItem('fleet.theme')).toBe('dark');
  });

  it('ignores invalid setTheme values', () => {
    setTheme('neon');
    expect(localStorage.getItem('fleet.theme')).toBe(null);
  });

  it('resolves system via prefers-color-scheme', () => {
    stubMatchMedia(true);
    expect(resolveTheme('system')).toBe('dark');
    stubMatchMedia(false);
    expect(resolveTheme('system')).toBe('light');
    expect(resolveTheme('dark')).toBe('dark');
  });

  it('applyTheme writes the resolved value to data-theme', () => {
    stubMatchMedia(true);
    applyTheme('system');
    expect(document.documentElement.dataset.theme).toBe('dark');
    applyTheme('light');
    expect(document.documentElement.dataset.theme).toBe('light');
  });

  it('initTheme re-applies when system preference changes', () => {
    const mql = stubMatchMedia(false);
    setTheme('system');
    initTheme();
    expect(document.documentElement.dataset.theme).toBe('light');
    mql._fire(true);
    expect(document.documentElement.dataset.theme).toBe('dark');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- theme`
Expected: FAIL — cannot resolve `static/fleet/utils/theme.js`.

- [ ] **Step 3: Write the implementation**

```js
// static/fleet/utils/theme.js
const KEY = 'fleet.theme';
const VALID = ['light', 'dark', 'system'];

export function getTheme() {
  const v = localStorage.getItem(KEY);
  return VALID.includes(v) ? v : 'system';
}

function prefersDark() {
  return typeof window.matchMedia === 'function'
    && window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function resolveTheme(choice) {
  if (choice === 'dark') return 'dark';
  if (choice === 'light') return 'light';
  return prefersDark() ? 'dark' : 'light';
}

export function applyTheme(choice = getTheme()) {
  document.documentElement.dataset.theme = resolveTheme(choice);
}

export function setTheme(choice) {
  if (!VALID.includes(choice)) return;
  localStorage.setItem(KEY, choice);
  applyTheme(choice);
}

export function initTheme() {
  applyTheme();
  if (typeof window.matchMedia === 'function') {
    const mql = window.matchMedia('(prefers-color-scheme: dark)');
    mql.addEventListener('change', () => {
      if (getTheme() === 'system') applyTheme('system');
    });
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- theme`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/utils/theme.js tests/fleet/theme.test.js
git commit -s -m "feat(fleet): add theme module (light/dark/system, persisted)

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 3: Nav config + scope-filtered renderer

**Files:**
- Create: `static/fleet/components/nav.js`
- Test: `tests/fleet/nav.test.js`

- [ ] **Step 1: Write the failing test**

```js
// tests/fleet/nav.test.js
import { describe, it, expect, beforeEach } from 'vitest';
import { NAV_GROUPS, visibleGroups, renderSidebar } from '../../static/fleet/components/nav.js';

const ALL = ['*'];
const DISPATCHER = [
  'loads:read', 'trips:read', 'events:read', 'drivers:read',
  'trucks:read', 'trailers:read', 'facilities:read', 'terminals:read', 'blobs:read',
];

describe('visibleGroups', () => {
  it('shows Home only when no scopes are present', () => {
    const groups = visibleGroups([]);
    expect(groups).toHaveLength(1);
    expect(groups[0].label).toBe(null);
    expect(groups[0].items.map(i => i.label)).toEqual(['Home']);
  });

  it('superuser sees every group and item', () => {
    const groups = visibleGroups(ALL);
    expect(groups.map(g => g.label)).toEqual([null, 'Operations', 'Fleet', 'Network', 'Admin']);
    const labels = groups.flatMap(g => g.items.map(i => i.label));
    expect(labels).toEqual([
      'Home', 'Loads', 'Trips', 'Events', 'Drivers', 'Trucks',
      'Trailers', 'Facilities', 'Terminals', 'Documents',
    ]);
  });

  it('drops a group whose every item is scope-hidden', () => {
    // No drivers/trucks/trailers scopes -> Fleet group disappears entirely.
    const scopes = ['loads:read'];
    const groups = visibleGroups(scopes);
    expect(groups.map(g => g.label)).toEqual([null, 'Operations']);
    expect(groups.find(g => g.label === 'Operations').items.map(i => i.label)).toEqual(['Loads']);
  });

  it('dispatcher sees all operational groups', () => {
    const groups = visibleGroups(DISPATCHER);
    expect(groups.map(g => g.label)).toEqual([null, 'Operations', 'Fleet', 'Network', 'Admin']);
  });
});

describe('renderSidebar', () => {
  let host;
  beforeEach(() => { host = document.createElement('div'); });

  it('renders data-link anchors with icon + label', () => {
    renderSidebar(host, { scopes: ALL, pathname: '/fleet/loads' });
    const links = host.querySelectorAll('a.sidebar__link');
    expect(links.length).toBe(10);
    const loads = [...links].find(a => a.getAttribute('href') === '/fleet/loads');
    expect(loads.hasAttribute('data-link')).toBe(true);
    expect(loads.querySelector('svg')).not.toBe(null);
    expect(loads.textContent).toContain('Loads');
  });

  it('marks the current path active', () => {
    renderSidebar(host, { scopes: ALL, pathname: '/fleet/loads' });
    const active = host.querySelectorAll('.sidebar__link--active');
    expect(active.length).toBe(1);
    expect(active[0].getAttribute('href')).toBe('/fleet/loads');
  });

  it('renders group headers only for non-empty labelled groups', () => {
    renderSidebar(host, { scopes: ['loads:read'], pathname: '/fleet/home' });
    const headers = [...host.querySelectorAll('.sidebar__group-label')].map(h => h.textContent);
    expect(headers).toEqual(['Operations']);
  });

  it('clears prior content on re-render', () => {
    renderSidebar(host, { scopes: ALL, pathname: '' });
    renderSidebar(host, { scopes: [], pathname: '' });
    expect(host.querySelectorAll('a.sidebar__link').length).toBe(1); // Home only
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- nav`
Expected: FAIL — cannot resolve `static/fleet/components/nav.js`.

- [ ] **Step 3: Write the implementation**

```js
// static/fleet/components/nav.js
import {
  homeIcon, loadsIcon, tripsIcon, eventsIcon, driversIcon,
  trucksIcon, trailersIcon, facilitiesIcon, terminalsIcon, documentsIcon,
} from './icons.js';
import { scopeGranted } from './scope-gate.js';

export const NAV_GROUPS = [
  { label: null, items: [
    { label: 'Home', path: '/fleet/home', icon: homeIcon },
  ] },
  { label: 'Operations', items: [
    { label: 'Loads',  path: '/fleet/loads',  icon: loadsIcon,  scope: 'loads:read' },
    { label: 'Trips',  path: '/fleet/trips',  icon: tripsIcon,  scope: 'trips:read' },
    { label: 'Events', path: '/fleet/events', icon: eventsIcon, scope: 'events:read' },
  ] },
  { label: 'Fleet', items: [
    { label: 'Drivers',  path: '/fleet/drivers',  icon: driversIcon,  scope: 'drivers:read' },
    { label: 'Trucks',   path: '/fleet/trucks',   icon: trucksIcon,   scope: 'trucks:read' },
    { label: 'Trailers', path: '/fleet/trailers', icon: trailersIcon, scope: 'trailers:read' },
  ] },
  { label: 'Network', items: [
    { label: 'Facilities', path: '/fleet/facilities', icon: facilitiesIcon, scope: 'facilities:read' },
    { label: 'Terminals',  path: '/fleet/terminals',  icon: terminalsIcon,  scope: 'terminals:read' },
  ] },
  { label: 'Admin', items: [
    { label: 'Documents', path: '/fleet/documents', icon: documentsIcon, scope: 'blobs:read' },
  ] },
];

export function visibleGroups(scopes) {
  return NAV_GROUPS
    .map(g => ({
      label: g.label,
      items: g.items.filter(it => !it.scope || scopeGranted(scopes, it.scope)),
    }))
    .filter(g => g.items.length > 0);
}

export function renderSidebar(container, { scopes = [], pathname = '' } = {}) {
  container.replaceChildren();
  for (const group of visibleGroups(scopes)) {
    if (group.label) {
      const header = document.createElement('div');
      header.className = 'sidebar__group-label';
      header.textContent = group.label;
      container.appendChild(header);
    }
    for (const item of group.items) {
      const a = document.createElement('a');
      a.className = 'sidebar__link';
      a.dataset.link = '';
      a.setAttribute('href', item.path);
      if (item.path === pathname) a.classList.add('sidebar__link--active');

      const iconWrap = document.createElement('span');
      iconWrap.className = 'sidebar__icon';
      iconWrap.appendChild(item.icon());

      const label = document.createElement('span');
      label.textContent = item.label;

      a.append(iconWrap, label);
      container.appendChild(a);
    }
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- nav`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/components/nav.js tests/fleet/nav.test.js
git commit -s -m "feat(fleet): scope-gated sidebar nav config + renderer

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 4: Account footer with pull-up menu

**Files:**
- Create: `static/fleet/components/account-footer.js`
- Test: `tests/fleet/account-footer.test.js`

The footer's Account entry is a `data-link` anchor (the router intercepts it, same as nav links), so no router import is needed. Sign out calls an injected `onSignOut` callback. The theme control uses `theme.js`.

- [ ] **Step 1: Write the failing test**

```js
// tests/fleet/account-footer.test.js
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { initials, roleLabel, renderAccountFooter } from '../../static/fleet/components/account-footer.js';

beforeEach(() => {
  localStorage.clear();
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener() {} });
});

describe('initials', () => {
  it('takes first letters of the first two name words', () => {
    expect(initials('Jim Phillips', 'x@y.com')).toBe('JP');
  });
  it('handles a single-word name', () => {
    expect(initials('Jim', 'x@y.com')).toBe('J');
  });
  it('falls back to the email initial when name is blank', () => {
    expect(initials('', 'dispatch@acme.com')).toBe('D');
  });
  it('returns ? when nothing is available', () => {
    expect(initials('', '')).toBe('?');
  });
});

describe('roleLabel', () => {
  it('title-cases known roles', () => {
    expect(roleLabel('owner')).toBe('Owner');
    expect(roleLabel('fleet_manager')).toBe('Fleet Manager');
    expect(roleLabel('dispatcher')).toBe('Dispatcher');
  });
  it('passes through an unknown role', () => {
    expect(roleLabel('auditor')).toBe('auditor');
  });
});

describe('renderAccountFooter', () => {
  const identity = { name: 'Jim Phillips', email: 'jim@acme.com', role: 'owner' };
  let host;
  beforeEach(() => { host = document.createElement('div'); document.body.appendChild(host); });

  it('renders the user chip with initials, name and role', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    expect(host.querySelector('.sidebar__avatar').textContent).toBe('JP');
    expect(host.textContent).toContain('Jim Phillips');
    expect(host.textContent).toContain('Owner');
  });

  it('menu starts closed and toggles open on chip click', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    const menu = host.querySelector('.sidebar__menu');
    expect(menu.hidden).toBe(true);
    host.querySelector('.sidebar__account').click();
    expect(menu.hidden).toBe(false);
  });

  it('closes on Escape and on outside click', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    const menu = host.querySelector('.sidebar__menu');
    host.querySelector('.sidebar__account').click();
    expect(menu.hidden).toBe(false);
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    expect(menu.hidden).toBe(true);
    host.querySelector('.sidebar__account').click();
    document.body.click();
    expect(menu.hidden).toBe(true);
  });

  it('shows the Account link only with api_keys:read', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    expect(host.querySelector('a[href="/fleet/account"]')).not.toBe(null);

    const host2 = document.createElement('div');
    renderAccountFooter(host2, { identity, scopes: ['loads:read'], onSignOut() {} });
    expect(host2.querySelector('a[href="/fleet/account"]')).toBe(null);
  });

  it('invokes onSignOut when Sign out is clicked', () => {
    const onSignOut = vi.fn();
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut });
    host.querySelector('.sidebar__account').click();
    host.querySelector('[data-action="sign-out"]').click();
    expect(onSignOut).toHaveBeenCalledTimes(1);
  });

  it('theme buttons mark the active choice', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    const dark = host.querySelector('[data-theme-choice="dark"]');
    dark.click();
    expect(dark.classList.contains('is-active')).toBe(true);
    expect(localStorage.getItem('fleet.theme')).toBe('dark');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- account-footer`
Expected: FAIL — cannot resolve `static/fleet/components/account-footer.js`.

- [ ] **Step 3: Write the implementation**

```js
// static/fleet/components/account-footer.js
import { keyIcon, chevronUpIcon, logoutIcon } from './icons.js';
import { scopeGranted } from './scope-gate.js';
import { getTheme, setTheme } from '../utils/theme.js';

const ROLE_LABELS = {
  owner: 'Owner',
  fleet_manager: 'Fleet Manager',
  dispatcher: 'Dispatcher',
};

export function initials(name, email) {
  const n = (name || '').trim();
  if (n) {
    return n.split(/\s+/).slice(0, 2).map(w => w[0].toUpperCase()).join('');
  }
  const e = (email || '').trim();
  return e ? e[0].toUpperCase() : '?';
}

export function roleLabel(role) {
  return ROLE_LABELS[role] || (role || '');
}

const THEME_CHOICES = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System' },
];

function buildThemeSwitch() {
  const wrap = document.createElement('div');
  wrap.className = 'sidebar__theme';
  const current = getTheme();
  const buttons = [];
  for (const choice of THEME_CHOICES) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'sidebar__theme-btn' + (choice.value === current ? ' is-active' : '');
    btn.dataset.themeChoice = choice.value;
    btn.textContent = choice.label;
    btn.addEventListener('click', () => {
      setTheme(choice.value);
      buttons.forEach(b => b.classList.toggle('is-active', b === btn));
    });
    buttons.push(btn);
    wrap.appendChild(btn);
  }
  return wrap;
}

export function renderAccountFooter(container, { identity, scopes = [], onSignOut } = {}) {
  const id = identity || {};
  container.replaceChildren();

  const menu = document.createElement('div');
  menu.className = 'sidebar__menu';
  menu.hidden = true;

  if (scopeGranted(scopes, 'api_keys:read')) {
    const account = document.createElement('a');
    account.className = 'sidebar__menu-item';
    account.dataset.link = '';
    account.setAttribute('href', '/fleet/account');
    account.appendChild(keyIcon());
    account.appendChild(document.createTextNode('Account'));
    menu.appendChild(account);
  }

  menu.appendChild(buildThemeSwitch());

  const signOut = document.createElement('button');
  signOut.type = 'button';
  signOut.className = 'sidebar__menu-item';
  signOut.dataset.action = 'sign-out';
  signOut.appendChild(logoutIcon());
  signOut.appendChild(document.createTextNode('Sign out'));
  signOut.addEventListener('click', () => { if (onSignOut) onSignOut(); });
  menu.appendChild(signOut);

  const chip = document.createElement('button');
  chip.type = 'button';
  chip.className = 'sidebar__account';

  const avatar = document.createElement('span');
  avatar.className = 'sidebar__avatar';
  avatar.textContent = initials(id.name, id.email);

  const meta = document.createElement('span');
  meta.className = 'sidebar__account-meta';
  const nameEl = document.createElement('span');
  nameEl.className = 'sidebar__account-name';
  nameEl.textContent = id.name || id.email || 'Signed in';
  const roleEl = document.createElement('span');
  roleEl.className = 'sidebar__account-role';
  roleEl.textContent = roleLabel(id.role);
  meta.append(nameEl, roleEl);

  const chev = document.createElement('span');
  chev.className = 'sidebar__account-chev';
  chev.appendChild(chevronUpIcon());

  chip.append(avatar, meta, chev);

  const close = () => {
    menu.hidden = true;
    document.removeEventListener('keydown', onKey);
    document.removeEventListener('click', onOutside);
  };
  const onKey = (e) => { if (e.key === 'Escape') close(); };
  const onOutside = (e) => { if (!container.contains(e.target)) close(); };
  const open = () => {
    menu.hidden = false;
    document.addEventListener('keydown', onKey);
    // Defer outside-click wiring so the opening click doesn't immediately close it.
    setTimeout(() => document.addEventListener('click', onOutside), 0);
  };

  chip.addEventListener('click', () => {
    if (menu.hidden) open(); else close();
  });

  container.append(menu, chip);
}
```

Note: the outside-click test clicks `document.body` synchronously; the `setTimeout(…, 0)` defer means the listener may not be attached yet under fake timing. Use a guard instead of a timer so the test passes deterministically — replace the `open()` body's `setTimeout(...)` line with a direct add plus an "ignore the originating click" flag:

```js
  let justOpened = false;
  const onOutside = (e) => {
    if (justOpened) { justOpened = false; return; }
    if (!container.contains(e.target)) close();
  };
  const open = () => {
    menu.hidden = false;
    justOpened = true;
    document.addEventListener('keydown', onKey);
    document.addEventListener('click', onOutside);
  };
```

Use this guard version (delete the `setTimeout` variant above) so behavior is deterministic in tests and the browser alike.

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- account-footer`
Expected: PASS (all describe blocks green).

- [ ] **Step 5: Commit**

```bash
git add static/fleet/components/account-footer.js tests/fleet/account-footer.test.js
git commit -s -m "feat(fleet): account footer with pull-up menu + theme switch

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 5: Wire the chrome into the shell (index.html + app.js)

**Files:**
- Modify: `static/fleet/index.html:102-123` (sidebar block)
- Modify: `static/fleet/app.js` (imports, `enterApp`, `boot`, remove old `initSidebar`)

- [ ] **Step 1: Replace the static sidebar block in `index.html`**

Replace lines 102–123 (the `<nav class="sidebar">…</nav>` block) with:

```html
    <!-- Sidebar navigation -->
    <nav class="sidebar">
      <div class="sidebar__logo">Ollie Fleet</div>
      <nav class="sidebar__nav" id="sidebar-nav"></nav>
      <div class="sidebar__footer" id="sidebar-footer"></div>
    </nav>
```

- [ ] **Step 2: Update `app.js` imports**

Replace the existing `import { renderAccountView } from './pages/account.js';` line by keeping it, and add these imports near the other component imports (after the `utils/dom.js` import):

```js
import { renderSidebar } from './components/nav.js';
import { renderAccountFooter } from './components/account-footer.js';
import { initTheme } from './utils/theme.js';
import { getScopes, getIdentity } from './utils/api.js';
```

Note: `getScopes`/`getIdentity` come from `utils/api.js`; extend the existing `import { tryRefresh, loadMe, clearMe, setOnUnauthorized } from './utils/api.js';` line to also import them rather than adding a duplicate import:

```js
import {
  tryRefresh, loadMe, clearMe, setOnUnauthorized, getScopes, getIdentity,
} from './utils/api.js';
```

(Use the single combined import; do not also add a separate `getScopes/getIdentity` import line.)

- [ ] **Step 3: Add a `renderChrome` helper and the sign-out flow to `app.js`**

Add, in the "Sidebar & logout" section, replacing the entire existing `initSidebar` function:

```js
// ─── Sidebar & account footer ────────────────────────────────

async function signOut() {
  await fetch('/fleet/auth/logout', {
    method: 'POST',
    credentials: 'same-origin',
  }).catch(() => {});
  clearToken();
  clearMe();
  clearEventsRefresh();
  showLogin();
}

// Render the scope-gated nav + account footer from the current /me snapshot.
// Safe to call repeatedly (boot, login, tab refocus).
function renderChrome() {
  const navEl = document.getElementById('sidebar-nav');
  const footerEl = document.getElementById('sidebar-footer');
  if (navEl) {
    renderSidebar(navEl, { scopes: getScopes(), pathname: window.location.pathname });
  }
  if (footerEl) {
    renderAccountFooter(footerEl, {
      identity: getIdentity(),
      scopes: getScopes(),
      onSignOut: signOut,
    });
  }
}
```

`clearToken` is already imported at the top of `app.js` (`import { isAuthenticated, clearToken } from './utils/auth.js';`) — no new import needed for it.

- [ ] **Step 4: Render chrome inside `enterApp` and drop `initSidebar` from `boot`**

In `enterApp`, call `renderChrome()` after `showApp()`:

```js
function enterApp() {
  showApp();
  renderChrome();
  if (!routerStarted) {
    routerStarted = true;
    startRouter(renderRoute);
  } else {
    renderRoute(matchRoute(window.location.pathname + window.location.search));
  }
}
```

In `boot`, remove the `initSidebar();` call and add `initTheme();` early. Update the `visibilitychange` handler to re-render the chrome after `/me` reloads:

```js
async function boot() {
  initTheme();
  initLoginForm(enterApp);
  initSetupForm(enterApp);

  setOnUnauthorized(() => {
    clearMe();
    showLogin();
  });

  document.addEventListener('visibilitychange', async () => {
    if (document.visibilityState === 'visible' && isAuthenticated()) {
      await loadMe();
      renderChrome();
    }
  });

  if (isAuthenticated()) {
    await loadMe();
    enterApp();
  } else {
    const refreshed = await tryRefresh();
    if (refreshed) {
      await loadMe();
      enterApp();
    } else {
      await showLoginOrSetup();
    }
  }
}
```

- [ ] **Step 5: Keep the active-link sync in `renderRoute`**

The existing block in `renderRoute` already re-applies `.sidebar__link--active` on every navigation and works against the rendered anchors — leave it unchanged:

```js
  document.querySelectorAll('.sidebar__link[href]').forEach((a) => {
    a.classList.toggle('sidebar__link--active', a.getAttribute('href') === window.location.pathname);
  });
```

- [ ] **Step 6: Run the full JS suite**

Run: `npm test`
Expected: PASS — all existing fleet tests plus the four new files. No test imports the removed `initSidebar`.

- [ ] **Step 7: Commit**

```bash
git add static/fleet/index.html static/fleet/app.js
git commit -s -m "feat(fleet): render scope-gated sidebar + account footer in shell

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 6: Sidebar, footer, and theme-stub CSS

**Files:**
- Modify: `static/fleet/css/components.css` (sidebar/footer rules)
- Modify: `static/fleet/css/base.css` (`[data-theme]` token blocks)

No automated test (CSS). Verified visually in Task 7.

- [ ] **Step 1: Add the dark-theme stub to `base.css`**

After the closing `}` of the `:root { … }` block (before `[hidden] { … }`), add:

```css
/* Dark palette is deferred: the switcher is wired and sets [data-theme="dark"],
   but these values intentionally mirror light until the dark palette lands.
   Filling these in is the only step needed to enable dark mode. */
[data-theme="dark"] {
  /* TODO(dark-palette follow-on): real dark values. Mirrors light for now. */
  --color-bg: #f9fafb;
  --color-surface: #ffffff;
  --color-surface-2: #f3f4f6;
  --color-text: #111827;
  --color-text-muted: #6b7280;
  --color-border: #e5e7eb;
}
```

(This is the single allowed placeholder in the plan — the spec deliberately defers the dark palette. The comment documents it as intentional, not forgotten.)

- [ ] **Step 2: Add the icon + footer rules to `components.css`**

After the `.sidebar__footer { … }` rule, add:

```css
.sidebar__icon {
  display: inline-flex;
  flex: none;
  width: 18px;
  height: 18px;
}

.sidebar__icon svg {
  width: 18px;
  height: 18px;
}

/* Account footer */
.sidebar__footer {
  position: relative;
}

.sidebar__account {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  width: 100%;
  padding: var(--space-2);
  border: none;
  background: none;
  border-radius: var(--radius-sm);
  cursor: pointer;
  text-align: left;
  color: var(--color-text);
}

.sidebar__account:hover {
  background: var(--color-surface-2);
}

.sidebar__avatar {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 32px;
  height: 32px;
  border-radius: var(--radius-pill);
  background: var(--color-primary-soft);
  color: var(--color-primary-dark);
  font-size: 0.75rem;
  font-weight: 700;
}

.sidebar__account-meta {
  display: flex;
  flex-direction: column;
  min-width: 0;
  flex: 1;
}

.sidebar__account-name {
  font-size: 0.8125rem;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sidebar__account-role {
  font-size: 0.6875rem;
  color: var(--color-text-muted);
}

.sidebar__account-chev {
  display: inline-flex;
  flex: none;
  width: 16px;
  height: 16px;
  color: var(--color-text-subtle);
}

.sidebar__account-chev svg { width: 16px; height: 16px; }

.sidebar__menu {
  position: absolute;
  left: var(--space-3);
  right: var(--space-3);
  bottom: calc(100% - var(--space-2));
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: var(--radius);
  box-shadow: var(--shadow-popover);
  padding: var(--space-2);
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  z-index: 200;
}

.sidebar__menu-item {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  width: 100%;
  padding: var(--space-2) var(--space-3);
  border: none;
  background: none;
  border-radius: var(--radius-sm);
  font-size: 0.8125rem;
  color: var(--color-text);
  text-decoration: none;
  cursor: pointer;
  text-align: left;
}

.sidebar__menu-item:hover {
  background: var(--color-surface-2);
}

.sidebar__menu-item svg {
  width: 16px;
  height: 16px;
  flex: none;
}

.sidebar__theme {
  display: flex;
  gap: var(--space-1);
  padding: var(--space-1) var(--space-2) var(--space-2);
  border-bottom: 1px solid var(--color-border);
  margin-bottom: var(--space-1);
}

.sidebar__theme-btn {
  flex: 1;
  padding: var(--space-1) var(--space-2);
  font-size: 0.6875rem;
  font-weight: 600;
  border: 1px solid var(--color-border);
  background: var(--color-surface);
  border-radius: var(--radius-xs);
  color: var(--color-text-muted);
  cursor: pointer;
}

.sidebar__theme-btn.is-active {
  background: var(--color-primary-soft);
  color: var(--color-primary-dark);
  border-color: var(--color-primary);
}
```

- [ ] **Step 3: Commit**

```bash
git add static/fleet/css/base.css static/fleet/css/components.css
git commit -s -m "style(fleet): sidebar icon, account footer, theme-stub CSS

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Task 7: Verify in the running app

**Files:** none (manual verification)

- [ ] **Step 1: Run the full suite once more**

Run: `npm test`
Expected: PASS — entire `tests/fleet/**` suite green.

- [ ] **Step 2: Launch the app and sign in as an owner**

Use the project's run path (the `/run` skill or the documented server start). Sign in as an owner/superuser.
Expected:
- Sidebar shows Home, then **Operations** (Loads/Trips/Events), **Fleet** (Drivers/Trucks/Trailers), **Network** (Facilities/Terminals), **Admin** (Documents) — each with a leading icon.
- No "Equipment" label, no Account link in the list.
- Footer shows an initials avatar, name, and "Owner"; clicking it opens a pull-up with Account, a Light/Dark/System switch, and Sign out.
- Active link highlights as you navigate; switching theme to Dark sets `data-theme="dark"` on `<html>` (palette unchanged for now); the choice survives reload.

- [ ] **Step 3: Verify scope gating with a dispatcher**

Sign in as a Dispatcher (no `users:*`, but all operational `*:read`).
Expected: all four groups still visible (dispatchers hold every `*:read` in the table). The Account menu item is present (dispatchers have `api_keys:read`). If you have a user with a deliberately reduced scope set, confirm groups with all-hidden items disappear (header included) and Home always remains.

- [ ] **Step 4: Final commit (only if Step 2/3 surfaced fixes)**

If verification required changes, commit them:

```bash
git add -A
git commit -s -m "fix(fleet): nav revamp verification fixes

Co-Authored-By: Claude with claude-opus-4-8[1m] <noreply@anthropic.com>"
```

---

## Self-review notes

- **Spec coverage:** groups (Task 3), scope-gating + empty-group hiding + Home-always (Task 3 tests), icons (Task 1), account footer + pull-up + initials/role (Task 4), theme switcher wired with deferred palette (Task 2 + Task 6 stub), JS-rendered single-config nav (Task 3), wiring + re-render on `/me` refresh (Task 5), Documents in Admin (Task 3 config). All spec sections map to a task.
- **Type/name consistency:** `renderSidebar(container, {scopes, pathname})`, `renderAccountFooter(container, {identity, scopes, onSignOut})`, `visibleGroups(scopes)`, `initials(name, email)`, `roleLabel(role)`, `getTheme/setTheme/resolveTheme/applyTheme/initTheme` — used identically across tasks and tests. Icon factory names match between `icons.js`, `nav.js`, and `account-footer.js`.
- **Placeholders:** the only `TODO` is the intentional deferred dark palette (Task 6 Step 1), called out by the spec's non-goals; every other step contains complete code.
- **Version stamps:** deliberately untouched (repo convention: only `cut-release` bumps `?v=`).
