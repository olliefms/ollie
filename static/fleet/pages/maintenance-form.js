import { apiFetch, API_BASE } from '../utils/api.js';
import { navigate } from '../utils/dom.js';
import { renderFormPage } from './_form.js';
import { CATEGORY_OPTIONS } from '../utils/maintenance-meta.js';

// equipment_type/equipment_id are set on create and immutable afterward.
// The backend's deny_unknown_fields rejects them on PATCH, so we omit those
// fields entirely when editing.
function fields(editing) {
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
  return editing ? shared : [...create, ...shared];
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

  renderFormPage({
    title: id ? `Edit Maintenance — ${values.service_date || ''}` : 'New Maintenance',
    fields: fields(!!id),
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
}
