# Driver UI Overhaul — Design

**Date:** 2026-05-17
**Author:** Jim + Claude (brainstorming session)
**Status:** Draft — awaiting plan
**Closes / folds in:** #140 (part 1), #144, #179

## Summary

Restructure the driver PWA around a three-tab bottom nav (`Trips | Pay | Account`), replace the Past tab's pagination with a pay-period weekly stepper, polish the login screen, and tighten navigation hygiene. While the driver-portal code is open, also fold in three open issues that touch the same files: stop arrival/departure entry (#140 part 1, deferred from v1.9), trip document uploads (#144), and the XSS escaping audit (#179).

A future Pay tab is reserved as a placeholder; its actual content is out of scope and tracked separately (depends on #132 driver-pay computation).

## Goals

- Past trips browsable by Sunday-Saturday pay-period weeks with chevrons, a date picker, and horizontal swipe — no more "Load more" with hidden cards.
- A persistent bottom nav establishes Trips, Pay (placeholder), and Account as first-class destinations.
- Refresh on any URL lands on that exact view. Browser back goes to the previous screen, including the right tab and the right week.
- Drivers can enter actual arrival and departure times on stops.
- Drivers can upload trip documents (BOL, POD, scale tickets) from camera or file system, with visibility-aware access so they never see rate cons or other dispatch-private docs.
- Login screen reads as designed, not pinned to the viewport top.
- Driver-app JS modules stay small and focused; common views become reusable components.

## Non-goals

- Pay tab functionality (driver pay computation). Tracked under #132; this work ships a "Coming soon" placeholder.
- Per-terminal data modeling. Tracked as a follow-up to this design (see Follow-ups). Interim uses a single `TERMINAL_TIMEZONE` env var.
- Geofence-based auto-arrival. Out of scope; drivers tap explicitly.
- Dark mode. DESIGN.md says light is canonical.
- Re-architecting login.js around DOM construction (innerHTML template stays; no interpolation of user input).
- AI-assisted doctype detection at upload time.
- Switching driver-app routing to the hash-based scheme used by the dispatch app.

## Section 1 — Information architecture and navigation

### Bottom nav (new persistent component)

Driver navigation per DESIGN.md (3-4 tabs, mobile bottom tab bar). Height `64px + env(safe-area-inset-bottom)`. `--color-surface` background, 1px top `--color-border`, no shadow. Three items:

| Lucide icon | Label | Route |
|---|---|---|
| `truck` | Trips | `/driver/trips` |
| `dollar-sign` | Pay | `/driver/pay` |
| `user` | Account | `/driver/account` |

- Icons inlined as SVG, 24px, stroke 2, color via `currentColor`. No emoji. No filled variants (DESIGN.md icon rule).
- Active item: icon + label colored `--color-primary`, label weight 600. Inactive: `--color-text-muted`, weight 500.
- Hidden only on `/driver` (login). Visible on all authenticated pages including detail screens.
- Implemented as `renderBottomNav(activeItem)` in `static/driver/components/bottom-nav.js`. Each authenticated page appends it as the last step of its render function.

### Top app bar

Driver app bars are 56px per DESIGN.md. Today's `.trips-header` is ~24px+24px padding (taller than spec) — bring to spec while in there. Title left-aligned. Settings gear icon removed from the trips header (Settings is reached via Account tab).

### Routes

| Route | Notes |
|---|---|
| `/driver` | Login (unauthenticated). |
| `/driver/trips` | Trips screen — existing tab bar (Past, Current, Upcoming). |
| `/driver/trips/:id` | Trip detail. |
| `/driver/trips/:id/stops/:seq` | Stop detail. |
| `/driver/pay` | **New.** Placeholder: app bar titled "Pay", empty state "Pay periods coming soon." Bottom nav present. |
| `/driver/account` | **New.** Replaces `/driver/settings`. Profile (name, phone, status) on top; existing settings content (logout, version) below. |
| `/driver/settings` | Redirects (replaceState) to `/driver/account` for one release; removed in the next. |

### Layout

`.trips-page` (and pay/account pages) reserve bottom padding equal to the nav height plus safe-area inset so the scrollable region ends above the nav, not under it.

## Section 2 — Login screen polish

### Current behavior

`.login-screen` uses `padding: clamp(48px, 15vh, 96px) ...` so content stacks from the top. On any viewport with a short form, lots of empty space hangs below — reads as "abandoned at top."

### Changes

- `.login-screen` becomes `display: flex; flex-direction: column; justify-content: center; align-items: center; min-height: 100dvh` with `padding: var(--space-6) var(--space-4)` plus safe-area insets.
- Add `<img src="/driver/icon-192.png">` displayed at 64px above the existing header, with `border-radius: var(--radius-lg)` (no background, the icon is opaque). 16px gap below it.
- Title → subtitle → card spacing unchanged.
- Stale comment in `static/driver/index.html` claiming icons aren't generated → removed.
- No copy changes.

## Section 3 — Past tab: week stepper

Replace the existing 5-at-a-time + "Load more" rendering with a weekly stepper. Current and Upcoming tabs unchanged.

### Anatomy

```
┌────────────────────────────────────────────┐
│  Past  │ Current │ Upcoming                │  segmented control (existing)
├────────────────────────────────────────────┤
│  ◀   Apr 27 – May 3, 2026   ▶              │  week stepper (new)
├────────────────────────────────────────────┤
│  [trip card]                                │
│  [trip card]                                │  scrollable list
│  ...                                        │
└────────────────────────────────────────────┘
```

### Week stepper component

- Single row: `[◀ ghost button] [date-label button] [▶ ghost button]`.
- Surface `--color-surface`, 1px bottom `--color-border`, 56px tall, sticky to the top of the scrollable region.
- Chevrons: Lucide `chevron-left` / `chevron-right`, 24px, stroke 2, ghost buttons, 44×44 hit target.
- Center label: text button (ghost), `body` weight 600. Tap opens a native `<input type="date">` anchored to that button; picking any date snaps to that date's Sun-Sat week.
- Chevron disabled state at boundaries: `--color-text-disabled`, non-interactive. Driven by `has_prev` / `has_next` from the response.

### Week label

`Apr 27 – May 3, 2026`. When the displayed week is the current week (today, in terminal tz, falls within it): label reads `This week`. The prior week: `Last week`. All others: date range.

### Horizontal swipe

- Touch gesture on `.trip-list` area only (not the stepper).
- `pointerdown` + `pointermove`: track horizontal delta; if `|Δx| > 60px` and `|Δx| > |Δy| * 1.5`, trigger prev/next on `pointerup`.
- Block swipe direction when at boundary (matches disabled chevron).
- Optional 150ms slide-in on commit (DESIGN.md quick timing); no swipe-with-progress animation.

### Trip card in Past mode

Unchanged visually except the card content replaces the scheduled-date row with `Delivered Wed, May 1 · 3:42 PM` formatted with `formatShortTime(actual_depart, timezone)` against the *trip's* tz.

### Empty week

Centered message: "No trips delivered this week." Chevrons and date picker remain functional.

### State and URL

- Past tab URL: `/driver/trips?tab=past&week_start=YYYY-MM-DD` where `week_start` is the Sunday-of-week in **terminal-tz** (see Section 4). Same parameter name client URL and API call — no translation layer.
- Tab changes and week changes both use `history.replaceState` (sub-state of the Trips page). Drill-down (trip detail) uses `pushState`.

## Section 4 — Backend API changes

### Trips list endpoint

Same handler and route, extended for Past.

`GET /driver/api/v1/trips?tab=past&week_start=YYYY-MM-DD`

- `week_start` optional. ISO date (Sunday). Interpreted in terminal tz. Defaults to "Sunday on or before today, in terminal tz."
- For `tab=current` / `tab=upcoming`, `week_start` is ignored and the response shape is unchanged.

### Terminal timezone configuration

- New `Config.terminal_timezone: String` (IANA name), sourced from env var `TERMINAL_TIMEZONE`. Default `America/New_York`. Validated at startup by parsing into `chrono_tz::Tz` — invalid value returns a startup error, not a first-request panic.

### Week-bucket rule

A trip belongs to a week when its delivery completion timestamp falls within `[Sunday 00:00 terminal-tz, +7d)` evaluated in the terminal tz.

- "Delivery completion timestamp" = `parse_stop_time(last_stop.actual_depart, last_stop.timezone)` → UTC instant. Existing helper in `src/models/load.rs`.
- Fallback when last-stop `actual_depart` is absent (manual data, legacy trips): use `scheduled_start` parsed with the first stop's tz.
- Cancelled trips: bucketed by `updated_at` (already UTC `DateTime`).
- Naive-string rules from AGENTS.md apply — `actual_depart` with a `Z` or offset is treated as a data error and logged; the trip falls into the scheduled_start fallback.

### Response shape (Past only)

```json
{
  "items": [ /* DriverTripListItem[] with the new delivered_at / delivered_tz fields */ ],
  "week": {
    "start": "2026-04-26",
    "end":   "2026-05-02",
    "has_prev": true,
    "has_next": false,
    "earliest_week_start": "2024-11-03",
    "latest_week_start":   "2026-04-26"
  }
}
```

- `has_prev` / `has_next`: do any earlier/later weeks have trips? Drives chevron disabled state.
- `earliest_week_start` / `latest_week_start`: bounds the date picker.
- The `week` field is omitted for `tab=current` / `tab=upcoming`.

### New fields on `DriverTripListItem`

- `delivered_at: Option<String>` — naive datetime (last stop's `actual_depart`).
- `delivered_tz: Option<String>` — last stop's IANA tz, copied through.

### Implementation sketch (Rust)

```rust
let terminal_tz: chrono_tz::Tz = state.config.terminal_timezone.parse()?;
let week_start_local = parse_week_start_param(params.week_start, &terminal_tz)?;
let week_lo = terminal_tz.from_local_datetime(&week_start_local.and_hms_opt(0,0,0).unwrap()).single()
    .ok_or(...)?.with_timezone(&Utc);
let week_hi = week_lo + chrono::Duration::days(7);

for trip in past_trips {
    let last = trip.stops.last();
    let delivered_utc = last.and_then(|s| {
        parse_stop_time(s.actual_depart.as_deref()?, s.timezone.as_deref())
    });
    let anchor = delivered_utc
        .or_else(|| trip.stops.first().and_then(|s| parse_stop_time(s.scheduled_arrive.as_deref()?, s.timezone.as_deref())))
        .or_else(|| if matches!(trip.status, TripStatus::Cancelled) { Some(trip.updated_at) } else { None });

    if let Some(at) = anchor {
        if at >= week_lo && at < week_hi { /* include */ }
    }
}
```

The handler keeps fetching all driver trips then in-memory filtering (existing pattern). `has_prev` / `has_next` / earliest / latest are derived in a single pass over the unfiltered list.

## Section 5 — Code refactor

`static/driver/pages/trips.js` is 228 lines doing too many things and will balloon with the new content. Split now while we're in the file.

### New file layout

```
static/driver/
  components/
    bottom-nav.js     renderBottomNav(activeItem) -> DOM node
    app-bar.js        renderAppBar({ title, right? }) -> DOM node
    trip-card.js      renderTripCard(trip, mode) where mode in {'past','current','upcoming'}
    week-stepper.js   renderWeekStepper({ week, onChange }) -> DOM node
    swipe.js          attachHorizontalSwipe(el, { onPrev, onNext, canPrev, canNext })
  pages/
    trips.js          orchestrates app-bar + tab bar + per-tab pane
    trips-past.js     past pane: week stepper + list; owns week-state and fetching
    trips-current.js  current pane
    trips-upcoming.js upcoming pane
    pay.js            placeholder
    account.js        replaces settings.js
  utils/
    week.js           sundayOf(date, tz), formatWeekRange(start), formatDeliveredAt(iso)
    time.js           nowInZone(tz), convertNaive(value, fromTz, toTz)
    format.js         unchanged
    api.js            unchanged
    auth.js           unchanged
```

### Boundaries

- `bottom-nav.js` is presentational; uses `navigate()` for routing, knows nothing about page state.
- `app-bar.js` produces a consistent 56px top bar with optional right-side action.
- `trip-card.js` is mode-aware (one source of truth for the card markup).
- `week-stepper.js` is pure; it does not fetch trips. The Past pane owns fetching.
- `swipe.js` is reusable.
- `utils/week.js` is the only place that knows Sun-Sat math and delivered-at formatting.

Files not refactored this round: `login.js`, `trip-detail.js`, `stop-detail.js`. Each gets the bottom nav added but otherwise stays put.

`/driver/settings` redirect handled in `app.js` with `replaceNavigate('/driver/account')` for one release.

CSS version stamps in `index.html` bump per existing pattern; stale icon comment removed.

## Section 6 — Navigation hygiene

The driver app already uses pushState with a server SPA fallback (`api/mod.rs:548` serves `index.html` for unmatched `/driver/*` paths), so refresh works in principle. Rough edges remain.

### History op rules

| Action | History op | Why |
|---|---|---|
| Initial route into `/driver/trips` from `/driver` | replaceState | Don't trap user in a back-to-login loop. |
| Switch bottom-nav tab (Trips ↔ Pay ↔ Account) | replaceState | Sibling tabs are peers, not parent-child. |
| Switch Past/Current/Upcoming sub-tab | replaceState | Sub-state of Trips (already today). |
| Step the week stepper (`?week=...`) | replaceState | Otherwise dozens of history entries pile up. |
| Open trip detail / stop detail | pushState | Drill-down; back should return. |
| Logout from Account | replaceState to `/driver` | Don't let back re-enter authenticated app. |

### In-app back buttons

Today's trip-detail and stop-detail back buttons call `navigate(path)` — which is pushState. That breaks return-to-the-correct-tab and grows the history stack.

Both back handlers change to:

```js
if (history.length > 1) history.back();
else navigate(fallbackPath);
```

`fallbackPath` is `/driver/trips` for trip-detail and `/driver/trips/${tripId}` for stop-detail. Deep-link-safe.

### Service worker

Bump the SW version constant in `static/driver/sw.js` as part of this work so installed PWAs pick up the new shell.

## Section 7 — Stop arrival/departure entry (closes #140 part 1)

### Backend

New route: `PATCH /driver/api/v1/trips/:id/stops/:seq`

- Body: `{ actual_arrive?: string, actual_depart?: string }`, both optional, both **naive local datetimes** (`YYYY-MM-DDTHH:MM:SS`, no Z, no offset).
- Validation uses `validate_stop_time_str(value, stop.timezone, field)` from `src/models/load.rs`. Rejects bad formats, DST-ambiguous times, any string with `Z` or offset.
- If stop has `timezone: None` (legacy): accept the value, interpret as UTC (matches `parse_stop_time` fallback). Don't gate the feature on backfilling tz.
- Cross-field rules: `actual_depart` requires an `actual_arrive` (either in the same payload or already on the stop); `actual_depart >= actual_arrive` compared as parsed UTC instants (correct across DST).
- Future-clock-skew guard (24h) against `Utc::now()`.
- Auth: driver JWT; driver must be assigned to the trip (404 otherwise).
- Side effect: if `actual_depart` is set on the **last stop** of an `InTransit` trip, auto-transition trip status to `Delivered`. No transition otherwise.
- Response: updated `DriverStopDetailResponse`.

### Frontend (stop-detail.js)

Replace the read-only Actual section with action-driven UI:

| State | UI |
|---|---|
| No arrival yet | Primary button: **Arrive now** |
| Arrived, not departed | `Arrived: 3:42 PM` (with ✎ edit) + primary button **Depart now** |
| Both set | `Arrived: 3:42 PM` ✎ &nbsp; `Departed: 5:18 PM` ✎ |

- **Arrive now / Depart now:** PATCH with the current time formatted via `nowInZone(stop.timezone)` from `utils/time.js`. Optimistic update; revert on failure with inline error.
- **Edit (✎):** opens `<input type="datetime-local">` prefilled with the value converted from the stop's tz to the device's local tz (so the picker reads naturally). On submit: convert back to the stop's tz before PATCH.
- Disable button during in-flight request.
- No success toast; the UI updating to show the timestamp is the success signal.

### Client time helpers

`utils/time.js`:

```js
export function nowInZone(tz) {
  if (!tz) return new Date().toISOString().slice(0, 19);
  const s = new Date().toLocaleString('sv-SE', { timeZone: tz, hour12: false });
  return s.replace(' ', 'T');
}

export function convertNaive(value, fromTz, toTz) { /* ... */ }
```

## Section 8 — Trip document uploads (closes #144)

### Visibility model (covers rate-con leak prevention)

Add `visibility: BlobVisibility` to `BlobRecord`. Two values:

| Value | Meaning |
|---|---|
| `private` (default) | Internal-only. Invisible to driver-portal endpoints. |
| `driver` | Visible to the driver currently assigned to the trip the blob is tagged for. |

Typed field, not a tag. Migration adds the field with default `Private` for all existing blobs. Safe default — drivers see nothing they didn't see before.

**Defaults by upload origin:**
- Dispatcher upload: `private`. Dispatcher form gets `☐ Visible to assigned driver` checkbox, default off.
- Driver upload: `driver` (server-set, not client-controllable).
- Admin upload: `private`; can flip via `?visibility=driver` query param or existing PATCH endpoint.

### Ownership tracking

Add `uploaded_by: Option<Uuid>` to `BlobRecord`. Populated to `claims.driver_id` on driver uploads. Used to gate delete permission and as belt-and-suspenders for visibility (driver always sees their own uploads even if visibility flipped to `private` later).

### Tag conventions

| Tag | Purpose |
|---|---|
| `trip:<uuid>` | Associates the blob with a trip. |
| `doctype:bol` / `doctype:pod` / `doctype:scale_ticket` / `doctype:other` | Document type. |
| `source:driver` | Disambiguates driver-side uploads. |

### New driver-portal routes

All under `/driver/api/v1`. Driver JWT required; each handler also validates the trip belongs to the driver (404 otherwise).

| Method | Path | Body / Response |
|---|---|---|
| `POST` | `/trips/:id/documents` | multipart: `file` (required), `doctype` (string). Server tags `trip:<id>`, `doctype:<value>`, `source:driver`; sets `uploaded_by = claims.driver_id`, `visibility = driver`. 202 with `BlobRecord`. |
| `GET` | `/trips/:id/documents` | Filter: `tags contains "trip:<id>" AND (visibility = 'driver' OR uploaded_by = claims.driver_id)`. Returns `BlobListItem[]`. |
| `GET` | `/trips/:id/documents/:blob_id/content` | Streams blob bytes with `Content-Disposition: inline`. |
| `DELETE` | `/trips/:id/documents/:blob_id` | Allowed only when `uploaded_by == claims.driver_id` AND tags include `trip:<id>`. Otherwise 403. |

Server enforces 50MB per file (matches existing dispatcher limit).

### Frontend — Documents card on trip-detail

```
┌────────────────────────────────────────┐
│  Documents                  [+ Upload] │
├────────────────────────────────────────┤
│  📄 BOL — bol-1234.pdf      May 1 3:42 │
│  📸 POD — pod-photo.jpg     May 1 5:18 │
└────────────────────────────────────────┘
```

- `+ Upload` opens a doctype prompt sheet first (BOL / POD / Scale Ticket / Other; skip = `other`). After selecting, fires a single `<input type="file" accept="image/*,application/pdf" capture="environment">`. The OS's native picker handles the Camera / Photo Library / Files choice — no in-app sheet needed for that.
- List: sorted newest-first. Doctype label + filename + upload timestamp.
- Tap row → full-screen overlay (`--shadow-popover` modal pattern) with `<iframe sandbox="" src="/driver/api/v1/trips/:id/documents/:blob_id/content">`. Same sandboxed-iframe pattern dispatch adopted (commit `a076a39`).
- Delete: trailing kebab (⋯) shown only on rows where `blob.uploaded_by === currentDriver.id`. Tap → "Delete" with confirmation dialog. Rows from other uploaders show no kebab (cannot delete).
- Per-row upload spinner during inflight upload; inline error on 4xx with "Try again."
- AI extract pipeline (`BlobStatus::Pending → Ready`) not surfaced.

## Section 9 — XSS escaping audit (closes #179)

### Audit method

1. `rg -n 'innerHTML\s*=|insertAdjacentHTML' static/driver/ static/dispatch/`.
2. Classify each hit:
   - Static template, no interpolation → safe; no action.
   - Interpolates API-derived field → fix with `escHtml(value)` or rewrite to DOM construction (cheaper option locally).
3. Close #179's three named call sites in the dispatch app: `driver.name`, `trip.driver_name`, `stops[i].name`.
4. Audit adjacent fields on the same object even if not named in #179.

### `escHtml` helper

Reuse if it already exists in dispatch helpers. Otherwise add to a shared util:

```js
export function escHtml(s) {
  if (s == null) return '';
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}
```

### New code convention

All new files in this design (bottom-nav.js, trip-card.js, week-stepper.js, documents card, stop-detail editor, week-stepper, etc.) use DOM construction with `.textContent` — no `innerHTML`. Documented in AGENTS.md as a load-bearing convention.

### Out of scope

Rewriting dispatch list views from innerHTML to DOM construction. Targeted escaping only.

## Section 10 — Testing approach

### Rust (backend)

| Area | Test type | Locks |
|---|---|---|
| Week-bucket math in terminal tz | Unit in `data.rs` test module | Phoenix 23:00 Sat-pacific bucketed in *next* Eastern week. DST boundaries don't double-count or skip. `actual_depart=None` → falls back to `scheduled_start`. Cancelled bucketed by `updated_at`. |
| `GET /trips?tab=past&week_start=...` | Integration | Empty week: empty items, valid `has_prev` / `has_next`. `earliest_week_start` / `latest_week_start` reflect history. Cross-driver isolation. |
| `PATCH /trips/:id/stops/:seq` | Integration | Naive accepted; `Z`-suffixed rejected. `depart` without `arrive` → 422. `depart < arrive` → 422. Future > 24h → 422. Last-stop depart on `InTransit` → `Delivered`. |
| Doc visibility filter | Integration | Dispatcher `private` doc: invisible to driver. Same doc set `driver`: visible. Driver-uploaded doc still visible after visibility flipped to `private` (via `uploaded_by`). |
| Doc auth | Integration | Unrelated driver hits another trip's doc endpoint → 404. Delete on doc you didn't upload → 403. |
| Terminal-tz config | Unit | Invalid IANA name → startup error, not first-request panic. |

### JavaScript (driver UI)

No JS test runner today. Add `node --test` files for pure logic; manual smoke for DOM-heavy work.

| Area | Test |
|---|---|
| `utils/week.js` (`sundayOf`, `formatWeekRange`) | `tests/driver/week.test.js`, `node --test` |
| `utils/time.js` (`nowInZone`, `convertNaive`) | Same harness |

### Manual smoke checklist

- [ ] Refresh on each of `/driver/trips`, `/driver/trips?tab=past`, `/driver/trips?tab=past&week_start=YYYY-MM-DD`, `/driver/trips/:id`, `/driver/trips/:id/stops/:seq`, `/driver/pay`, `/driver/account` lands on the named view.
- [ ] Browser back from trip detail returns to the same tab (and same week if Past) the user came from. Tested on Past + Current + Upcoming.
- [ ] Browser back from stop detail returns to trip detail. No history stack growth after multiple round trips.
- [ ] Logout from Account lands on `/driver`. Back cannot re-enter app.
- [ ] Week stepper: chevron ← from current week goes to last week. Disabled at boundary. Tap label → date picker → land on Sun-Sat week.
- [ ] Horizontal swipe on trip list changes week; doesn't fire on the stepper itself.
- [ ] DST boundary week of `2026-03-08`: label reads `Mar 8 – 14, 2026`; list contents sane.
- [ ] Stop arrival entry: tap "Arrive now" → time appears, "Depart now" shows. Tap ✎ → picker prefilled. Submit invalid (`depart < arrive`) → inline 422 message.
- [ ] Last-stop departure on `in_transit` trip → trip moves from Current to Past after refresh.
- [ ] Doc upload from rear camera on iOS and Android. Photo + PDF. Inline preview opens. Delete works on own uploads only.
- [ ] Doc visibility: dispatch `private` not visible to driver; flip to `driver` makes it visible.
- [ ] Bottom nav: Trips / Pay / Account highlight correctly. Pay shows "Coming soon."
- [ ] Login: vertically centered on phone viewport. Icon visible. Same on wide tablet/desktop viewport.
- [ ] Service-worker cache: install PWA, ship new version, reload — old SW replaced; no stale `index.html`.

## Follow-ups (file as separate issues)

- **`feat(api): terminals table — replace global TERMINAL_TIMEZONE env with per-terminal data`** — multi-terminal companies need per-terminal timezones and yard addresses. Proposed: `terminals` table with `id`, `name`, `address`, `timezone`, `is_default`; `drivers.terminal_id` FK; migrate by seeding one default terminal from the env var and backfilling drivers; drop the env var once required.
- **Pay tab content (depends on #132)** — driver-pay computation block on trips is foundational. Larger design conversation that bundles rate-per-mile sourcing, extra-stop fees, detention, and week-roll-up summaries.
- **Edit blob visibility via dispatcher UI** — letting dispatch re-classify an existing blob from `private` to `driver` after upload. Not blocking; existing PATCH endpoint already supports it via API.

## File-level impact summary

**Backend (Rust):**
- `src/config.rs` — add `terminal_timezone` field + env binding.
- `src/models/blob.rs` — add `visibility: BlobVisibility`, `uploaded_by: Option<Uuid>`.
- `src/db/blob_ops.rs` — schema migration, query updates.
- `src/api/blobs.rs`, `src/api/dispatcher_portal/blobs.rs` — visibility query param + form field.
- `src/api/driver_portal/mod.rs` — add new routes.
- `src/api/driver_portal/data.rs` — Past tab week filtering, response shape, `delivered_at` / `delivered_tz` fields. New `PATCH /trips/:id/stops/:seq` handler.
- New: `src/api/driver_portal/documents.rs` — doc upload / list / view / delete handlers.

**Frontend (Driver):**
- New `static/driver/components/` directory: `bottom-nav.js`, `app-bar.js`, `trip-card.js`, `week-stepper.js`, `swipe.js`.
- New `static/driver/pages/`: `pay.js`, `account.js`, `trips-past.js`, `trips-current.js`, `trips-upcoming.js`.
- Refactor `static/driver/pages/trips.js` as orchestrator.
- Update `static/driver/pages/login.js` — vertical centering + icon (CSS-driven; minimal JS change).
- Update `static/driver/pages/trip-detail.js` — Documents card, back-button fix.
- Update `static/driver/pages/stop-detail.js` — arrival/departure editor, back-button fix.
- Remove `static/driver/pages/settings.js` (folded into `account.js`).
- New `static/driver/utils/week.js`, `static/driver/utils/time.js`.
- Update `static/driver/app.js` — `/pay`, `/account`, `/settings` redirect.
- Update `static/driver/css/components.css` — bottom nav, week stepper, doc card, arrival editor, login centering.
- Update `static/driver/index.html` — bump CSS / JS version stamps; remove stale icon comment.
- Bump SW version in `static/driver/sw.js`.

**Dispatch:**
- Upload form — `Visible to assigned driver` checkbox.
- XSS audit fixes per #179 (`driver.name`, `trip.driver_name`, `stops[i].name`, plus adjacent fields).

**Docs:**
- AGENTS.md — note the new "no innerHTML in driver-app new code" convention.

## Acceptance criteria

- [ ] Bottom nav present on all authenticated driver pages with active state correctly reflecting current route. Hidden on `/driver` login.
- [ ] `/driver/pay` renders a placeholder with the bottom nav.
- [ ] `/driver/account` renders profile (name, phone, status) + existing settings content. `/driver/settings` redirects.
- [ ] Login screen is vertically centered with the PWA icon shown at 64px above the title.
- [ ] Past tab shows the current Sun-Sat week by default, scoped in terminal tz. Chevrons step weeks, disabled at history boundaries. Date picker label opens native `<input type="date">`. Horizontal swipe on the list area changes weeks. URL carries `?week_start=YYYY-MM-DD`.
- [ ] Past trip card shows `Delivered <day> · <time>` formatted in the trip's local tz.
- [ ] Refreshing on any driver URL lands on that exact view, including Past tab with week.
- [ ] In-app back buttons on trip-detail and stop-detail use `history.back()` (with deep-link-safe fallback). Repeated trip→stop→trip→stop navigation does not grow history.
- [ ] Bottom-nav switches and Past-tab sub-state changes do not push history entries.
- [ ] Driver can record stop arrival and departure via Arrive now / Depart now buttons; can edit via datetime-local picker. Server validates naive-tz format, ordering, future-skew.
- [ ] Last-stop departure on an in-transit trip transitions the trip to Delivered server-side. The trip then appears in the correct Past week (terminal tz).
- [ ] Driver can upload trip documents from camera / photo library / files. Documents card on trip-detail lists them, opens inline preview, allows delete only on own uploads.
- [ ] `private`-visibility blobs never appear in driver `GET /trips/:id/documents`. `driver`-visibility blobs do. Driver always sees their own uploads regardless of visibility.
- [ ] XSS audit completed: every `innerHTML =` and `insertAdjacentHTML` site has either no interpolation or `escHtml()` wrap. Convention noted in AGENTS.md.
- [ ] All new Rust tests pass. Manual smoke checklist completed before PR review.
