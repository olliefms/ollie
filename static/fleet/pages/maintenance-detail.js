import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';
import { renderDetailPage, detailLink } from './_detail.js';
import { confirmDelete } from '../components/confirm.js';
import { categoryLabel, money } from '../utils/maintenance-meta.js';

export async function renderMaintenanceDetail(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/maintenance/${encodeURIComponent(id)}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const m = await res.json();

    renderDetailPage({
      title: `Maintenance — ${m.service_date || ''}`.trim(),
      fields: [
        { label: 'Service Date', value: m.service_date },
        { label: 'Equipment Type', value: m.equipment_type },
        { label: 'Equipment ID', value: m.equipment_id },
        { label: 'Category', value: categoryLabel(m.category) },
        { label: 'Description', value: m.description },
        { label: 'Cost', value: money(m.cost) },
        { label: 'Odometer', value: m.odometer },
        { label: 'Vendor', value: m.vendor },
        { label: 'Invoice Ref', value: m.invoice_ref },
        ...(m.expense_id
          ? [{ label: 'Linked expense', html: detailLink('expense-detail', m.expense_id, 'View expense') }]
          : []),
      ],
      actions: [
        { label: 'Edit', scope: 'maintenance:write', onClick: () => navigate('maintenance-edit', { id }) },
        { label: 'Delete', scope: 'maintenance:delete', onClick: (statusEl) => deleteMaintenance(statusEl, id) },
      ],
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load maintenance: ${escHtml(err.message)}</div>`);
    }
  }
}

// Hard delete: the entry is removed entirely.
async function deleteMaintenance(statusEl, id) {
  if (!confirmDelete('this maintenance entry')) return;
  try {
    const res = await apiFetch(`${API_BASE}/maintenance/${encodeURIComponent(id)}`, { method: 'DELETE' });
    if (res.ok || res.status === 204) { navigate('maintenance'); return; }
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
