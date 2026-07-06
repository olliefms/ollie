import { apiFetch, API_BASE, hasScope } from '../utils/api.js';
import { escHtml, badge, shortId, fmtArrivalWindow } from '../utils/format.js';
import { setContent, navigate, setTopbarControls } from '../utils/dom.js';

const LOAD_SCAN_CAP = 2000;

export async function renderLoadsView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  let initialStatus = params.status || '';

  const buildContent = (loads, filterStatus, capTotal = null) => {
    const capBanner = capTotal !== null
      ? `<div style="background:var(--color-warning-soft);border:1px solid var(--color-warning);border-radius:var(--radius);padding:var(--space-3) var(--space-4);margin-bottom:var(--space-4);font-size:var(--text-sm);color:var(--color-text);">
           Showing the most recent ${escHtml(String(loads.length))} of ${escHtml(String(capTotal))} loads. Use the status filter to narrow results.
         </div>`
      : '';

    const sorted = [...loads].sort((a, b) => {
      const ta = a.stops && a.stops[0] ? new Date(a.stops[0].scheduled_arrive || 0).getTime() : 0;
      const tb = b.stops && b.stops[0] ? new Date(b.stops[0].scheduled_arrive || 0).getTime() : 0;
      if (ta === 0 && tb === 0) return 0;
      if (ta === 0) return 1;
      if (tb === 0) return -1;
      return tb - ta;
    });

    let rows = '';
    if (sorted.length === 0) {
      rows = `<tr><td colspan="6" style="text-align:center; padding: var(--space-5); color: var(--color-text-muted);">No loads found</td></tr>`;
    } else {
      rows = sorted.map(load => {
        const stops = load.stops || [];
        const last = stops.length - 1;
        const origin = stops[0]?.name || '—';
        const dest = stops[last]?.name || '—';
        return `
        <tr data-load-id="${load.id}">
          <td style="font-variant-numeric: tabular-nums;">${escHtml(load.load_number || shortId(load.id))}</td>
          <td>${badge(load.status)}</td>
          <td>${escHtml(load.customer_name || '—')}</td>
          <td>${escHtml(origin)} → ${escHtml(dest)}</td>
          <td>${fmtArrivalWindow(stops[0]?.scheduled_arrive, stops[0]?.scheduled_arrive_end)}</td>
          <td>${fmtArrivalWindow(stops[last]?.scheduled_arrive, stops[last]?.scheduled_arrive_end)}</td>
        </tr>
      `;
      }).join('');
    }

    return `
      ${capBanner}<div class="table-wrapper">
        <table class="data-table">
          <thead>
            <tr>
              <th>Load #</th>
              <th>Status</th>
              <th>Customer</th>
              <th>Route</th>
              <th>Pickup</th>
              <th>Delivery</th>
            </tr>
          </thead>
          <tbody id="loads-tbody">
            ${rows}
          </tbody>
        </table>
      </div>
    `;
  };

  const fetchAndRender = async (status) => {
    try {
      const qs = status ? `?status=${encodeURIComponent(status)}` : '';
      const res = await apiFetch(`${API_BASE}/loads${qs}`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      const loads = data.loads || data.items || (Array.isArray(data) ? data : []);
      const returned = typeof data.returned === 'number' ? data.returned : null;
      // The server caps scans at LOAD_SCAN_CAP (2000). Only show the cap
      // banner when we're actually at that ceiling — otherwise it fires on
      // every normal paginated result where total exceeds page size.
      const capTotal = returned !== null && returned >= LOAD_SCAN_CAP ? returned : null;
      setContent(buildContent(loads, status, capTotal));

      const statusOptions = [
        '', 'planned', 'assigned', 'dispatched', 'in_transit',
        'delivered', 'invoiced', 'settled', 'cancelled',
      ];
      const filterStatus = status || '';
      const selectHtml = `
        <select class="form-select" id="status-filter">
          ${statusOptions.map(s =>
            `<option value="${s}" ${s === filterStatus ? 'selected' : ''}>${s || 'All Statuses'}</option>`
          ).join('')}
        </select>
      `;
      const createBtn = hasScope('loads:write')
        ? `<button class="btn btn--primary" id="new-load">+ New Load</button>`
        : '';
      setTopbarControls((slot) => { slot.innerHTML = `${selectHtml}${createBtn}`; });

      document.getElementById('new-load')?.addEventListener('click', () => navigate('load-new'));

      const filterEl = document.getElementById('status-filter');
      if (filterEl) {
        filterEl.addEventListener('change', () => {
          navigate('loads', { status: filterEl.value });
        });
      }

      document.querySelectorAll('#loads-tbody tr[data-load-id]').forEach(row => {
        row.addEventListener('click', () => {
          navigate('load-detail', { id: row.dataset.loadId });
        });
      });
    } catch (err) {
      if (err.message !== 'Unauthorized — please sign in again.') {
        setContent(`<div class="state-error">Failed to load data: ${escHtml(err.message)}</div>`);
      }
    }
  };

  await fetchAndRender(initialStatus);
}
