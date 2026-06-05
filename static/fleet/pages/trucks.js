import { apiFetch, API_BASE } from '../utils/api.js';
import { setContent } from '../utils/dom.js';
import { renderEntityList } from './_list.js';

export async function renderTrucksView() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/trucks`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.items || (Array.isArray(data) ? data : []);

    renderEntityList({
      title: 'Trucks',
      createView: 'truck-new',
      createScope: 'trucks:write',
      createLabel: '+ Add Truck',
      detailView: 'truck-detail',
      emptyText: 'No trucks found.',
      columns: [
        { header: 'Unit #', cell: t => t.unit_number },
        { header: 'Year',   cell: t => (t.year ?? '—') },
        { header: 'Make',   cell: t => (t.make || '—') },
        { header: 'Model',  cell: t => (t.model || '—') },
        { header: 'Status', cell: t => (t.status || '—') },
      ],
      rows: items,
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load trucks: ${err.message}</div>`);
    }
  }
}
