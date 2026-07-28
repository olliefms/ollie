import { apiFetch, API_BASE, hasScope } from '../utils/api.js';
import { escHtml, badge, shortId, fmtArrivalWindow } from '../utils/format.js';
import { setContent, navigate, setTopbarControls } from '../utils/dom.js';

const PAGE_SIZE = 20;
// Mirrors LOAD_SCAN_CAP in src/db/load_ops.rs: offsets at or past the cap
// return empty pages even though `total` still reflects the full count.
const LOAD_SCAN_CAP = 2000;

export async function renderLoadsView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  const status = params.status || '';
  let loaded = [];
  let total = null;
  let hasMore = false;

  const buildContent = (loads) => {
    const capBanner = total !== null && total > LOAD_SCAN_CAP && loads.length >= LOAD_SCAN_CAP
      ? `<div style="background:var(--color-warning-soft);border:1px solid var(--color-warning);border-radius:var(--radius);padding:var(--space-3) var(--space-4);margin-bottom:var(--space-4);font-size:var(--text-sm);color:var(--color-text);">
           Showing the most recent ${escHtml(String(loads.length))} of ${escHtml(String(total))} loads. Use the status filter to narrow results.
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
      ${hasMore ? `
        <div style="text-align:center;margin-top:var(--space-3);">
          <button class="btn btn--secondary" id="loads-load-more">Load more</button>
        </div>` : ''}
    `;
  };

  const fetchPage = async (offset) => {
    const qs = new URLSearchParams({ limit: PAGE_SIZE, offset });
    if (status) qs.set('status', status);
    const res = await apiFetch(`${API_BASE}/loads?${qs}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const loads = data.loads || data.items || (Array.isArray(data) ? data : []);
    total = typeof data.total === 'number' ? data.total : null;
    loaded = offset === 0 ? loads : loaded.concat(loads);
    // Cap the pager at the DB scan ceiling; without a server total, fall back
    // to "a full page probably means more". An empty page always stops.
    const reachable = total !== null ? Math.min(total, LOAD_SCAN_CAP) : null;
    hasMore = loads.length === 0 ? false
      : reachable !== null ? loaded.length < reachable
      : loads.length === PAGE_SIZE;
  };

  const render = () => {
    setContent(buildContent(loaded));

    document.getElementById('loads-load-more')?.addEventListener('click', async (e) => {
      e.target.disabled = true;
      try {
        await fetchPage(loaded.length);
        render();
      } catch (err) {
        if (err.message !== 'Unauthorized — please sign in again.') {
          setContent(`<div class="state-error">Failed to load data: ${escHtml(err.message)}</div>`);
        }
      }
    });

    document.querySelectorAll('#loads-tbody tr[data-load-id]').forEach(row => {
      row.addEventListener('click', () => {
        navigate('load-detail', { id: row.dataset.loadId });
      });
    });
  };

  try {
    await fetchPage(0);
    render();

    const statusOptions = [
      '', 'planned', 'assigned', 'dispatched', 'in_transit',
      'delivered', 'invoiced', 'settled', 'cancelled',
    ];
    const selectHtml = `
      <select class="form-select" id="status-filter">
        ${statusOptions.map(s =>
          `<option value="${s}" ${s === status ? 'selected' : ''}>${s || 'All Statuses'}</option>`
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
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load data: ${escHtml(err.message)}</div>`);
    }
  }
}
