import { apiFetch, API_BASE } from '../utils/api.js';
import { badge, escHtml } from '../utils/format.js';
import { setContent } from '../utils/dom.js';
import { renderEntityList } from './_list.js';

let showArchived = false;

async function renderFacilitiesView() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const qs = showArchived ? '?include_archived=true' : '';
    const res = await apiFetch(`${API_BASE}/facilities${qs}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.items || (Array.isArray(data) ? data : []);

    const columns = [
      { header: 'Name',    cell: f => f.name || '—' },
      { header: 'Address', cell: f => f.normalized_address || f.address || '—' },
      { header: 'Geocode', cell: f => badge(f.geocode_status), html: true },
    ];
    if (showArchived) {
      columns.push({ header: 'Status', cell: f => badge(f.archived ? 'archived' : 'active'), html: true });
    }

    renderEntityList({
      title: 'Facilities',
      createView: 'facility-new',
      createScope: 'facilities:write',
      createLabel: '+ Add Facility',
      detailView: 'facility-detail',
      emptyText: 'No facilities found.',
      columns,
      rows: items,
      extraControls: (controlsEl) => {
        const label = document.createElement('label');
        label.style.cssText = 'display:flex;align-items:center;gap:var(--space-2);font-size:var(--text-sm);cursor:pointer;';
        const cb = document.createElement('input');
        cb.type = 'checkbox';
        cb.checked = showArchived;
        cb.addEventListener('change', () => {
          showArchived = cb.checked;
          renderFacilitiesView();
        });
        label.appendChild(cb);
        label.appendChild(document.createTextNode('Show archived'));
        controlsEl.appendChild(label);
      },
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load facilities: ${escHtml(err.message)}</div>`);
    }
  }
}

export { renderFacilitiesView };
