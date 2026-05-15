# Design: Document Viewer, Navigation Stack & Home Dashboard

**Date:** 2026-05-15
**Status:** Approved

## Overview

Three linked GitHub issues covering: a generic SPA navigation history stack, an inline document detail/viewer screen in the dispatcher UI, and a home dashboard with KPI tiles. Each issue is independently shippable but the nav stack is a prerequisite for the document viewer.

---

## Issue 1 — Navigation History Stack

### Problem

All "back" buttons in the dispatcher SPA are hardcoded (e.g. `navigate('loads')`). This works when there is only one path into a detail view, but breaks as the app grows — a document detail reachable from multiple places can't know where to send the user back without encoding every possible caller.

### Design

Add a module-level `navHistory` array holding `{ view, params }` entries.

**`navigate(view, params)`** — before rendering the new view, push the current `{ view, params }` state onto `navHistory`. On first call the stack is empty and nothing is pushed.

**`goBack()`** — pop the top entry from `navHistory` and re-render it without pushing (so back never adds to the stack). If the stack is empty, fall back to `navigate('home')`.

All existing hardcoded back buttons are replaced with `goBack()`:
- `back-to-loads` → `goBack()`
- `back-to-trips` → `goBack()`
- `back-to-drivers` → `goBack()`

### Scope boundary

- No browser History API integration (`pushState`/`popstate`) — that is a separate project.
- No persistence across page reloads.

---

## Issue 2 — Document Detail Screen with Inline Viewer

### Problem

Clicking a document row in the dispatcher currently triggers a file download. There is no way to view document metadata or preview content without downloading.

### Design

**New route:** `'document'` added to the `navigate()` switch, calling `renderDocumentDetailView(params)`.

**Entry points:**
- Documents list (`renderDocumentsView`): doc-row click calls `navigate('document', { id: blobId })`
- Load detail (`renderLoadDetailView`): doc-row click calls `navigate('document', { id: blobId })`
- Future entry points: same pattern, no changes needed to the document view itself

**Back button:** calls `goBack()`, falling back to `navigate('home')` on empty stack.

**Data fetch:** `GET /dispatch/api/v1/blob/{id}` with `Accept: application/json` returns the full `BlobRecord` (includes `updated_at` and `error`, which are absent from the list response shape). A separate fetch for the raw bytes is made only for the viewer iframe.

### Metadata card

Uses the existing `detail-card` pattern. Fields shown (all read-only):

| Field | Notes |
|-------|-------|
| Name | |
| Type | `mime_type` displayed as-is |
| Size | Formatted (e.g. "1.2 MB") |
| Status | Badge (pending / processing / ready / failed) |
| Summary | AI-generated summary, omitted if absent |
| Tags | Comma-separated; noted as a future edit target |
| Uploaded | `created_at` formatted date |
| Updated | `updated_at` formatted date |
| Error | Only shown when status is `failed` |

A **"Download document"** button sits in the card header. Uses the same blob fetch + `URL.createObjectURL` + `<a>.click()` pattern as the existing download handler.

### Viewer section

Positioned below the metadata card.

**Fetch:** `GET /dispatch/api/v1/blob/{id}` (raw bytes) → `URL.createObjectURL(blob)` → set as `iframe.src`. Object URL is revoked when the user navigates away from the view.

**Supported types (render in iframe):**
- `application/pdf`
- `image/*` (png, jpeg, gif, webp, svg, etc.)
- `text/plain`
- `text/html`

**Unsupported types:** Display a placeholder block with an icon and the message:
> "This document type can't be previewed — use the Download button above."

No third-party viewers (Google Docs Viewer, Office Online) in scope for this issue.

### Scope boundary

- Tags are read-only. Editing tags is a future issue.
- No upload from this screen.
- No third-party viewer integration.

---

## Issue 3 — Home Dashboard with KPI Tiles

### Problem

The app currently lands on the loads list. There is no overview screen. As the app grows, dispatchers need a quick-glance summary before drilling into a specific area.

### Design

**New default route:** `'home'` becomes the initial view rendered on load (replacing `'loads'`). The sidebar nav adds a "Home" entry at the top with the same `sidebar__link` pattern as existing nav items, using the label "Home".

**`renderHomeView()`** fetches four counts in parallel and renders a row of KPI tiles:

| Tile | Label | Endpoint | Filter |
|------|-------|----------|--------|
| Open loads | "Open Loads" | `GET /dispatch/api/v1/loads/count` | status = open/active |
| Active drivers | "Active Drivers" | `GET /dispatch/api/v1/drivers/count` | status = active |
| Pending documents | "Pending Documents" | `GET /dispatch/api/v1/blobs/count` | status = pending |
| Recent events | "Events Today" | `GET /dispatch/api/v1/events/count` | today |

Each tile is clickable and navigates to the corresponding list view.

**Count endpoint response shape (all four):**
```json
{ "count": 42 }
```

### Backend work (Rust)

Four new lightweight GET endpoints, one per entity. Each queries the DB with a filtered `COUNT(*)` rather than fetching rows. No pagination, no embedding lookups.

Suggested paths:
- `GET /dispatch/api/v1/loads/count?status=open`
- `GET /dispatch/api/v1/drivers/count?status=active`
- `GET /dispatch/api/v1/blobs/count?status=pending`
- `GET /dispatch/api/v1/events/count?since=today`

Exact filter params to be confirmed against the existing DB query layer during implementation.

### KPI tile styling

Uses a new `kpi-tile` CSS component: card with a large number, a label below, subtle hover state indicating it's clickable. Consistent with existing `detail-card` / `data-table` visual language.

### Scope boundary

- Tiles show counts only — no sparklines, no trend data.
- No auto-refresh; dispatcher refreshes manually.
- Dashboard content will be expanded in future sprints.

---

## Issue dependency order

```
Issue 1 (nav stack) → Issue 2 (document viewer)
Issue 3 (home dashboard) — can ship independently, but nav stack should land first
```
