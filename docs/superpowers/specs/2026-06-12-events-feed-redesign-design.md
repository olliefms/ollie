# Events Feed Redesign — Design

**Date:** 2026-06-12
**Status:** Approved (brainstorm), pending implementation plan

## Problem

The Events page (`static/fleet/pages/events.js`) is the least useful screen in the
system. Each row shows only an entity-type badge, a humanized event name, an
occasionally-present stop suffix, and a timestamp. It never says *which* trip,
driver, or facility an event concerns (the `entity_id` UUID is never resolved),
rows can't be opened for detail, and half the generated event types aren't even
labeled. It reads as noise.

## Goal

Reframe the page as a glanceable **live ops feed** — "what's happening across the
fleet right now" — optimized for recency and at-a-glance scanning, with the
ability to open any event for detail and jump to the entity it concerns.

This is Phase 1. Full filtering/search (audit use case) is explicitly out of scope.

## Decisions (from brainstorm)

- **Primary job:** live ops feed (recency + glanceability), not audit/forensics.
- **Event tiering:** tier visually, show everything. Exceptions highlighted,
  routine lifecycle normal, low-value system events muted — nothing hidden.
- **Subject resolution:** hybrid. Read-time hydration for the subject label
  (works retroactively on all existing events), plus emit-time enrichment only
  for context the reader can't reconstruct (facility name behind a stop `seq`).
- **Click behavior:** inline expand (event's own detail) **plus** a prominent
  jump link to the related entity.
- **Row layout:** dense single-line (option A).
- **Refresh:** keep the existing 30s polling. No SSE/websockets.
- **Filtering in Phase 1:** a single "Needs attention only" (exceptions) toggle.
  Type/entity/date filters deferred.

## Architecture

### Backend (`src/`)

1. **Read-time subject hydration** — `GET /fleet/api/v1/events`
   (handler in `src/data.rs`, query `ListEventsDispatchQuery` ~`data.rs:1417`).
   After loading the page of `EventRecord`s (`src/.../event.rs`), collect
   `entity_id`s grouped by `entity_type` and batch-resolve to a display label,
   attaching a new `subject` field to each item in `EventListResponse`:
   - `trip` → `TRIP-#### · {origin}→{dest}` (trip number + route)
   - `driver` → driver name
   - `truck` / `trailer` → unit number
   - `blob` → filename
   Batched one query per entity type per page (not per row) to avoid N+1.
   Missing/deleted entities degrade gracefully to a short id-based label.

2. **Emit-time facility enrichment** — stop events only.
   At the `stop.arrived` / `stop.departed` / `stop.late` emit sites
   (`src/events/...`, e.g. `events.rs:62/68/74`), resolve `seq → stop → facility`
   and write `facility_name` into the event payload. The reader cannot
   reconstruct this from `{seq}` alone, so it must be captured at emit time.
   Applies to new events only (acceptable — this is point-in-time context).

3. **Severity classification** — derive `severity` ∈ {`exception`, `normal`,
   `system`} from `event_type` and return it per event so the frontend does not
   hardcode the mapping:
   Exception classification wins over system when both could apply.
   - `exception`: `stop.late`, `processing_failed`
   - `system`: `processing_started`, `processing_completed`,
     `driver.equipment_changed`, `driver.trailer_changed`
   - `normal`: everything else (trip lifecycle, stop arrive/depart, check_call,
     *_available)

### Frontend (`static/fleet/`)

4. **Layout A rows** (`pages/events.js`): single line —
   `[badge] {subject} {verb} · {context} {relative-time}`.
   Context comes from the enriched payload (facility name, location, ETA, unit).

5. **Tiering**: exception rows get a red left-accent + tint; system rows muted
   (reduced opacity); normal rows plain. Remove the current client-side
   `BLOB_NOISE_EVENTS` filter — those become muted system rows instead.

6. **Click → inline expand**: clicking a row toggles an inline detail panel
   showing actor, full UTC timestamp, and the humanized payload
   (location / ETA / notes / equipment change / error), plus a
   "Go to {trip|driver|truck|trailer|document} →" link that routes to the
   existing SPA entity view via `entity_type` + `entity_id`.

7. **Humanizer fix** (`utils/format.js`, `humanizeEventType`): add
   `driver.equipment_changed` and `driver.trailer_changed` mappings.

8. **"Needs attention only" toggle**: client-side filter showing only
   `severity === 'exception'` rows.

9. Keep the 30s auto-refresh. Timestamps render relative ("2m", "15m") in the
   row, absolute UTC in the expanded panel.

## Data flow

```
events.js
  → GET /fleet/api/v1/events
      (response now includes `subject` + `severity` per event,
       and stop payloads carry `facility_name`)
  → render tiered single-line rows (A)
  → click row → toggle inline detail panel
      → "Go to …" link routes via entity_type + entity_id to the SPA entity view
```

## Testing

- **Backend (Rust):**
  - Unit test for severity classification across all ~20 event types
    (including the `processing_failed` exception-over-system precedence).
  - Hydration test: `subject` is populated for each entity type, and degrades
    to an id-based label when the referenced entity is missing/deleted.
  - Stop-event emit test: payload carries `facility_name`.
- **Frontend (Vitest + happy-dom, per PR #347 toolkit):**
  - Rows render the enriched subject and correct verb/context.
  - Exception rows receive the accent class; system rows the muted class.
  - Clicking a row toggles the inline detail; second click collapses it.
  - "Go to" jump link targets the correct route for each entity type.
  - "Needs attention only" toggle filters to exceptions.

## Out of scope (later phases)

- Type / entity / date-range filters and search (audit use case).
- Server-side pagination UI / infinite scroll.
- A standalone per-event detail route (the inline expand + entity jump covers it).
- Real-time push (SSE/websockets).
