import { apiFetch, API_BASE } from '../utils/api.js';
import { badge, escHtml } from '../utils/format.js';
import { setContent } from '../utils/dom.js';
import { renderEntityList } from './_list.js';

export async function renderDriversView() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/drivers`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.drivers || data.items || (Array.isArray(data) ? data : []);

    renderEntityList({
      title: 'Drivers',
      createView: 'driver-new',
      createScope: 'drivers:write',
      createLabel: '+ Add Driver',
      detailView: 'driver-detail',
      emptyText: 'No drivers found.',
      columns: [
        { header: 'Name',   cell: d => d.name || '—' },
        { header: 'Status', cell: d => badge(d.status), html: true },
        { header: 'Phone',  cell: d => d.phone || '—' },
      ],
      rows: items,
      rowClass: d => d.status === 'available' ? 'row--available' : '',
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load drivers: ${escHtml(err.message)}</div>`);
    }
  }
}
