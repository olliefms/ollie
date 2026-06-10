import { apiFetch, API_BASE } from '../utils/api.js';
import { badge, escHtml } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';
import { renderDetailPage } from './_detail.js';
import { confirmDelete } from '../components/confirm.js';

export async function renderTruckDetail(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/trucks/${encodeURIComponent(id)}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const t = await res.json();

    renderDetailPage({
      title: `Truck ${t.unit_number || ''}`.trim(),
      fields: [
        { label: 'Unit #', value: t.unit_number },
        { label: 'Status', html: badge(t.status) },
        { label: 'Year', value: t.year },
        { label: 'Make', value: t.make },
        { label: 'Model', value: t.model },
        { label: 'VIN', value: t.vin },
        { label: 'Plate', value: t.plate },
        { label: 'Plate State', value: t.plate_state },
        { label: 'Notes', value: t.notes },
      ],
      actions: [
        { label: 'Edit', scope: 'trucks:write', onClick: () => navigate('truck-edit', { id }) },
        { label: 'Delete', scope: 'trucks:delete', onClick: (statusEl) => deleteTruck(statusEl, id, t.unit_number) },
      ],
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load truck: ${escHtml(err.message)}</div>`);
    }
  }
}

// Soft delete: backend sets status = Inactive (hides from active lists).
async function deleteTruck(statusEl, id, unit) {
  if (!confirmDelete(`truck "${unit}"`)) return;
  try {
    const res = await apiFetch(`${API_BASE}/trucks/${encodeURIComponent(id)}`, { method: 'DELETE' });
    if (res.ok || res.status === 204) { navigate('trucks'); return; }
    const data = await res.json().catch(() => ({}));
    statusEl.hidden = false;
    statusEl.className = 'alert alert--error';
    statusEl.textContent = data.error || `Delete failed (HTTP ${res.status}).`;
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      statusEl.hidden = false;
      statusEl.className = 'alert alert--error';
      statusEl.textContent = `Delete failed: ${err.message}`;
    }
  }
}
