import { apiFetch, API_BASE } from '../utils/api.js';
import { navigate } from '../utils/dom.js';
import { renderFormPage } from './_form.js';
import { EXPENSE_CATEGORY_OPTIONS } from '../utils/expense-meta.js';

// Receipts attach via the driver upload flow or MCP for v1, so this manager
// create form has no blob picker (noted in the PR).
function fields(editing) {
  const shared = [
    { key: 'category', label: 'Category', type: 'select', required: true,
      options: EXPENSE_CATEGORY_OPTIONS },
    { key: 'driver_id', label: 'Driver ID (UUID)', type: 'text' },
    { key: 'trip_id', label: 'Trip ID (UUID)', type: 'text' },
    { key: 'vendor', label: 'Vendor', type: 'text' },
    { key: 'expense_date', label: 'Expense Date', type: 'date' },
    { key: 'amount', label: 'Amount', type: 'number' },
  ];
  // equipment_type/equipment_id are set together on create only; the backend
  // rejects them on PATCH, so omit them when editing.
  const create = [
    { key: 'equipment_type', label: 'Equipment Type', type: 'select',
      options: [{ value: 'truck', label: 'Truck' }, { value: 'trailer', label: 'Trailer' }] },
    { key: 'equipment_id', label: 'Equipment ID (UUID)', type: 'text' },
  ];
  return editing ? shared : [...shared, ...create];
}

export async function renderExpenseForm(id, prefill = {}) {
  let values = {};
  if (id) {
    const res = await apiFetch(`${API_BASE}/expenses/${encodeURIComponent(id)}`);
    if (res.ok) values = await res.json();
  } else {
    const fromQuery = new URLSearchParams(location.search);
    values = {
      driver_id: prefill.driver_id || fromQuery.get('driver_id') || undefined,
      trip_id: prefill.trip_id || fromQuery.get('trip_id') || undefined,
      equipment_type: prefill.equipment_type || fromQuery.get('equipment_type') || undefined,
      equipment_id: prefill.equipment_id || fromQuery.get('equipment_id') || undefined,
    };
  }

  renderFormPage({
    title: id ? 'Edit Expense' : 'New Expense',
    fields: fields(!!id),
    values,
    submitLabel: id ? 'Save changes' : 'Add expense',
    onSubmit: async (payload) => {
      const url = id
        ? `${API_BASE}/expenses/${encodeURIComponent(id)}`
        : `${API_BASE}/expenses`;
      const res = await apiFetch(url, {
        method: id ? 'PATCH' : 'POST',
        body: JSON.stringify(payload),
      });
      if (res.ok) {
        const saved = await res.json().catch(() => ({}));
        navigate('expense-detail', { id: id || saved.id });
      }
      return res;
    },
  });
}
