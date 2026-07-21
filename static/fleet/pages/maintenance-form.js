import { apiFetch, API_BASE } from '../utils/api.js';
import { navigate } from '../utils/dom.js';
import { escHtml } from '../utils/format.js';
import { renderFormPage } from './_form.js';
import { CATEGORY_OPTIONS } from '../utils/maintenance-meta.js';

// equipment_type/equipment_id are set on create and immutable afterward.
// The backend's deny_unknown_fields rejects them on PATCH, so we omit those
// fields entirely when editing.
//
// Cost works the same way for a record linked to an expense (expense_id set):
// the backend now 400s ANY patch containing cost on a linked record ("cost
// is managed by the linked expense"), so we omit it on edit too. A hint is
// rendered in its place — see renderMaintenanceForm.
function fields(editing, linkedToExpense) {
  const create = [
    { key: 'equipment_type', label: 'Equipment Type', type: 'select', required: true,
      options: [{ value: 'truck', label: 'Truck' }, { value: 'trailer', label: 'Trailer' }] },
    { key: 'equipment_id', label: 'Equipment ID (UUID)', type: 'text', required: true },
  ];
  const shared = [
    { key: 'service_date', label: 'Service Date', type: 'date', required: true },
    { key: 'category', label: 'Category', type: 'select', required: true,
      options: CATEGORY_OPTIONS },
    { key: 'description', label: 'Description', type: 'text', required: true },
    { key: 'cost', label: 'Cost', type: 'number' },
    { key: 'odometer', label: 'Odometer', type: 'int' },
    { key: 'vendor', label: 'Vendor', type: 'text' },
    { key: 'invoice_ref', label: 'Invoice Ref', type: 'text' },
  ];
  const list = (editing && linkedToExpense) ? shared.filter(f => f.key !== 'cost') : shared;
  return editing ? list : [...create, ...list];
}

export async function renderMaintenanceForm(id, prefill = {}) {
  let values = {};
  // Set when arriving from an expense (category=repair): links the new
  // maintenance record to the expense and mirrors its cost server-side.
  let expenseId;
  if (id) {
    const res = await apiFetch(`${API_BASE}/maintenance/${encodeURIComponent(id)}`);
    if (res.ok) values = await res.json();
  } else {
    const fromQuery = new URLSearchParams(location.search);
    values = {
      equipment_type: prefill.equipment_type || fromQuery.get('equipment_type') || undefined,
      equipment_id: prefill.equipment_id || fromQuery.get('equipment_id') || undefined,
    };
    expenseId = prefill.expense_id || fromQuery.get('expense_id') || undefined;
  }

  const linkedToExpense = !!values.expense_id;

  renderFormPage({
    title: id ? `Edit Maintenance — ${values.service_date || ''}` : 'New Maintenance',
    fields: fields(!!id, linkedToExpense),
    values,
    submitLabel: id ? 'Save changes' : 'Add maintenance',
    onSubmit: async (payload) => {
      const url = id
        ? `${API_BASE}/maintenance/${encodeURIComponent(id)}`
        : `${API_BASE}/maintenance`;
      const body = (!id && expenseId) ? { ...payload, expense_id: expenseId } : payload;
      const res = await apiFetch(url, {
        method: id ? 'PATCH' : 'POST',
        body: JSON.stringify(body),
      });
      if (res.ok) {
        const saved = await res.json().catch(() => ({}));
        navigate('maintenance-detail', { id: id || saved.id });
      }
      return res;
    },
  });

  if (id && linkedToExpense) {
    const host = document.getElementById('form-host');
    const odometerGroup = host && host.querySelector('[data-field="odometer"]')?.closest('.form-group');
    const hint = document.createElement('div');
    hint.className = 'form-group';
    hint.innerHTML = `
      <label class="form-label">${escHtml('Cost')}</label>
      <p class="form-label" style="color:var(--color-text-muted);margin:0;">${escHtml('Cost is managed by the linked expense.')}</p>`;
    if (odometerGroup) odometerGroup.before(hint);
    else host?.querySelector('.form-panel')?.appendChild(hint);
  }
}
