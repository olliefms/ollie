import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent } from '../utils/dom.js';
import { renderEntityList } from './_list.js';

export async function renderTrailersView() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/trailers`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.items || (Array.isArray(data) ? data : []);

    renderEntityList({
      title: 'Trailers',
      createView: 'trailer-new',
      createScope: 'trailers:write',
      createLabel: '+ Add Trailer',
      detailView: 'trailer-detail',
      emptyText: 'No trailers found.',
      columns: [
        { header: 'Unit #', cell: t => t.unit_number },
        { header: 'Owner',  cell: t => (t.owner_name || t.owner || '—') },
        { header: 'Type',   cell: t => (t.trailer_type || '—') },
        { header: 'Length', cell: t => (t.length_ft != null ? `${t.length_ft} ft` : '—') },
        { header: 'Status', cell: t => (t.status || '—') },
      ],
      rows: items,
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load trailers: ${escHtml(err.message)}</div>`);
    }
  }
}
