# Events Feed Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the flat, dead-end Events page into a glanceable live ops feed with enriched subjects, severity tiering, and click-to-expand rows that jump to the related entity.

**Architecture:** Backend adds two derived fields to each event in the `/fleet/api/v1/events` response — `severity` (pure function of `event_type`) and `subject` (a human label resolved by looking up the referenced entity) — plus emit-time `stop_name` enrichment for stop events. Frontend renders dense single-line rows tiered by severity, with an inline expand panel and a `data-link` jump to the entity's SPA detail route.

**Tech Stack:** Rust (axum handlers, LanceDB via `DbClient`), vanilla-JS SPA (`static/fleet/`), Vitest + happy-dom for frontend tests, `cargo test` for backend. Repo uses DCO (`git commit -s`) and clippy; never run `cargo fmt --all`.

---

## File Structure

**Backend**
- `src/models/event.rs` — add `classify_severity()` (pure fn + unit tests); add `severity` + `subject` fields to `EventResponse`; set `severity` in `From`, `subject` to `None`.
- `src/api/fleet_portal/data.rs` — in `list_events`, hydrate `subject` per event via deduped entity lookups (`subject_for`, `trip_subject` helpers added here).
- `src/events/mod.rs` — enrich `stop.arrived/departed/late` payloads with `stop_name` resolved from the trip's stop list.

**Frontend**
- `static/fleet/utils/format.js` — add 2 humanizer entries; add `fmtRelative()`.
- `static/fleet/pages/events.js` — rewrite rendering: layout-A rows, severity tiering, relative time, inline expand + jump link, "Needs attention only" toggle. Extract pure helpers (`eventContext`, `eventRowHtml`, `eventsListHtml`, `jumpHref`) so they're unit-testable.

**Tests**
- `src/models/event.rs` (`#[cfg(test)]`) — severity classification.
- `tests/fleet/format.test.js` — humanizer additions + `fmtRelative`.
- `tests/fleet/events.test.js` (new) — row HTML, tiering, context, jump href, expand toggle, attention filter.

---

## Task 1: Severity classification (backend, pure)

**Files:**
- Modify: `src/models/event.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/models/event.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::classify_severity;

    #[test]
    fn classifies_severity() {
        assert_eq!(classify_severity("stop.late"), "exception");
        assert_eq!(classify_severity("processing_failed"), "exception");
        assert_eq!(classify_severity("processing_started"), "system");
        assert_eq!(classify_severity("processing_completed"), "system");
        assert_eq!(classify_severity("driver.equipment_changed"), "system");
        assert_eq!(classify_severity("driver.trailer_changed"), "system");
        assert_eq!(classify_severity("trip.dispatched"), "normal");
        assert_eq!(classify_severity("stop.arrived"), "normal");
        assert_eq!(classify_severity("check_call"), "normal");
        assert_eq!(classify_severity("anything_else"), "normal");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib event::tests::classifies_severity`
Expected: FAIL — `cannot find function classify_severity`.

- [ ] **Step 3: Add the function**

In `src/models/event.rs`, after the `use` lines (before `EventRecord`), add:

```rust
/// Classify an event's display severity from its type.
/// Exception wins over system when both could apply.
pub fn classify_severity(event_type: &str) -> &'static str {
    match event_type {
        "stop.late" | "processing_failed" => "exception",
        "processing_started" | "processing_completed" | "driver.equipment_changed"
        | "driver.trailer_changed" => "system",
        _ => "normal",
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib event::tests::classifies_severity`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/models/event.rs
git commit -s -m "feat(events): add event severity classification"
```

---

## Task 2: Add `severity` + `subject` fields to `EventResponse`

**Files:**
- Modify: `src/models/event.rs:48-46` (`EventResponse` struct + `From` impl)

- [ ] **Step 1: Add the fields**

Replace the `EventResponse` struct and its `From` impl in `src/models/event.rs` with:

```rust
#[derive(Debug, Serialize, ToSchema)]
pub struct EventResponse {
    pub id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub occurred_at: String,
    /// Display severity: "exception" | "system" | "normal".
    pub severity: String,
    /// Human label for the referenced entity (filled by the handler).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

impl From<EventRecord> for EventResponse {
    fn from(r: EventRecord) -> Self {
        Self {
            id: r.id,
            entity_type: r.entity_type,
            entity_id: r.entity_id,
            event_type: r.event_type.clone(),
            payload: r.payload.as_deref().and_then(|s| serde_json::from_str(s).ok()),
            actor: r.actor,
            occurred_at: r.occurred_at,
            severity: classify_severity(&r.event_type).to_string(),
            subject: None,
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: builds (existing `event::tests` still pass). The `r.event_type.clone()` is required because `classify_severity(&r.event_type)` borrows it after the move into the struct.

- [ ] **Step 3: Commit**

```bash
git add src/models/event.rs
git commit -s -m "feat(events): expose severity + subject on EventResponse"
```

---

## Task 3: Hydrate `subject` in the `list_events` handler

**Files:**
- Modify: `src/api/fleet_portal/data.rs:1442-1465` (`list_events`)

- [ ] **Step 1: Add subject-builder helpers**

In `src/api/fleet_portal/data.rs`, directly above `pub async fn list_events`, add:

```rust
fn trip_subject(t: &crate::models::trip::TripRecord) -> String {
    let route = if t.stops.len() >= 2 {
        let o = t.stops.first().and_then(|s| s.name.as_deref()).unwrap_or("?");
        let d = t.stops.last().and_then(|s| s.name.as_deref()).unwrap_or("?");
        format!(" · {o} → {d}")
    } else {
        String::new()
    };
    format!("Trip {}{}", t.trip_number, route)
}

async fn subject_for(db: &crate::db::DbClient, entity_type: &str, id: Uuid) -> Option<String> {
    match entity_type {
        "trip" => db.get_trip(id).await.ok().map(|t| trip_subject(&t)),
        "driver" => db.get_driver_by_id(id).await.ok().map(|d| d.name),
        "truck" => db.get_truck_by_id(id).await.ok().map(|t| format!("Truck {}", t.unit_number)),
        "trailer" => db.get_trailer_by_id(id).await.ok().map(|t| format!("Trailer {}", t.unit_number)),
        "blob" => db.get_by_id(id).await.ok().map(|b| b.name),
        _ => None,
    }
}
```

> Note: if any of these `TripRecord`/`DbClient` paths differ, use the module path that resolves in this file — `src/models/event.rs` and neighbors already import the model types. Confirm with `cargo build`.

- [ ] **Step 2: Hydrate subjects in the handler**

Replace the final two lines of `list_events` (currently building `items` and returning) with:

```rust
    let mut items: Vec<EventResponse> = records.into_iter().map(EventResponse::from).collect();

    // Resolve subject labels, deduping lookups by (entity_type, entity_id).
    let mut cache: std::collections::HashMap<(String, Uuid), Option<String>> =
        std::collections::HashMap::new();
    for it in items.iter_mut() {
        let key = (it.entity_type.clone(), it.entity_id);
        let label = match cache.get(&key) {
            Some(v) => v.clone(),
            None => {
                let v = subject_for(&state.db, &it.entity_type, it.entity_id).await;
                cache.insert(key, v.clone());
                v
            }
        };
        it.subject = label;
    }

    Ok(Json(EventListResponse { returned: items.len(), items }))
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: builds. If a getter path is wrong, the error names the missing method — fix the path per the codebase and rebuild.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --lib`
Expected: no new warnings on the touched file.

- [ ] **Step 5: Commit**

```bash
git add src/api/fleet_portal/data.rs
git commit -s -m "feat(events): hydrate human subject labels in list_events"
```

---

## Task 4: Enrich stop-event payloads with `stop_name`

**Files:**
- Modify: `src/events/mod.rs:60-76` (`on_stop_arrived`, `on_stop_departed`, `on_stop_late`)

- [ ] **Step 1: Add a stop-name resolver**

In `src/events/mod.rs`, below `now_z()`, add:

```rust
async fn stop_name(db: &DbClient, trip_id: Uuid, seq: u32) -> Option<String> {
    db.get_trip(trip_id)
        .await
        .ok()?
        .stops
        .into_iter()
        .find(|s| s.sequence == seq)
        .and_then(|s| s.name)
}
```

- [ ] **Step 2: Use it in the three stop emitters**

Replace `on_stop_arrived`, `on_stop_departed`, and `on_stop_late` with:

```rust
pub async fn on_stop_arrived(db: &DbClient, trip_id: Uuid, seq: u32) {
    let payload = serde_json::json!({ "seq": seq, "stop_name": stop_name(db, trip_id, seq).await });
    let _ = db.append_event("trip", trip_id, "stop.arrived", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, seq, "stop arrived");
}

pub async fn on_stop_departed(db: &DbClient, trip_id: Uuid, seq: u32) {
    let payload = serde_json::json!({ "seq": seq, "stop_name": stop_name(db, trip_id, seq).await });
    let _ = db.append_event("trip", trip_id, "stop.departed", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, seq, "stop departed");
}

pub async fn on_stop_late(db: &DbClient, trip_id: Uuid, seq: u32, eta: Option<String>, notes: Option<String>) {
    let payload = serde_json::json!({
        "seq": seq, "stop_name": stop_name(db, trip_id, seq).await, "eta": eta, "notes": notes
    });
    let _ = db.append_event("trip", trip_id, "stop.late", Some(payload), None, &now_z(), None).await;
    tracing::info!(trip_id = %trip_id, seq, "stop late");
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: builds.

- [ ] **Step 4: Commit**

```bash
git add src/events/mod.rs
git commit -s -m "feat(events): enrich stop events with stop_name in payload"
```

---

## Task 5: Frontend format helpers — humanizer + relative time

**Files:**
- Modify: `static/fleet/utils/format.js`
- Test: `tests/fleet/format.test.js`

- [ ] **Step 1: Write the failing tests**

Append to `tests/fleet/format.test.js`:

```javascript
import { fmtRelative } from '../../static/fleet/utils/format.js';

describe('humanizeEventType additions', () => {
  it('maps equipment + trailer change events', () => {
    expect(humanizeEventType('driver.equipment_changed')).toBe('Driver Equipment Changed');
    expect(humanizeEventType('driver.trailer_changed')).toBe('Driver Trailer Changed');
  });
});

describe('fmtRelative', () => {
  const now = 1_000_000_000_000;
  it('seconds / minutes / hours / days', () => {
    expect(fmtRelative(new Date(now - 5_000).toISOString(), now)).toBe('5s');
    expect(fmtRelative(new Date(now - 120_000).toISOString(), now)).toBe('2m');
    expect(fmtRelative(new Date(now - 3 * 3600_000).toISOString(), now)).toBe('3h');
    expect(fmtRelative(new Date(now - 2 * 86400_000).toISOString(), now)).toBe('2d');
  });
  it('em-dash for falsy/invalid', () => {
    expect(fmtRelative('', now)).toBe('—');
    expect(fmtRelative('not-a-date', now)).toBe('—');
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run tests/fleet/format.test.js`
Expected: FAIL — `fmtRelative` is not exported; equipment mappings missing.

- [ ] **Step 3: Implement**

In `static/fleet/utils/format.js`, add the two entries to the `humanizeEventType` map (after `'trailer_available'`):

```javascript
    'driver.equipment_changed': 'Driver Equipment Changed',
    'driver.trailer_changed':   'Driver Trailer Changed',
```

Then append a new export:

```javascript
export function fmtRelative(isoStr, nowMs = Date.now()) {
  if (!isoStr) return '—';
  const t = new Date(isoStr).getTime();
  if (Number.isNaN(t)) return '—';
  const s = Math.max(0, Math.floor((nowMs - t) / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}
```

- [ ] **Step 4: Run to verify pass**

Run: `npx vitest run tests/fleet/format.test.js`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add static/fleet/utils/format.js tests/fleet/format.test.js
git commit -s -m "feat(events): humanize equipment events + add fmtRelative"
```

---

## Task 6: Events page rendering — rows, tiering, context, jump href

**Files:**
- Modify: `static/fleet/pages/events.js`
- Test: `tests/fleet/events.test.js` (create)

This task extracts pure helpers and rewrites the row markup to layout A. Behavior (expand/toggle/filter) comes in Task 7.

- [ ] **Step 1: Write the failing test**

Create `tests/fleet/events.test.js`:

```javascript
import { describe, it, expect } from 'vitest';
import { eventContext, jumpHref, eventRowHtml, eventsListHtml } from '../../static/fleet/pages/events.js';

const base = {
  id: 'e1', entity_type: 'trip', entity_id: 't1', event_type: 'stop.arrived',
  occurred_at: new Date().toISOString(), severity: 'normal',
  subject: 'Trip 1042 · Acme → Dallas', payload: { seq: 2, stop_name: "Love's #212" },
};

describe('eventContext', () => {
  it('uses stop_name for stop events', () => {
    expect(eventContext(base.payload, 'stop.arrived')).toBe("Love's #212");
  });
  it('uses location for check_call', () => {
    expect(eventContext({ location: 'I-40 nr Amarillo' }, 'check_call')).toBe('I-40 nr Amarillo');
  });
  it('empty when nothing useful', () => {
    expect(eventContext({}, 'trip.dispatched')).toBe('');
  });
});

describe('jumpHref', () => {
  it('maps entity types to detail routes', () => {
    expect(jumpHref('trip', 't1')).toBe('/fleet/trips/t1');
    expect(jumpHref('driver', 'd1')).toBe('/fleet/drivers/d1');
    expect(jumpHref('truck', 'k1')).toBe('/fleet/trucks/k1');
    expect(jumpHref('trailer', 'r1')).toBe('/fleet/trailers/r1');
    expect(jumpHref('blob', 'b1')).toBe('/fleet/documents/b1');
  });
  it('null for unknown', () => {
    expect(jumpHref('mystery', 'x')).toBe(null);
  });
});

describe('eventRowHtml', () => {
  it('renders subject, humanized verb, and context', () => {
    const html = eventRowHtml(base);
    expect(html).toContain('Trip 1042 · Acme → Dallas');
    expect(html).toContain('Stop Arrived');
    expect(html).toContain("Love's #212");
  });
  it('adds exception class for exception severity', () => {
    expect(eventRowHtml({ ...base, severity: 'exception', event_type: 'stop.late' }))
      .toContain('event-item--exception');
  });
  it('adds system class for system severity', () => {
    expect(eventRowHtml({ ...base, severity: 'system', entity_type: 'blob', event_type: 'processing_failed' }))
      .toContain('event-item--system');
  });
  it('falls back to short id when subject missing', () => {
    expect(eventRowHtml({ ...base, subject: null })).toContain('t1'.slice(0, 8));
  });
});

describe('eventsListHtml', () => {
  it('renders one row per event', () => {
    const html = eventsListHtml([base, { ...base, id: 'e2' }]);
    expect((html.match(/event-item"/g) || []).length).toBe(2);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run tests/fleet/events.test.js`
Expected: FAIL — helpers not exported.

- [ ] **Step 3: Rewrite `events.js` rendering**

Replace the top imports of `static/fleet/pages/events.js` with:

```javascript
import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml, shortId, fmtDate, fmtRelative, humanizeEventType } from '../utils/format.js';
import { setContent, setRefreshIndicator } from '../utils/dom.js';
```

Delete the `BLOB_NOISE_EVENTS` constant. Then add these exported pure helpers near the top (after the imports):

```javascript
const ROUTE_BASE = {
  trip: 'trips', driver: 'drivers', truck: 'trucks', trailer: 'trailers', blob: 'documents',
};

export function jumpHref(entityType, entityId) {
  const base = ROUTE_BASE[entityType];
  return base ? `/fleet/${base}/${entityId}` : null;
}

export function eventContext(payload, eventType) {
  const p = payload || {};
  if (eventType && eventType.startsWith('stop.')) {
    return p.stop_name || p.facility_name || (p.seq != null ? `Stop ${p.seq}` : '');
  }
  if (eventType === 'check_call') return p.location || '';
  return p.stop_name || p.facility_name || p.location || '';
}

function severityClass(severity) {
  if (severity === 'exception') return ' event-item--exception';
  if (severity === 'system') return ' event-item--system';
  return '';
}

function parsePayload(ev) {
  try {
    return typeof ev.payload === 'string' ? JSON.parse(ev.payload) : (ev.payload || {});
  } catch (_) { return {}; }
}

export function eventRowHtml(ev) {
  const entityType = (ev.entity_type || '').toLowerCase().replace(/[^a-z0-9_]/g, '_');
  const entityLabel = entityType.charAt(0).toUpperCase() + entityType.slice(1);
  const badge = entityType ? `<span class="badge badge--${entityType}">${escHtml(entityLabel)}</span> ` : '';
  const subject = ev.subject || shortId(ev.entity_id);
  const payload = parsePayload(ev);
  const ctx = eventContext(payload, ev.event_type);
  const ctxHtml = ctx ? ` <span class="event-item__ctx">· ${escHtml(ctx)}</span>` : '';
  const href = jumpHref(entityType, ev.entity_id);
  const jump = href
    ? `<a class="event-item__jump" data-link href="${href}">Go to ${escHtml(entityType)} →</a>`
    : '';
  const detailRows = [
    ev.actor ? `<dt>Actor</dt><dd>${escHtml(ev.actor)}</dd>` : '',
    `<dt>Time</dt><dd>${escHtml(fmtDate(ev.occurred_at))}</dd>`,
    `<dt>Detail</dt><dd>${escHtml(JSON.stringify(payload))}</dd>`,
  ].join('');

  return `
    <div class="event-item${severityClass(ev.severity)}" data-event-id="${escHtml(ev.id)}">
      <div class="event-item__line">
        ${badge}<span class="event-item__subject">${escHtml(subject)}</span>
        <span class="event-item__type">${escHtml(humanizeEventType(ev.event_type || ''))}</span>${ctxHtml}
        <span class="event-item__time">${escHtml(fmtRelative(ev.occurred_at))}</span>
      </div>
      <div class="event-item__detail" hidden>
        <dl>${detailRows}</dl>
        ${jump}
      </div>
    </div>`;
}

export function eventsListHtml(events) {
  return events.map(eventRowHtml).join('');
}
```

Then update `fetchAndRenderEvents` to use the new helpers: remove the `filtered`/`BLOB_NOISE_EVENTS` line so it sorts `events` directly, and replace the `.map(...)` block that builds `items` with:

```javascript
    const items = eventsListHtml(sorted);
```

(Keep the existing sort, empty-state, and error handling.)

- [ ] **Step 4: Run to verify pass**

Run: `npx vitest run tests/fleet/events.test.js`
Expected: PASS.

- [ ] **Step 5: Add CSS for tiering + layout A**

Append to the fleet stylesheet (find it: `grep -rl "\.event-item" static/fleet`; it is the same file that already styles `.event-item`). Add:

```css
.event-item { border-left: 3px solid transparent; }
.event-item__line { display: flex; align-items: baseline; gap: 8px; white-space: nowrap; }
.event-item__subject { font-weight: 600; }
.event-item__ctx { color: var(--color-text-subtle); overflow: hidden; text-overflow: ellipsis; flex: 1; }
.event-item__time { margin-left: auto; color: var(--color-text-subtle); font-size: 0.75rem; }
.event-item--exception { border-left-color: #e5484d; background: #fff5f5; }
.event-item--system { opacity: 0.55; }
.event-item__detail { margin-top: 8px; padding: 8px 10px; background: var(--color-surface-subtle, #f7f8fa); border-radius: 6px; font-size: 0.75rem; }
.event-item__detail dl { display: grid; grid-template-columns: auto 1fr; gap: 2px 12px; margin: 0 0 6px; }
.event-item__jump { font-weight: 600; }
```

> If the variable names differ, match the existing theme tokens used elsewhere in that stylesheet.

- [ ] **Step 6: Commit**

```bash
git add static/fleet/pages/events.js tests/fleet/events.test.js static/fleet/<stylesheet>
git commit -s -m "feat(events): live-feed rows with subject, tiering, and context"
```

---

## Task 7: Inline expand + "Needs attention only" toggle

**Files:**
- Modify: `static/fleet/pages/events.js`
- Test: `tests/fleet/events.test.js`

- [ ] **Step 1: Write the failing test**

Append to `tests/fleet/events.test.js`:

```javascript
import { attachEventHandlers, applyAttentionFilter } from '../../static/fleet/pages/events.js';

describe('attachEventHandlers (expand)', () => {
  it('toggles the detail panel on row click', () => {
    const root = document.createElement('div');
    root.innerHTML = eventsListHtml([base]);
    document.body.appendChild(root);
    attachEventHandlers(root);

    const detail = root.querySelector('.event-item__detail');
    expect(detail.hidden).toBe(true);
    root.querySelector('.event-item__line').click();
    expect(detail.hidden).toBe(false);
    root.querySelector('.event-item__line').click();
    expect(detail.hidden).toBe(true);
    root.remove();
  });

  it('does not toggle when clicking the jump link', () => {
    const root = document.createElement('div');
    root.innerHTML = eventsListHtml([base]);
    document.body.appendChild(root);
    attachEventHandlers(root);
    const detail = root.querySelector('.event-item__detail');
    const link = root.querySelector('.event-item__jump');
    if (link) link.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    expect(detail.hidden).toBe(true);
    root.remove();
  });
});

describe('applyAttentionFilter', () => {
  it('keeps only exception rows when on', () => {
    const evs = [base, { ...base, id: 'x', severity: 'exception' }];
    expect(applyAttentionFilter(evs, true).map(e => e.id)).toEqual(['x']);
  });
  it('returns all rows when off', () => {
    const evs = [base, { ...base, id: 'x', severity: 'exception' }];
    expect(applyAttentionFilter(evs, false).length).toBe(2);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run tests/fleet/events.test.js`
Expected: FAIL — `attachEventHandlers` / `applyAttentionFilter` not exported.

- [ ] **Step 3: Implement the helpers**

Add to `static/fleet/pages/events.js`:

```javascript
export function applyAttentionFilter(events, attentionOnly) {
  return attentionOnly ? events.filter(e => e.severity === 'exception') : events;
}

export function attachEventHandlers(root) {
  root.addEventListener('click', (e) => {
    if (e.target.closest('.event-item__jump')) return; // let the router handle navigation
    const item = e.target.closest('.event-item');
    if (!item || !root.contains(item)) return;
    const detail = item.querySelector('.event-item__detail');
    if (detail) detail.hidden = !detail.hidden;
  });
}
```

- [ ] **Step 4: Wire the toggle + handlers into the view**

In `renderEventsView`, add an "attention" control to the header and a module-scoped flag. At the top of the module (with the other `let` declarations) add:

```javascript
let attentionOnly = false;
```

In the `setContent(...)` template inside `renderEventsView`, add the toggle button into `.page-header` (after the auto-refresh span):

```javascript
      <button id="events-attention" class="btn btn--ghost" aria-pressed="false">Needs attention only</button>
```

After `await fetchAndRenderEvents();` in `renderEventsView`, wire the button:

```javascript
  const attnBtn = document.getElementById('events-attention');
  if (attnBtn) {
    attnBtn.addEventListener('click', () => {
      attentionOnly = !attentionOnly;
      attnBtn.setAttribute('aria-pressed', String(attentionOnly));
      attnBtn.classList.toggle('is-active', attentionOnly);
      fetchAndRenderEvents();
    });
  }
```

In `fetchAndRenderEvents`, apply the filter and attach handlers. After computing `sorted`, change the render block to:

```javascript
    const visible = applyAttentionFilter(sorted, attentionOnly);
    if (visible.length === 0) {
      // (existing empty-state block, but use `visible` instead of `sorted`)
    }
    const listEl = document.getElementById('events-list');
    if (listEl) {
      listEl.innerHTML = eventsListHtml(visible);
      attachEventHandlers(listEl);
    }
```

(Replace the prior `sorted.length === 0` empty-state check and the `listEl.innerHTML = items` assignment accordingly. `attachEventHandlers` is idempotent per render since `listEl` is freshly replaced each fetch — it is a new listener on the same element, which is acceptable because innerHTML replacement discards old child nodes; the listener lives on `listEl` itself, so guard against double-binding by attaching once.)

> To avoid stacking listeners across the 30s refreshes, attach the handler once. Move `attachEventHandlers(listEl)` out of `fetchAndRenderEvents` and call it a single time right after the initial `setContent` in `renderEventsView`, using the `events-list` element. Click handling is delegated, so it keeps working after each `innerHTML` replacement.

- [ ] **Step 5: Run to verify pass**

Run: `npx vitest run tests/fleet/events.test.js`
Expected: PASS.

- [ ] **Step 6: Full frontend suite + commit**

Run: `npx vitest run tests/fleet/`
Expected: PASS (no regressions in sibling fleet tests).

```bash
git add static/fleet/pages/events.js tests/fleet/events.test.js
git commit -s -m "feat(events): inline expand + needs-attention filter"
```

---

## Task 8: Full verification

- [ ] **Step 1: Backend tests + clippy**

Run: `cargo test --lib`
Expected: PASS.
Run: `cargo clippy --lib`
Expected: no new warnings.

- [ ] **Step 2: Frontend suite**

Run: `npm test`
Expected: all Vitest suites PASS.

- [ ] **Step 3: Manual smoke (optional but recommended)**

Use the `run` skill (or the project's dev server) to load `/fleet/events`, confirm: rows show subjects, a late stop / failed parse shows the red accent, clicking a row expands detail with a working "Go to …" link, and the "Needs attention only" toggle filters to exceptions.

- [ ] **Step 4: Final review**

Confirm every spec requirement maps to a task (it does: severity → T1/2, subject hydration → T3, stop enrichment → T4, humanizer + relative time → T5, layout-A rows + tiering + remove blob filter → T6, expand + jump + attention toggle → T7). No standalone event-detail endpoint, no extra filters, no SSE — all correctly out of scope.

---

## Self-Review Notes

- **Spec coverage:** All Phase-1 requirements are covered (see Task 8 Step 4).
- **Type consistency:** `severity` is a `String`/JS string with values `"exception"|"system"|"normal"` throughout. `subject` is `Option<String>` (backend) → `ev.subject` (frontend, may be absent → `shortId` fallback). `eventContext`, `eventRowHtml`, `eventsListHtml`, `jumpHref`, `attachEventHandlers`, `applyAttentionFilter` names are used identically in implementation and tests.
- **Known soft spots flagged for the implementer:** exact model module paths in Task 3 and the stylesheet filename in Task 6 are codebase-specific — both are called out with a `grep`/`cargo build` confirmation step rather than a guess.
