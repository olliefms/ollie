import { apiFetch, API_BASE, hasScope } from '../utils/api.js';
import { escHtml, badge, shortId, fmtArrivalWindow } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';

export async function renderTripsView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const qs = params.status ? `?status=${encodeURIComponent(params.status)}` : '';
    const res = await apiFetch(`${API_BASE}/trips${qs}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const trips = data.items || data.trips || (Array.isArray(data) ? data : []);

    const statusOptions = ['', 'planned', 'assigned', 'dispatched', 'in_transit', 'delivered', 'completed', 'cancelled'];
    const filterStatus = params.status || '';
    const selectHtml = `<select class="form-select" id="trip-status-filter">${statusOptions.map(s => `<option value="${s}" ${s === filterStatus ? 'selected' : ''}>${s || 'All Statuses'}</option>`).join('')}</select>`;

    const createBtn = hasScope('trips:write')
      ? `<button class="btn btn--primary" id="new-trip">+ New Trip</button>`
      : '';

    const sorted = [...trips].sort((a, b) => {
      const ta = a.stops && a.stops[0] ? new Date(a.stops[0].scheduled_arrive || 0).getTime() : 0;
      const tb = b.stops && b.stops[0] ? new Date(b.stops[0].scheduled_arrive || 0).getTime() : 0;
      if (ta === 0 && tb === 0) return 0;
      if (ta === 0) return 1;
      if (tb === 0) return -1;
      return tb - ta;
    });

    let rows = '';
    if (sorted.length === 0) {
      rows = `<tr><td colspan="7" style="text-align:center; padding: var(--space-5); color: var(--color-text-muted);">No trips found</td></tr>`;
    } else {
      rows = sorted.map(trip => {
        const lastStop = trip.stops && trip.stops.length > 0 ? trip.stops[trip.stops.length - 1] : null;
        const origin = trip.stops && trip.stops[0] ? (trip.stops[0].name || '—') : '—';
        const dest = lastStop ? (lastStop.name || '—') : '—';
        const pickup = fmtArrivalWindow(
          trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive : null,
          trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive_end : null,
        );
        const delivery = fmtArrivalWindow(
          lastStop ? lastStop.scheduled_arrive : null,
          lastStop ? lastStop.scheduled_arrive_end : null,
        );
        return `<tr data-trip-id="${trip.id}" style="cursor:pointer;"><td style="font-variant-numeric: tabular-nums;">${escHtml(trip.trip_number || shortId(trip.id))}</td><td>${escHtml(trip.load_number || '—')}</td><td>${badge(trip.status)}</td><td>${escHtml(trip.driver_name || '—')}</td><td>${escHtml(origin)} → ${escHtml(dest)}</td><td>${pickup}</td><td>${delivery}</td></tr>`;
      }).join('');
    }

    setContent(`
      <div class="page-header"><h1 class="page-title">Trips</h1><div class="page-controls">${selectHtml}
          ${createBtn}</div></div>
      <div class="table-wrapper">
        <table class="data-table">
          <thead><tr><th>Trip #</th><th>Load #</th><th>Status</th><th>Driver</th><th>Route</th><th>Pickup</th><th>Delivery</th></tr></thead>
          <tbody id="trips-tbody">${rows}</tbody>
        </table>
      </div>
    `);

    document.getElementById('new-trip')?.addEventListener('click', () => navigate('trip-new'));

    const filterEl = document.getElementById('trip-status-filter');
    if (filterEl) filterEl.addEventListener('change', () => navigate('trips', { status: filterEl.value }));
    document.querySelectorAll('#trips-tbody tr[data-trip-id]').forEach(row => {
      row.addEventListener('click', () => navigate('trip-detail', { id: row.dataset.tripId }));
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load trips: ${err.message}</div>`);
    }
  }
}
