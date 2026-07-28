import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml, shortId, fmtDate, fmtRelative, humanizeEventType } from '../utils/format.js';
import { setContent, setRefreshIndicator, setTopbarControls } from '../utils/dom.js';

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
  const jumpNoun = entityType === 'blob' ? 'document' : entityType;
  const jump = href
    ? `<a class="event-item__jump" data-link href="${escHtml(href)}">Go to ${escHtml(jumpNoun)} →</a>`
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

export function applyAttentionFilter(events, attentionOnly) {
  return attentionOnly ? events.filter(e => e.severity === 'exception') : events;
}

export function attachEventHandlers(root) {
  root.addEventListener('click', (e) => {
    if (e.target.closest('.event-item__jump')) return;
    const item = e.target.closest('.event-item');
    if (!item || !root.contains(item)) return;
    const detail = item.querySelector('.event-item__detail');
    if (detail) detail.hidden = !detail.hidden;
  });
}

const PAGE_SIZE = 20;
// Server clamps ?limit= at 100 (list_events in src/api/fleet_portal/data.rs).
const MAX_WINDOW = 100;

let attentionOnly = false;
let eventsRefreshTimer = null;
// Single contiguous window from offset 0, refetched whole on every refresh —
// new arrivals shift offsets on the append-only table, so pages fetched at
// different times can open a silent gap. "Load more" grows the window instead.
let events = [];
let windowLimit = PAGE_SIZE;
let hasMore = false;

export function clearEventsRefresh() {
  if (eventsRefreshTimer !== null) {
    clearInterval(eventsRefreshTimer);
    eventsRefreshTimer = null;
  }
}

async function fetchEventsWindow() {
  const res = await apiFetch(`${API_BASE}/events?limit=${windowLimit}&offset=0`);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const data = await res.json();
  return data.events || data.items || (Array.isArray(data) ? data : []);
}

function renderEventsList() {
  const visible = applyAttentionFilter(events, attentionOnly);

  const listEl = document.getElementById('events-list');
  if (listEl) {
    listEl.innerHTML = visible.length === 0
      ? '<div class="state-empty" style="min-height:120px;">No events found</div>'
      : eventsListHtml(visible);
  }

  const moreEl = document.getElementById('events-more');
  if (moreEl) {
    if (hasMore) {
      moreEl.innerHTML = '<button class="btn btn--secondary" id="events-load-more">Load more</button>';
      moreEl.querySelector('#events-load-more')?.addEventListener('click', loadOlderEvents);
    } else if (events.length >= MAX_WINDOW) {
      moreEl.innerHTML = `<div style="color:var(--color-text-muted);font-size:var(--text-sm);">Showing the ${MAX_WINDOW} most recent events</div>`;
    } else {
      moreEl.innerHTML = '';
    }
  }
}

async function fetchAndRenderEvents() {
  try {
    events = await fetchEventsWindow();
    hasMore = events.length === windowLimit && windowLimit < MAX_WINDOW;
    setRefreshIndicator(`Updated ${new Date().toLocaleTimeString()}`);
    renderEventsList();
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      const listEl = document.getElementById('events-list');
      if (listEl) {
        listEl.innerHTML = `<div class="state-error" style="min-height:80px;">Failed to load events: ${escHtml(err.message)}</div>`;
      }
      setRefreshIndicator('Error');
    }
  }
}

async function loadOlderEvents() {
  const btn = document.getElementById('events-load-more');
  if (btn) btn.disabled = true;
  windowLimit = Math.min(windowLimit + PAGE_SIZE, MAX_WINDOW);
  await fetchAndRenderEvents();
}

export async function renderEventsView() {
  // Reset filter + paging state on each (re)entry so the controls match the list
  attentionOnly = false;
  events = [];
  windowLimit = PAGE_SIZE;
  hasMore = false;

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
    <div id="events-more" style="text-align:center;margin-top:var(--space-3);"></div>
  `);

  // Attach row-expand handler once on the persistent container
  const listEl = document.getElementById('events-list');
  if (listEl) attachEventHandlers(listEl);

  await fetchAndRenderEvents();

  // Wire the attention-only toggle
  const attnBtn = document.getElementById('events-attention');
  if (attnBtn) {
    attnBtn.addEventListener('click', () => {
      attentionOnly = !attentionOnly;
      attnBtn.setAttribute('aria-pressed', String(attentionOnly));
      attnBtn.classList.toggle('is-active', attentionOnly);
      renderEventsList();
    });
  }

  // Auto-refresh every 30s
  eventsRefreshTimer = setInterval(fetchAndRenderEvents, 30_000);
}
