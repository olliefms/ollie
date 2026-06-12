# Fleet Sidebar Nav Revamp — Design

**Date:** 2026-06-12
**Status:** Draft (pending user review)

## Problem

The fleet SPA's left-nav sidebar (`static/fleet/index.html` lines 102–123) has
grown to 11 flat links since the first release and is now disorderly. It carries
a single group label — **"Equipment"** — that is mis-scoped: it sits above
Trucks/Trailers but has no closing structure, so Events, Documents, Terminals,
and Account visually fall under "Equipment" too, none of which are equipment.

Three structural problems compound this:

- **No grouping.** Eleven sibling `<a>` links with one inaccurate header. The
  back half of the nav is an undifferentiated tail.
- **No icons.** Links are text-only `<span>`s — nothing to scan by.
- **No authority awareness.** The sidebar renders all 11 links unconditionally
  even though every underlying endpoint is already scope-guarded server-side. A
  dispatcher who lacks `users:*` still sees links to surfaces they can't use,
  and the nav can't grow into role-gated areas (e.g. user management) cleanly.

Separately, **Account** is a full nav link today and **Sign out** is a bare
button in the footer. Both belong in a single persistent account control rather
than competing with operational nav items.

## Goals

- Replace the flat list + mis-scoped "Equipment" label with **four honest
  groups** — Operations, Fleet, Network, Admin — plus an ungrouped **Home**.
- **Scope-gate** every nav item against the scope its backend already enforces;
  hide items (and empty group headers) the signed-in user lacks.
- Add **leading icons** to every item, reusing the driver surface's
  inline-SVG pattern.
- Move **Account** and **Sign out** into a **bottom account footer**
  (Claude-Desktop style): initials avatar + name + role, with a pull-up menu
  for Account, Theme, and Sign out.
- Add a **theme switcher** control (light / dark / system) in the footer menu —
  wired to persist and drive `[data-theme]`, but shipping with the existing
  light palette only (dark palette is a deferred follow-on).
- Convert the sidebar from static HTML to a **JS-rendered nav driven by one
  config array**, so structure, icons, grouping, and scope-gating have a single
  source of truth.

## Non-goals

- The actual **dark color palette.** The switcher is built and functional
  (persists choice, sets `data-theme`, follows system preference), but only the
  current light tokens exist. Filling in the dark token block is a separate,
  small follow-on.
- A **user-management surface.** Admin is structured to receive it later; this
  work does not build it.
- **Avatar images.** `MeResponse` carries no avatar URL; the footer renders an
  initials chip from `me.name`.
- Migrating the **driver surface** or any non-fleet nav.
- Changing any **backend** authorization. Gating is a UX layer over guards that
  already exist.

## Decisions (resolved during brainstorming)

| Topic | Decision |
|---|---|
| Grouping | 4 groups: **Operations** (Loads, Trips, Events), **Fleet** (Drivers, Trucks, Trailers), **Network** (Facilities, Terminals), **Admin** (Documents). **Home** ungrouped at top. |
| "Equipment" label | Removed; replaced by the four groups above. |
| Documents | Stays in **Admin** — now holds cross-cutting records (equipment maintenance receipts, etc.), not just load docs. |
| Account | Leaves the nav list; becomes the **bottom account footer** + pull-up menu. |
| Sign out | Moves from bare footer button into the account pull-up menu. |
| Theme switcher | In the account pull-up: light / dark / system. **Wired now, light-only palette** (dark deferred). |
| Nav rendering | **JS-rendered from a single config array** (`{label, path, icon, scope, group}`), mirroring driver `bottom-nav.js`. |
| Scope source | Existing `getScopes()` / `hasScope()` from `utils/api.js`, populated by `loadMe()` against `GET /fleet/api/v1/me`. No backend change. |
| Empty groups | A group whose items are all scope-hidden hides its header too. |
| Home gating | **Always visible.** Its KPI tiles already degrade per-scope. |
| Icons | Reuse the driver inline-SVG approach; new `static/fleet/components/icons.js`. |
| Avatar | Initials derived from `me.name` (no image). |

## Information architecture

```
  Home                              (ungrouped, always visible)

  OPERATIONS
    Loads        loads:read
    Trips        trips:read
    Events       events:read

  FLEET
    Drivers      drivers:read
    Trucks       trucks:read
    Trailers     trailers:read

  NETWORK
    Facilities   facilities:read
    Terminals    terminals:read

  ADMIN
    Documents    blobs:read
    (future: Users → users:read, etc.)

  ─────────────────────────────────────
  [ JP ]  Jim Phillips        ▲        account footer (pinned bottom)
          Owner                        └─ pull-up menu:
                                          · Account    (→ /fleet/account, api_keys:read)
                                          · Theme      (light / dark / system)
                                          · Sign out
```

**Why these groups.** Operations is the live dispatch workflow (the freight,
the movements, the activity timeline) and leads. Fleet is the assignable
resources you attach to a trip — drivers, trucks, trailers. Network is physical
places — customer facilities and own terminals. Admin is records and
configuration, low-frequency, parked at the bottom and built to grow.

## Scope mapping

Each nav item declares the read scope its list/read endpoint already enforces
server-side. Source of truth verified in `src/api/fleet_portal/data.rs` and
siblings; scope vocabulary in `src/models/permission.rs`.

| Item | Path | Gating scope |
|---|---|---|
| Home | `/fleet/home` | *(none — always shown)* |
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

**Matcher semantics.** Gating reuses `scopeGranted()` (`components/scope-gate.js`):
a required `r:read` is satisfied by exact match, the per-resource wildcard
`r:*`, or the global superuser `*`. Owners/FleetManagers hold `*` and see
everything; Dispatchers see all of Operations/Fleet/Network + Documents +
Account, and would not see a future `users:*`-gated Admin item.

**Fail-safe.** `getScopes()` returns `[]` until `/me` resolves, so the nav
renders Home-only on first paint and fills in once scopes load. This is
fail-closed: a missing/expired `/me` shows fewer links, never more.

## Rendering architecture

The sidebar moves from static markup to a small JS-rendered component, the same
shape as the driver surface's `bottom-nav.js`.

**`static/fleet/components/nav.js`** — exports a `NAV_GROUPS` config and a
`renderSidebar(container)` function.

```js
// shape (illustrative)
const NAV_GROUPS = [
  { label: null,         items: [ { label: 'Home', path: '/fleet/home', icon: homeIcon } ] }, // ungrouped
  { label: 'Operations', items: [
      { label: 'Loads',  path: '/fleet/loads',  icon: loadIcon,   scope: 'loads:read' },
      { label: 'Trips',  path: '/fleet/trips',  icon: tripIcon,   scope: 'trips:read' },
      { label: 'Events', path: '/fleet/events', icon: eventIcon,  scope: 'events:read' },
  ] },
  // Fleet, Network, Admin …
];
```

`renderSidebar`:
1. For each group, filter `items` by `hasScope(item.scope)` (items with no
   `scope` always pass).
2. Drop any group left with zero visible items (header included).
3. Render each surviving group: an optional `.sidebar__group-label` header, then
   `<a class="sidebar__link" data-link href=…>` with the icon SVG before the
   `<span>` label. (`data-link` + `.sidebar__link` keep the existing router
   click-interception and active-highlight working unchanged.)

**Re-render trigger.** The nav is (re)rendered after `loadMe()` resolves —
on boot, browser refresh, token refresh, and `visibilitychange` — so scope
changes (role edits take effect without re-issuing tokens) reflect without a
hard reload. Active-link highlighting stays in `renderRoute` (`app.js`),
unchanged: it toggles `.sidebar__link--active` by `pathname` after each render.

**`static/fleet/components/icons.js`** — new, copying the driver surface's
`DOMParser`-based inline-SVG helpers (Lucide-style, 24×24,
`stroke="currentColor"` so they inherit `.sidebar__link` color). One icon per
nav item plus a chevron and a theme glyph for the footer.

## Account footer

Replaces the `.sidebar__footer` logout button and the Account nav link.

**`static/fleet/components/account-footer.js`** — `renderAccountFooter(container)`:

- **User chip:** an initials avatar (first letter of each of the first two
  words of `me.name`, e.g. "Jim Phillips" → "JP"; fallback to email initial),
  `me.name` (or email), and `me.role` (title-cased: Owner / Fleet Manager /
  Dispatcher), pulled from `getIdentity()`.
- **Pull-up menu** (toggled by clicking the chip / chevron), items:
  - **Account** → navigates to `/fleet/account` (the existing API-keys page).
    Shown only when `hasScope('api_keys:read')`.
  - **Theme** → a light / dark / system control (see below).
  - **Sign out** → the existing logout flow (`POST /fleet/auth/logout`,
    `clearToken()`, `clearMe()`, `clearEventsRefresh()`, `showLogin()`), moved
    verbatim out of `initSidebar`.

The menu closes on outside-click and on Escape. The `/fleet/account` route and
its page module are unchanged — only its entry point moves from a top-nav link
to the footer menu.

## Theming (switcher built, palette deferred)

**`static/fleet/utils/theme.js`** — `getTheme()`, `setTheme('light'|'dark'|'system')`:

- Persists the choice in `localStorage` (key `fleet.theme`).
- Applies it by setting `document.documentElement.dataset.theme` to `light` or
  `dark`; `system` resolves via `matchMedia('(prefers-color-scheme: dark)')` and
  subscribes to changes.
- Called once on boot (before first paint where practical) and whenever the
  footer switcher changes.

**CSS.** `base.css` gains a `[data-theme="dark"]` token block *stub* — present
and structurally correct but, for this work, holding the same values as light
(or the minimal set needed to avoid a broken-looking switch). The full dark
palette is the deferred follow-on; nothing else in this spec depends on it.
`[data-theme="light"]` / default is the current `:root` token set.

## CSS changes (`static/fleet/css/components.css`)

- `.sidebar__link` already has `gap` + `align-items:center` — the icon SVG drops
  in before the `<span>` with no rule change. Add an `.sidebar__icon` sizing
  rule (e.g. 18–20px, `flex: none`) for consistency.
- Group containers: optionally wrap each group so spacing/structure is bound to
  its header (today the label is a bare sibling). Keep `.sidebar__group-label`
  styling.
- New `.sidebar__footer` layout for the user chip + chevron, the initials
  `.sidebar__avatar`, and the pull-up `.sidebar__menu` (absolute-positioned
  above the footer, surface bg, shadow, divider). Reuse existing `--color-*`,
  `--space-*`, `--radius-*`, `--shadow-*` tokens.

## Files touched

**New**
- `static/fleet/components/nav.js` — config + `renderSidebar`
- `static/fleet/components/icons.js` — inline-SVG icon helpers (ported)
- `static/fleet/components/account-footer.js` — user chip + pull-up menu
- `static/fleet/utils/theme.js` — theme get/set/apply + system subscription
- Test files under the existing Vitest layout for each new module

**Modified**
- `static/fleet/index.html` — replace static `<nav class="sidebar__nav">` block
  and `.sidebar__footer` with mount points; bump `?v=` asset stamps per release
  convention
- `static/fleet/app.js` — call `renderSidebar` + `renderAccountFooter` after
  `loadMe()`; remove the inline logout wiring from `initSidebar` (moves to the
  footer component); apply theme on boot
- `static/fleet/css/base.css` — `[data-theme]` token blocks (light real, dark
  stub)
- `static/fleet/css/components.css` — icon, group, footer, menu rules

**Unchanged**
- All backend authorization (`src/api/fleet_portal/*`, `src/models/permission.rs`)
- `router.js` (the new `<a data-link>` links use the existing interception)
- `/fleet/account` page module and route

## Testing

Vitest + happy-dom (the toolkit established in the Fleet UI CRUD project):

- **nav.js** — given a scope set, renders exactly the expected links; hides
  items lacking scope; hides a group header when all its items are hidden; Home
  always present; superuser `*` shows everything; empty scopes → Home only.
- **account-footer.js** — initials derived correctly from name/email; role
  title-casing; Account item hidden without `api_keys:read`; menu open/close on
  toggle, outside-click, Escape; Sign out invokes the logout flow.
- **theme.js** — persists and reads back; `system` resolves via matchMedia mock
  and reacts to change events; sets `data-theme` on the root.
- Active-link highlight still applies after a scope-driven re-render.

## Open questions

None blocking. The dark palette values are intentionally deferred; everything
else is decided above.

## Follow-ons (out of scope)

- Fill in the **dark color palette** in `base.css` (the switcher already drives
  it).
- Build the **Users / user-management** Admin surface (`users:*`-gated), which
  slots into the Admin group via one `NAV_GROUPS` entry.
- Consider surfacing **Documents contextually** on load/trip detail pages in
  addition to the Admin link.
