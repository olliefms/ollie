import { apiFetch, API_BASE } from '../utils/api.js';
import { badge } from '../utils/format.js';
import { setContent } from '../utils/dom.js';
import { renderEntityList } from './_list.js';

export async function renderFacilitiesView() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/facilities`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.items || (Array.isArray(data) ? data : []);

    renderEntityList({
      title: 'Facilities',
      createView: 'facility-new',
      createScope: 'facilities:write',
      createLabel: '+ Add Facility',
      detailView: 'facility-detail',
      emptyText: 'No facilities found.',
      columns: [
        { header: 'Name',    cell: f => f.name || '—' },
        { header: 'Address', cell: f => f.normalized_address || f.address || '—' },
        { header: 'Geocode', cell: f => badge(f.geocode_status), html: true },
      ],
      rows: items,
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load facilities: ${err.message}</div>`);
    }
  }
}
