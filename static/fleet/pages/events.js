import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml, shortId, fmtDate, fmtRelative, humanizeEventType } from '../utils/format.js';
import { setContent, setRefreshIndicator } from '../utils/dom.js';

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

let attentionOnly = false;
let eventsRefreshTimer = null;

export function clearEventsRefresh() {
  if (eventsRefreshTimer !== null) {
    clearInterval(eventsRefreshTimer);
    eventsRefreshTimer = null;
  }
}

async function fetchAndRenderEvents() {
  try {
    const res = await apiFetch(`${API_BASE}/events`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const events = data.events || data.items || (Array.isArray(data) ? data : []);

    // Most recent first, using occurred_at
    const sorted = [...events].sort((a, b) =>
      new Date(b.occurred_at || 0).getTime() - new Date(a.occurred_at || 0).getTime()
    );

    const visible = applyAttentionFilter(sorted, attentionOnly);

    setRefreshIndicator(`Updated ${new Date().toLocaleTimeString()}`);

    const listEl = document.getElementById('events-list');
    if (visible.length === 0) {
      if (listEl) {
        listEl.innerHTML = '<div class="state-empty" style="min-height:120px;">No events found</div>';
      }
      return;
    }

    if (listEl) {
      listEl.innerHTML = eventsListHtml(visible);
    }
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

export async function renderEventsView() {
  // Reset filter state on each (re)entry so the button matches the rendered list
  attentionOnly = false;

  // Initial skeleton so the list element exists before fetch
  setContent(`
    <div class="page-header">
      <h1 class="page-title">Events</h1>
      <span style="font-size: 0.8125rem; color: var(--color-text-subtle);">Auto-refreshes every 30s</span>
      <button id="events-attention" class="btn btn--ghost" aria-pressed="false">Needs attention only</button>
    </div>
    <div class="events-list" id="events-list">
      <div class="state-loading"><div class="spinner"></div></div>
    </div>
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
      fetchAndRenderEvents();
    });
  }

  // Auto-refresh every 30s
  eventsRefreshTimer = setInterval(fetchAndRenderEvents, 30_000);
}
