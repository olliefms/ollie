# Fleet favicon + unified sticky topbar header — Design

**Date:** 2026-06-12
**Scope:** `static/fleet/` (header work) + `static/fleet/` & `static/driver/` (favicon)
**Status:** Approved, ready for implementation plan

## Problem

Two aesthetic issues in the shipped UI:

1. **No favicon.** Neither the fleet SPA nor the driver PWA links a favicon; browser
   tabs show the generic default. A brand mark already exists (white "O" monogram on
   brand blue) as the driver PWA icons (`icon-192.png`/`icon-512.png`), but it is never
   wired as a favicon.

2. **Duplicate header on list views.** `app.js` `renderRoute` already sets the fixed
   topbar title (`#topbar-title`) for every view from `VIEW_TITLES`. List pages then
   *also* render a `.page-header` containing an `<h1 class="page-title">` with the same
   word plus the filters/Create button in `.page-controls`. Result: e.g. "Trips" shows
   twice, and the stack of `.content` top padding (`--space-5`) + `.page-header` +
   its `margin-bottom: --space-5` leaves a ~0.5" dead band above the table.

## Solution overview

- **Favicon:** add an SVG favicon (the existing "O" monogram) + apple-touch PNG links to
  both apps.
- **Header:** hoist the list controls (filters + Create button) into the existing sticky
  topbar next to the title, drop the in-body `<h1>`, and tighten the content top padding.
  The topbar is `position: fixed`, so the controls stay pinned while scrolling long lists.

Header work is **fleet-only** — the driver PWA uses a different `app-bar` pattern and has
no duplicate-title problem.

## Part 1 — Favicon

### Asset

- New `favicon.svg`: the white "O" monogram on the brand blue `#1a56db` (matches the
  driver `manifest.json` `theme_color` and the existing PNGs) as a rounded-square vector.
- Identical file placed in both `static/fleet/favicon.svg` and `static/driver/favicon.svg`.

**Hardcoded `#1a56db` note:** the AGENTS.md "no inline hex" rule governs the CSS token
system (stylesheets). A standalone `.svg` image asset is not a token context — like the
PNGs, it carries the brand blue directly. Match the manifest value exactly.

### Wiring

- `static/fleet/index.html` `<head>`:
  - `<link rel="icon" href="/fleet/favicon.svg" type="image/svg+xml">`
  - `<link rel="apple-touch-icon" href="/fleet/icon-192.png">`
  - Copy `icon-192.png` + `icon-512.png` from `static/driver/` into `static/fleet/` so the
    fleet app is self-contained (no cross-app `/driver/...` references).
- `static/driver/index.html` `<head>`: same two links (driver already has the PNGs).
- **Driver PWA caching — intentionally NOT touched.** `favicon.svg` is *not* added to
  `STATIC_ASSETS` in `static/driver/sw.js`, and `CACHE_NAME` is *not* bumped. Rationale:
  bumping `CACHE_NAME` is a PWA cache-stamp change, which by project rule only `/cut-release`
  performs — feature PRs must not. A favicon loads fine over the network without being in
  the offline precache, so there is no need to precache it. The next `/cut-release` bump
  will fold `favicon.svg` into the cache naturally if desired. This keeps the change inside
  the governance rule and smaller.

## Part 2 — Unified sticky topbar header (fleet)

### Mechanism — a topbar controls slot

1. **`static/fleet/index.html`** — add one slot inside the existing `.topbar__actions`.
   Order is **refresh indicator first, controls second** so the actionable controls anchor
   the far-right edge:

   ```html
   <div class="topbar__actions">
     <span class="refresh-indicator" id="refresh-indicator"></span>
     <div class="topbar__controls" id="topbar-controls"></div>
   </div>
   ```

   Resulting bar: `Trips … [updated 2s ago] [filter ▾] [+ New Trip]`.

2. **`static/fleet/utils/dom.js`** — two helpers next to `setRefreshIndicator`:
   - `clearTopbarControls()` — empties `#topbar-controls`.
   - `setTopbarControls(builderFn)` — clears the slot, then calls `builderFn(slotEl)` so a
     page appends its `<select>`/buttons into the bar.

3. **`static/fleet/app.js` `renderRoute`** — call `clearTopbarControls()` once, right after
   the title is set (~line 130, alongside `setRefreshIndicator('')`). Every navigation
   resets the slot; only list views repopulate it. Detail/form/home views leave it empty —
   no behavior change for them.

4. **`static/fleet/pages/_list.js` `renderEntityList`** — stop emitting
   `.page-header`/`<h1>`. Build the Create button + `extraControls` into the topbar slot via
   `setTopbarControls`. Content becomes just `<div id="list-table"></div>`.

5. **One-off list pages** — `trips.js` (status-filter `<select>` + Create), `loads.js`,
   `events.js`: move their hand-built `.page-header` contents into the topbar slot the same
   way. The filter `change` handlers keep working (they still `navigate(...)` with the new
   filter value; the slot is rebuilt on the resulting re-render).

### CSS (`static/fleet/css/components.css`)

- Add `.topbar__controls`: `display:flex; align-items:center; gap:var(--space-3);
  flex-wrap:nowrap`.
- Compact control sizing inside the bar so they fit the `--space-8` (48px) height:
  `.topbar__controls .form-select, .topbar__controls .btn { height:32px;
  padding:0 var(--space-3); font-size:0.8125rem; }`.
- Bump `.topbar__actions` gap to `--space-4` so the refresh status isn't cramped against
  the filter.
- Tighten **only** the content top padding (not the shorthand): `.content` keeps its
  `--space-5` side/bottom gutters but top padding drops to `--space-3` (24px → 12px) so the
  table sits just under the bar.
- After the refactor, `.page-header` / `.page-title` / `.page-controls` are unused
  (grep-confirmed they appear only in the list pages being changed) — remove them.

### Net result

Title appears once, in the sticky bar; filters + actions sit beside it and stay visible
while scrolling; the ~0.5" dead band disappears.

## Testing

Vitest + happy-dom (existing toolkit; ~190 tests today):

- Update specs asserting on `.page-title`/`.page-header` (in `_list`, `trips`, `loads`,
  `events`) to instead assert controls land in `#topbar-controls` and no `.page-title`
  is emitted.
- Add a test that `renderRoute` / navigation clears `#topbar-controls` between views (a
  list view populates it; a subsequent non-list view empties it).
- Favicon: assert both `index.html` files contain the `rel="icon"` + `apple-touch-icon`
  links. (No `sw.js` assertion — see "Driver PWA caching" above; `sw.js` is unchanged.)

## Out of scope

- Driver PWA header layout (no duplicate-title problem there).
- Topbar responsiveness below desktop widths (fleet SPA is desktop-targeted;
  `flex-wrap:nowrap` + fixed-width select keeps the bar tidy).
- Any icon/logo in the topbar next to the title (considered, dropped).
