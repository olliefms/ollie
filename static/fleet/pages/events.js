import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml, fmtDate, humanizeEventType } from '../utils/format.js';
import { setContent, setRefreshIndicator } from '../utils/dom.js';

const BLOB_NOISE_EVENTS = new Set([
  'processing_started', 'processing_completed', 'processing_failed',
]);

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

    const filtered = events.filter(ev => !BLOB_NOISE_EVENTS.has(ev.event_type));

    // Most recent first, using occurred_at
    const sorted = [...filtered].sort((a, b) =>
      new Date(b.occurred_at || 0).getTime() - new Date(a.occurred_at || 0).getTime()
    );

    setRefreshIndicator(`Updated ${new Date().toLocaleTimeString()}`);

    if (sorted.length === 0) {
      const listEl = document.getElementById('events-list');
      if (listEl) {
        listEl.innerHTML = '<div class="state-empty" style="min-height:120px;">No events found</div>';
      }
      return;
    }

    const items = sorted.map(ev => {
      const entityType = (ev.entity_type || '').toLowerCase().replace(/[^a-z0-9_]/g, '_');
      const entityLabel = entityType.charAt(0).toUpperCase() + entityType.slice(1);

      let payload = {};
      try {
        payload = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : (ev.payload || {});
      } catch (_) {}
      const stopName = payload.facility_name || payload.stop_name ||
        (payload.sequence != null ? `Stop ${payload.sequence}` : null);
      const stopSuffix = stopName ? ` · ${escHtml(stopName)}` : '';

      const badgeHtml = entityType
        ? `<span class="badge badge--${entityType}">${escHtml(entityLabel)}</span> `
        : '';

      return `
      <div class="event-item">
        ${badgeHtml}<span class="event-item__type">${escHtml(humanizeEventType(ev.event_type || ''))}</span>${stopSuffix}
        <span class="event-item__time">${fmtDate(ev.occurred_at)}</span>
      </div>
    `;
    }).join('');

    const listEl = document.getElementById('events-list');
    if (listEl) {
      listEl.innerHTML = items;
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
  // Initial skeleton so the list element exists before fetch
  setContent(`
    <div class="page-header">
      <h1 class="page-title">Events</h1>
      <span style="font-size: 0.8125rem; color: var(--color-text-subtle);">Auto-refreshes every 30s</span>
    </div>
    <div class="events-list" id="events-list">
      <div class="state-loading"><div class="spinner"></div></div>
    </div>
  `);

  await fetchAndRenderEvents();

  // Auto-refresh every 30s
  eventsRefreshTimer = setInterval(fetchAndRenderEvents, 30_000);
}
